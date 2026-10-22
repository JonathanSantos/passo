//! The use cases: start, next, done, ask and the dev's decisions. Each one loads the run, gathers facts from the
//! world, lets the engine decide, then persists the result and carries out the effects.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use globset::{Glob, GlobSet, GlobSetBuilder};
use passo_adapters::{Platform, Usage};
use passo_core::answers::parse_answers;
use passo_core::config::{Delivery, GitMode};
use passo_core::spec::{self, Invalid};
use passo_core::*;
use passo_store::{Catalog, Layout, Store};
use serde_json::json;

use crate::infra::{CheckRunner, Clock, Git, ShellRunner, SystemClock};
use crate::metrics;
use crate::render::{self, View};
use crate::snapshot::{RunSummary, Snapshot, StageInfo};

/// Writes the delivery report. The web crate implements it; the app only knows the trait.
pub trait Reporter: Send + Sync {
    fn write(&self, snapshot: &Snapshot, path: &Path) -> Result<()>;
}

#[derive(Debug, Clone)]
pub enum DevAction {
    Approve(Vec<String>),
    Reject(String),
    Change(String),
    Resize(Size),
    Stop(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Cli,
    Chat,
    Web,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Source::Cli => "terminal",
            Source::Chat => "chat",
            Source::Web => "painel",
        }
    }
}

/// What the agent handed in with `passo done`.
#[derive(Debug, Clone, Default)]
pub struct DoneInput {
    /// A summary (build), a file path, `-` for stdin, or the JSON itself.
    pub text: String,
    pub empty_ok: bool,
    pub note: Option<String>,
}

/// A use case that the agent can fix: shown on stdout with a distinct exit code.
#[derive(Debug)]
pub struct Rejected(pub String);

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Rejected {}

pub struct App {
    pub layout: Layout,
    pub store: Store,
    pub config: Config,
    pub layers: Vec<String>,
    pub catalog: Catalog,
    pub bin: String,
    /// The project map as read when the app opened: `Err` holds why it was ignored. Never fails the app: a broken
    /// map file must not switch the lock off.
    pub map: std::result::Result<Option<passo_core::map::Map>, String>,
    clock: Arc<dyn Clock>,
    runner: Arc<dyn CheckRunner>,
    reporter: Option<Arc<dyn Reporter>>,
}

impl App {
    /// Opens the project around `start` (the nearest folder with `.passo/`).
    pub fn open(start: &Path) -> Result<App> {
        let layout = Layout::find(start)
            .ok_or_else(|| anyhow!("este projeto ainda não usa o Passo. Rode `passo init` na raiz do projeto."))?;
        Self::at(layout)
    }

    pub fn at(layout: Layout) -> Result<App> {
        let loaded = passo_store::config::load(&layout)?;
        let catalog = Catalog::load(&layout)?;
        let mut config = loaded.config;
        fill_platform_models(&mut config);
        let attached = attached_roots(&layout.root, &config);
        let map = passo_store::map::load_all(&layout, &attached).map_err(|e| format!("{e:#}"));
        Ok(App {
            store: Store::new(layout.clone()),
            layout,
            config,
            layers: loaded.layers,
            catalog,
            bin: "passo".into(),
            map,
            clock: Arc::new(SystemClock),
            runner: Arc::new(ShellRunner),
            reporter: None,
        })
    }

    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_runner(mut self, runner: Arc<dyn CheckRunner>) -> Self {
        self.runner = runner;
        self
    }

    pub fn with_reporter(mut self, reporter: Arc<dyn Reporter>) -> Self {
        self.reporter = Some(reporter);
        self
    }

    fn engine(&self) -> Engine<'_> {
        Engine::new(&self.config)
    }

    pub(crate) fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.clock.now()
    }

    pub(crate) fn git(&self) -> Git {
        Git::new(&self.layout.root)
    }

    pub fn uses_git(&self, run: &Run) -> bool {
        self.config.git.mode == GitMode::Branch && run.workspace.as_ref().is_some_and(|w| w.branch.is_some())
    }

    /// Platforms installed in this project.
    pub fn platforms(&self) -> Vec<Box<dyn Platform>> {
        passo_adapters::registry()
            .into_iter()
            .filter(|p| p.installed(&self.layout.root))
            .collect()
    }

    /// Refuses to go on when the repository is not on the run's branch: a commit now would put the agent's work
    /// wherever the dev happens to be.
    fn ensure_run_branch(&self, run: &Run) -> Result<()> {
        let Some(expected) = run
            .workspace
            .as_ref()
            .filter(|_| self.uses_git(run))
            .and_then(|w| w.branch.as_deref())
        else {
            return Ok(());
        };
        let current = self.git().branch();
        if current.as_deref() == Some(expected) {
            return Ok(());
        }
        Err(Rejected(format!(
            "o repositório está em {}, mas o run trabalha em {expected}. Peça ao dev para voltar com `git switch {expected}` e tente de novo.",
            current.unwrap_or_else(|| "um commit solto".into())
        ))
        .into())
    }

    pub fn active(&self) -> Result<Run> {
        self.store
            .active()
            .ok_or_else(|| anyhow!("não há run ativo. Comece com: passo start \"pedido\" --size m"))
    }

    // -- use cases -------------------------------------------------------------------------------------------

    pub fn start(&self, request: &str, size: Option<Size>) -> Result<String> {
        if request.trim().is_empty() {
            bail!("diga o pedido: passo start \"o que o dev pediu\"");
        }
        if let Some(run) = self.store.active() {
            bail!(
                "já há um run ativo: {}\nSiga com `passo next`. Para começar outro, o dev encerra este com `passo stop`.",
                run.headline()
            );
        }
        if self.mapping() {
            return Err(Rejected(
                "há um mapeamento em andamento; entregue o mapa com passo done ou cancele com passo map --cancel"
                    .into(),
            )
            .into());
        }
        if passo_store::learn::load_task(&self.layout).is_some() {
            return Err(Rejected(
                "há um aprendizado com os runs em andamento; o dev aplica com passo learn --apply ou descarta com passo learn --cancel"
                    .into(),
            )
            .into());
        }
        let now = self.clock.now();
        let size = size.unwrap_or(self.config.default_size);
        let id = make_id(request, now);
        let workspace = self.prepare_workspace(&id)?;
        self.store.init()?;
        let (mut run, out) = self.engine().start(&id, request, size, now);
        run.workspace = Some(workspace);
        self.store.set_current(Some(&id))?;
        self.write_tree(&run);
        let run = self.persist(run, out, None)?;
        let mut lines: Vec<String> = self.attached_warnings();
        lines.extend(self.map_stale_line(&run));
        lines.push(self.next_text(&run));
        Ok(lines.join("\n"))
    }

    /// Lists the repository tree for the run (on start, and again when the dev changes the request).
    fn write_tree(&self, run: &Run) {
        let git = self.uses_git(run).then(|| self.git());
        let tree = crate::tree::build(&self.layout.root, git.as_ref(), &self.config, 60);
        if let Err(e) = passo_store::layout::write_atomic(&self.layout.tree(&run.id), tree.as_bytes()) {
            let event = draft(
                run,
                "tree.failed",
                Actor::Passo,
                format!("A árvore do repositório não foi gravada: {e}"),
            );
            self.store.log(&run.id).append(&run.id, &[event], self.clock.now()).ok();
        }
    }

    fn prepare_workspace(&self, id: &str) -> Result<Workspace> {
        let root = self.layout.root.display().to_string();
        let git = self.git();
        if self.config.git.mode != GitMode::Branch || !git.is_repo() {
            return Ok(Workspace {
                dir: root,
                branch: None,
                base: None,
                from: None,
            });
        }
        let base = git
            .head()
            .ok_or_else(|| anyhow!("o repositório ainda não tem commits; faça o primeiro commit antes de começar"))?;
        let dirty = git.dirty()?;
        if !dirty.is_empty() {
            let list: Vec<String> = dirty.iter().take(8).map(|f| format!("  {f}")).collect();
            bail!(
                "há mudanças não commitadas:\n{}\nPeça ao dev para commitar (ou `git stash`) antes de começar. O Passo trabalha num branch próprio e faz um commit por passo.",
                list.join("\n")
            );
        }
        let from = git.branch();
        let branch = format!("passo/{id}");
        git.create_branch(&branch)?;
        Ok(Workspace {
            dir: root,
            branch: Some(branch),
            base: Some(base),
            from,
        })
    }

    pub fn next(&self) -> Result<String> {
        if self.store.active().is_none() && self.mapping() {
            return Ok(self.map_text());
        }
        if self.store.active().is_none()
            && let Some(task) = passo_store::learn::load_task(&self.layout)
        {
            return Ok(self.learn_text(&task));
        }
        match self.store.latest() {
            Some(run) => Ok(self.next_text(&self.spend(run)?)),
            None => Ok("Nenhum run ainda. Comece com: passo start \"pedido do dev\" --size m".into()),
        }
    }

    /// How the main agent hands work to `passo-<agent>` on the installed platform(s).
    pub(crate) fn delegation(&self, agent: &str) -> String {
        match self.platforms().as_slice() {
            [one] => one.delegation(agent),
            _ => format!(
                "Se a sua plataforma roda subagentes, use `passo-{agent}` e passe a ele esta saída inteira; senão, faça você mesmo, seguindo o papel abaixo."
            ),
        }
    }

    pub fn next_text(&self, run: &Run) -> String {
        let delegation = |agent: &str| self.delegation(agent);
        let tree = std::fs::read_to_string(self.layout.tree(&run.id)).ok();
        let files = self.map_files(run);
        let selection = self.map_selection(run, &files);
        let text = render::next(&View {
            run,
            config: &self.config,
            catalog: &self.catalog,
            delegation: &delegation,
            bin: &self.bin,
            tree: tree.as_deref(),
            map: selection.as_ref(),
        });
        let warnings: Vec<String> = self.hook_warning(run).into_iter().chain(self.map_warning()).collect();
        if warnings.is_empty() {
            text
        } else {
            format!("{}\n{text}", warnings.join("\n"))
        }
    }

    /// One line per attached project that has no map yet.
    pub fn attached_warnings(&self) -> Vec<String> {
        self.config
            .projects
            .iter()
            .zip(attached_roots(&self.layout.root, &self.config))
            .filter(|(_, (_, root))| !Layout::new(root).project_map().join("areas").is_dir())
            .map(|(p, _)| {
                format!(
                    "Aviso: projeto anexado {} sem mapa: rode passo map em {}",
                    p.name, p.path
                )
            })
            .collect()
    }

    /// One line when hooks failed since the run began: the lock may not be holding.
    pub fn hook_warning(&self, run: &Run) -> Option<String> {
        let text = std::fs::read_to_string(self.layout.hook_errors()).ok()?;
        let errors: Vec<serde_json::Value> = text
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|e| {
                e.get("t")
                    .and_then(|t| t.as_str())
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                    .is_some_and(|t| t >= run.created_at)
            })
            .collect();
        let last = errors
            .last()?
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("?")
            .to_string();
        Some(format!(
            "⚠ {} erro(s) de hook neste run (último: {last}). A trava pode não estar valendo; rode passo doctor.",
            errors.len()
        ))
    }

    /// One line when the project map was ignored, for the dev and the agent.
    pub fn map_warning(&self) -> Option<String> {
        self.map
            .as_ref()
            .err()
            .map(|e| format!("Aviso: mapa do projeto ignorado: {e}"))
    }

    /// The files the stage is working on, for the map: the brief's, the step's, or the run's diff.
    pub(crate) fn map_files(&self, run: &Run) -> Vec<String> {
        match run.stage {
            Stage::Planejar => run.brief.as_ref().map(|b| b.files.clone()).unwrap_or_default(),
            Stage::Construir => run.current_step().map(|s| s.files.clone()).unwrap_or_default(),
            Stage::Verificar => match (
                self.uses_git(run),
                run.workspace.as_ref().and_then(|w| w.base.as_deref()),
            ) {
                (true, Some(base)) => self.git().changed_since(base).unwrap_or_default(),
                _ => vec![],
            },
            Stage::Refinar | Stage::Entregar => vec![],
        }
    }

    /// The part of the project map the run's stage reads, within the size's line budget.
    pub(crate) fn map_selection<'a>(&'a self, run: &Run, files: &[String]) -> Option<passo_core::map::Selection<'a>> {
        let map = self.map.as_ref().ok()?.as_ref()?;
        let words = match run.stage {
            Stage::Planejar => run.brief.as_ref().map(|b| b.understanding.clone()).unwrap_or_default(),
            _ => {
                let changes = run
                    .changes
                    .iter()
                    .filter(|c| c.kind == ChangeKind::Mudanca)
                    .map(|c| c.text.as_str());
                std::iter::once(run.request.as_str())
                    .chain(changes)
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        };
        let budget = self.config.profile(run.size).map_lines.for_stage(run.stage);
        let target = passo_core::map::Target {
            stage: run.stage,
            files,
            words: &words,
        };
        let selection = passo_core::map::select(map, &target, budget);
        (!selection.nodes.is_empty()).then_some(selection)
    }

    pub fn done(&self, input: DoneInput) -> Result<String> {
        if self.store.active().is_none() && self.mapping() {
            return self.map_done(input);
        }
        if self.store.active().is_none() && passo_store::learn::load_task(&self.layout).is_some() {
            return self.learn_done(input);
        }
        let run = self.active()?;
        self.ensure_run_branch(&run)?;
        if run.turn() == Turn::Dev {
            return Err(Rejected(format!(
                "Agora é a vez do dev; o Passo não aceita entregas.\n\n{}",
                self.next_text(&run)
            ))
            .into());
        }
        let command = match run.stage {
            Stage::Refinar => {
                let text = read_input(&input.text)?;
                let brief = spec::parse_brief(&text, run.questions.len() + 1);
                self.check_read_only(&run)?;
                Command::SubmitBrief(self.valid(&run, brief)?)
            }
            Stage::Planejar => {
                let text = read_input(&input.text)?;
                self.check_read_only(&run)?;
                Command::SubmitPlan(self.valid(&run, spec::parse_plan(&text))?)
            }
            Stage::Construir => {
                if input.text.trim().is_empty() {
                    return Err(Rejected("diga o que mudou: passo done \"resumo numa linha\"".into()).into());
                }
                Command::StepDone(self.measure(&run, &input)?)
            }
            Stage::Verificar if run.reviewing => {
                let text = read_input(&input.text)?;
                self.check_read_only(&run)?;
                let ids: Vec<String> = run.criteria.iter().map(|c| c.id.clone()).collect();
                let files = self.map_files(&run);
                let areas: Vec<String> = self
                    .map_selection(&run, &files)
                    .map(|s| {
                        s.nodes
                            .iter()
                            .filter(|n| n.kind == passo_core::map::NodeKind::Area)
                            .map(|n| n.id.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                let review = self.valid(&run, spec::parse_review(&text, &ids, &areas))?;
                self.valid(&run, self.check_evidence(&run, &review))?;
                Command::SubmitReview(review)
            }
            stage => bail!("nada a entregar em {}; rode `passo next`", stage.label()),
        };
        let run = self.transition(&run, command, None)?;
        let run = self.spend(run)?;
        Ok(self.next_text(&run))
    }

    pub fn ask(&self, question: &str, options: &[String]) -> Result<String> {
        let run = self.active()?;
        let question = self.valid(&run, spec::question_from_cli(String::new(), question, options))?;
        let run = self.transition(&run, Command::Ask(question), None)?;
        let run = self.spend(run)?;
        Ok(self.next_text(&run))
    }

    pub fn decide(&self, action: DevAction, source: Source) -> Result<Run> {
        let run = self.active()?;
        self.ensure_run_branch(&run)?;
        // The last measurement before the dev's word: an accepted run keeps the tokens it really spent.
        let run = self.spend(run)?;
        let command = match action {
            DevAction::Approve(words) => Command::Approve(parse_answers(&words)),
            DevAction::Reject(text) => Command::Reject(text),
            DevAction::Change(text) => Command::Change(text),
            DevAction::Resize(size) => Command::Resize(size),
            DevAction::Stop(reason) => Command::Stop(reason),
        };
        let stopping = matches!(command, Command::Stop(_));
        let changing = matches!(command, Command::Change(_));
        let next = self.transition(&run, command, Some(source))?;
        if stopping {
            self.leave_branch(&next)?;
        }
        if changing {
            self.write_tree(&next);
        }
        Ok(next)
    }

    /// A stopped run keeps its work on its branch, committed, and the dev goes back where they were.
    fn leave_branch(&self, run: &Run) -> Result<()> {
        let Some(ws) = run.workspace.as_ref().filter(|_| self.uses_git(run)) else {
            return Ok(());
        };
        let (Some(branch), Some(back)) = (ws.branch.as_ref(), ws.from.as_ref()) else {
            return Ok(());
        };
        let git = self.git();
        self.store.archive(&run.id)?;
        git.stage_all()?;
        git.commit(&self.commit_message(run, "trabalho em andamento (run interrompido)")?)?;
        git.switch(back)?;
        let text = format!("O trabalho ficou em {branch}; de volta em {back}");
        self.store.log(&run.id).append(
            &run.id,
            &[draft(run, "branch.left", Actor::Passo, text)],
            self.clock.now(),
        )?;
        Ok(())
    }

    /// Leaves a `lock.denied` in the active run's log for a refusal outside the hooks (the CLI in an agent's
    /// environment). Best effort: the refusal stands whether or not there is a run to log it in.
    pub fn record_denial(&self, text: &str, via: &str) {
        let Some(run) = self.store.active() else { return };
        let event = EventDraft {
            kind: "lock.denied",
            actor: Actor::Agent,
            stage: run.stage,
            text: text.to_string(),
            data: json!({ "via": via }),
        };
        self.store.log(&run.id).append(&run.id, &[event], self.clock.now()).ok();
    }

    /// One line about the project map for `doctor`: none yet, its size and age, or why it was ignored.
    pub fn map_summary(&self) -> String {
        match passo_store::map::load(&self.layout) {
            Ok(None) => "nenhum (opcional: peça ao agente \"com o passo, mapeie o projeto\")".into(),
            Err(e) => format!("ignorado: {e}"),
            Ok(Some(map)) => {
                let commit = map.project.iter().chain(&map.areas).find_map(|n| n.commit.clone());
                let made = match &commit {
                    Some(c) => {
                        let days = self
                            .git()
                            .commit_time(c)
                            .map(|t| (self.clock.now().timestamp() - t).max(0) / 86_400)
                            .map(|d| format!(" ({d} dias)"))
                            .unwrap_or_default();
                        format!(" · feito em {}{days}", &c[..c.len().min(7)])
                    }
                    None => String::new(),
                };
                format!("{} áreas · {} libs{made}", map.areas.len(), map.libs.len())
            }
        }
    }

    /// One screen for the dev: where the run is and what it waits for.
    pub fn status_text(&self) -> String {
        if self.store.active().is_none()
            && let Some(task) = passo_store::map::load_task(&self.layout)
        {
            return format!(
                "Mapeamento do projeto em andamento desde {}.\nO agente entrega o mapa com `passo done <arquivo>`; o dev cancela com `passo map --cancel`.\n",
                task.started_at.format("%d/%m %H:%M")
            );
        }
        if self.store.active().is_none()
            && let Some(task) = passo_store::learn::load_task(&self.layout)
        {
            return if task.delivered() {
                self.learn_text(&task)
            } else {
                format!(
                    "Aprendizado com os runs em andamento desde {}.\nO agente entrega as propostas com `passo done <arquivo>`; o dev descarta com `passo learn --cancel`.\n",
                    task.started_at.format("%d/%m %H:%M")
                )
            };
        }
        let Some(run) = self.store.latest() else {
            return "Nenhum run ainda. No agente, peça algo \"com o passo\", ou rode: passo start \"pedido\"".into();
        };
        let mut out = format!("{}\n{}\n", run.headline(), render::rail(&run, &self.config));
        if let Some(warning) = self.hook_warning(&run).filter(|_| run.active()) {
            out = format!("{warning}\n{out}");
        }
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("Pedido: {}\n", run.request));
        if let Some(tokens) = render::tokens_line(&run, &self.config) {
            out.push_str(&tokens);
            out.push('\n');
        }
        match run.turn() {
            Turn::Dev => {
                out.push('\n');
                out.push_str(&self.gate_text(&run));
                out.push_str("\nNo terminal: passo ok · passo ok 1b · passo no \"o que mudar\"\n");
            }
            Turn::Agent => out.push_str("\nO agente está trabalhando. Acompanhe com `passo ui`.\n"),
            Turn::None => {}
        }
        if let Some(hint) = self.learn_hint().filter(|_| !run.active()) {
            out.push('\n');
            out.push_str(&hint);
            out.push('\n');
        }
        out
    }

    pub fn gate_text(&self, run: &Run) -> String {
        let delegation = |_: &str| String::new();
        render::gate(&View {
            run,
            config: &self.config,
            catalog: &self.catalog,
            delegation: &delegation,
            bin: &self.bin,
            tree: None,
            map: None,
        })
    }

    // -- audit -----------------------------------------------------------------------------------------------

    /// `passo(<run>): <text>` plus a trailer with the hash of the last event, so git history anchors the log.
    fn commit_message(&self, run: &Run, text: &str) -> Result<String> {
        let mut message = format!("passo({}): {text}", run.id);
        if let Some(last) = self.store.log(&run.id).last()? {
            message.push_str(&format!("\n\n{AUDIT_TRAILER}: {}", last.hash));
        }
        Ok(message)
    }

    /// Checks the log's chain and, with git, that the newest `Passo-Audit` trailer on the run's branch names an
    /// event that is still in the log. `Ok(n)` with the number of events, or what is wrong.
    pub fn verify_audit(&self, run: &Run) -> Result<std::result::Result<usize, String>> {
        let log = self.store.read_log(&run.id);
        let count = match log.verify()? {
            Ok(count) => count,
            Err((seq, why)) => return Ok(Err(format!("evento {seq}: {why}"))),
        };
        let Some(branch) = run
            .workspace
            .as_ref()
            .filter(|_| self.uses_git(run))
            .and_then(|w| w.branch.as_deref())
        else {
            return Ok(Ok(count));
        };
        let git = self.git();
        let rev = if git.branch_exists(branch) { branch } else { "HEAD" };
        let Some(anchor) = git.last_trailer(AUDIT_TRAILER, &format!("passo({}):", run.id), rev) else {
            return Ok(Ok(count));
        };
        if log.read()?.iter().any(|e| e.hash == anchor) {
            return Ok(Ok(count));
        }
        Ok(Err(format!(
            "o último commit do run aponta para o evento {}…, que não está no log: o log foi reescrito ou encurtado depois do commit",
            &anchor[..anchor.len().min(8)]
        )))
    }

    // -- engine plumbing -------------------------------------------------------------------------------------

    fn valid<T>(&self, run: &Run, parsed: std::result::Result<T, Invalid>) -> Result<T> {
        parsed.map_err(|invalid| self.invalid(run, invalid))
    }

    /// Hides secrets in text Passo is about to keep, with the project's extra keys.
    pub fn redact(&self, text: &str) -> String {
        passo_core::redact::redact_with(text, &self.config.audit.secrets)
    }

    fn invalid(&self, run: &Run, invalid: Invalid) -> anyhow::Error {
        let invalid = Invalid(invalid.0.iter().map(|p| self.redact(p)).collect());
        let draft = EventDraft {
            kind: "output.invalid",
            actor: Actor::Agent,
            stage: run.stage,
            text: format!("Entrega recusada: {} problema(s)", invalid.0.len()),
            data: json!({ "problems": invalid.0 }),
        };
        self.store.log(&run.id).append(&run.id, &[draft], self.clock.now()).ok();
        Rejected(format!(
            "A entrega tem problemas:\n{invalid}\nCorrija só esses pontos e rode `passo done` de novo."
        ))
        .into()
    }

    fn transition(&self, run: &Run, command: Command, source: Option<Source>) -> Result<Run> {
        let now = self.clock.now();
        let (next, mut out) = match self.engine().apply(run, command, now) {
            Ok(result) => result,
            Err(FlowError::Invalid(invalid)) => return Err(self.invalid(run, invalid)),
            Err(e) => return Err(Rejected(e.to_string()).into()),
        };
        if let Some(source) = source {
            for event in out.events.iter_mut().filter(|e| e.actor == Actor::Dev) {
                event.data = json!({ "via": source.label() });
            }
        }
        self.persist(next, out, source)
    }

    /// Saves the run, appends the events and carries out the effects, feeding results back to the engine.
    fn persist(&self, mut run: Run, out: Outcome, _source: Option<Source>) -> Result<Run> {
        let log = self.store.log(&run.id);
        let mut queue: VecDeque<Effect> = out.effects.into();
        self.store.save(&run)?;
        log.append(&run.id, &out.events, self.clock.now())?;
        let mut report = false;
        while let Some(effect) = queue.pop_front() {
            let mut events = vec![];
            match effect {
                Effect::Commit { step, message } => {
                    if self.uses_git(&run) {
                        let git = self.git();
                        git.stage_all()?;
                        let hash = git.commit(&self.commit_message(&run, &message)?)?;
                        if let Some(hash) = &hash
                            && let Some(s) = run.steps.iter_mut().find(|s| s.id == step)
                        {
                            s.commit = Some(hash.clone());
                        }
                        events.push(draft(
                            &run,
                            "commit",
                            Actor::Passo,
                            match &hash {
                                Some(h) => format!("Commit {} · {message}", &h[..7.min(h.len())]),
                                None => format!("Sem mudanças para commitar · {message}"),
                            },
                        ));
                    }
                }
                Effect::RunFinalChecks => {
                    let mut commands = self.config.checks.final_.clone();
                    if let Some(def) = self.catalog.stage(Stage::Verificar) {
                        commands.extend(def.checks.iter().cloned());
                    }
                    let results = self.run_checks(&commands);
                    let (next, more) = self
                        .engine()
                        .apply(&run, Command::FinalChecks(results), self.clock.now())
                        .map_err(|e| anyhow!("{e}"))?;
                    run = next;
                    events.extend(more.events);
                    queue.extend(more.effects);
                }
                Effect::MeasureCoverage => {
                    let coverage = self.measure_coverage(&run);
                    let (next, more) = self
                        .engine()
                        .apply(&run, Command::Coverage(coverage), self.clock.now())
                        .map_err(|e| anyhow!("{e}"))?;
                    run = next;
                    events.extend(more.events);
                    queue.extend(more.effects);
                }
                Effect::WriteReport => report = true,
                Effect::Deliver => events.extend(self.deliver(&run)),
            }
            self.store.save(&run)?;
            log.append(&run.id, &events, self.clock.now())?;
        }
        if run.turn() == Turn::Agent && !run.stage.writes() {
            self.save_baseline(&run);
        }
        if report || !run.active() {
            self.write_report(&run);
        }
        Ok(run)
    }

    fn run_checks(&self, commands: &[String]) -> Vec<CheckResult> {
        let timeout = Duration::from_secs(self.config.checks.timeout_secs.max(1));
        // The tail of a check's output goes to the run, the feedback and the report: secrets out first.
        commands
            .iter()
            .map(|c| self.runner.run(c, &self.layout.root, timeout))
            .map(|mut check| {
                check.command = self.redact(&check.command);
                check.output = self.redact(&check.output);
                check
            })
            .collect()
    }

    /// Runs the coverage command and compares its lcov with the lines the run changed (tests and `.md` left out,
    /// as in the diff limit). Without git there is no base to compare with.
    fn measure_coverage(&self, run: &Run) -> passo_core::coverage::DiffCoverage {
        use passo_core::coverage::{self, DiffCoverage};
        let base = run.workspace.as_ref().and_then(|w| w.base.clone());
        let Some(base) = base.filter(|_| self.uses_git(run)) else {
            return DiffCoverage::skipped("sem git não há base para comparar");
        };
        let dir = std::env::temp_dir().join(format!("passo-cobertura-{}-{}", run.id, std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        if std::fs::create_dir_all(&dir).is_err() {
            return DiffCoverage::skipped("não consegui criar a pasta temporária");
        }
        let out = dir.join("lcov.info");
        let command = self
            .config
            .checks
            .coverage
            .command
            .replace("{outdir}", &dir.display().to_string())
            .replace("{out}", &out.display().to_string());
        let timeout = Duration::from_secs(self.config.checks.timeout_secs.max(1));
        let ran = self.runner.run(&command, &self.layout.root, timeout);
        let text = std::fs::read_to_string(&out).unwrap_or_default();
        std::fs::remove_dir_all(&dir).ok();
        if !ran.ok {
            let tail = ran.output.lines().last().unwrap_or("").to_string();
            return DiffCoverage::skipped(self.redact(&format!("o comando de cobertura falhou: {tail}")));
        }
        if text.trim().is_empty() {
            return DiffCoverage::skipped("o comando de cobertura não escreveu o lcov");
        }
        let root = std::fs::canonicalize(&self.layout.root).unwrap_or(self.layout.root.clone());
        let mut lcov = coverage::parse_lcov(&text, &root.display().to_string());
        if root != self.layout.root {
            let other = coverage::parse_lcov(&text, &self.layout.root.display().to_string());
            if other.instrumented.len() > lcov.instrumented.len() {
                lcov = other;
            }
        }
        let Ok(mut changed) = self.git().changed_lines(&base) else {
            return DiffCoverage::skipped("não consegui ler o diff do run");
        };
        let tests = globs(&self.config.tests.patterns).ok();
        changed.retain(|path, _| {
            !path.ends_with(".md") && !path.starts_with(".passo/") && !tests.as_ref().is_some_and(|t| t.is_match(path))
        });
        coverage::diff_coverage(&changed, &lcov.executed, &lcov.instrumented)
    }

    fn deliver(&self, run: &Run) -> Vec<EventDraft> {
        let map_paths = self.apply_map_checks(run);
        let Some(ws) = run.workspace.as_ref().filter(|_| self.uses_git(run)) else {
            return vec![draft(
                run,
                "delivered",
                Actor::Passo,
                "Entrega aceita (sem git: as mudanças já estão no lugar)".into(),
            )];
        };
        let git = self.git();
        let branch = ws.branch.clone().unwrap_or_default();
        let back = ws.from.clone().unwrap_or_else(|| "main".into());
        let mut events = vec![];
        // The log goes into the branch before it leaves: committed, it travels with the code. If that fails (the
        // project ignores .passo/, say), the delivery still goes on: the log is on disk and the trailers are in.
        let subject = if map_paths.is_empty() {
            "log de auditoria do run"
        } else {
            "log de auditoria e mapa"
        };
        let archived = self
            .store
            .archive(&run.id)
            .and_then(|dir| self.commit_message(run, subject).map(|m| (dir, m)))
            .and_then(|(dir, message)| {
                let mut paths = vec![dir.display().to_string()];
                paths.extend(map_paths.iter().cloned());
                let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
                git.commit_paths(&message, &paths)
            });
        if let Err(e) = archived {
            events.push(draft(
                run,
                "audit.unarchived",
                Actor::Passo,
                format!("O log de auditoria não foi commitado em {branch}: {e}"),
            ));
        }
        if let Err(e) = git.switch(&back) {
            events.push(draft(
                run,
                "delivery.failed",
                Actor::Passo,
                format!("Não consegui voltar para {back}: {e}"),
            ));
            return events;
        }
        events.push(match self.config.git.delivery {
            Delivery::Keep => draft(
                run,
                "delivered",
                Actor::Passo,
                format!("Branch {branch} pronto para pull request (gh pr create --head {branch})"),
            ),
            Delivery::Merge => match self
                .commit_message(run, &run.request)
                .and_then(|message| git.merge(&branch, &message))
            {
                Ok(()) => draft(run, "delivered", Actor::Passo, format!("{branch} juntado em {back}")),
                Err(e) => draft(
                    run,
                    "delivery.failed",
                    Actor::Passo,
                    format!(
                        "O merge de {branch} em {back} deu conflito; o branch ficou intacto para juntar à mão. ({e})"
                    ),
                ),
            },
        });
        events
    }

    fn write_report(&self, run: &Run) {
        let Some(reporter) = &self.reporter else { return };
        if let Ok(snapshot) = self.snapshot(Some(&run.id)) {
            reporter.write(&snapshot, &self.layout.report(&run.id)).ok();
        }
    }

    // -- facts -----------------------------------------------------------------------------------------------

    fn measure(&self, run: &Run, input: &DoneInput) -> Result<StepFacts> {
        let mut facts = StepFacts {
            agent_tier: self
                .catalog
                .agent_for(Stage::Construir)
                .map(|a| a.model.clone())
                .unwrap_or_default(),
            summary: input.text.trim().to_string(),
            empty_ok: input.empty_ok,
            note: input.note.clone(),
            ..Default::default()
        };
        if self.uses_git(run)
            && let Some(base) = run.workspace.as_ref().and_then(|w| w.base.clone())
        {
            let git = self.git();
            git.stage_all()?;
            let tests = globs(&self.config.tests.patterns)?;
            let sensitive = globs(&self.config.limits.sensitive)?;
            // The archived log is Passo's own; everything else under .passo/ is project config and counts.
            let own = |p: &str| p.starts_with(".passo/audit/");
            facts.files = git.staged_files()?.into_iter().filter(|f| !own(f)).collect();
            let numstat: Vec<(String, usize)> = git.numstat(&base)?.into_iter().filter(|(p, _)| !own(p)).collect();
            let counted: Vec<&(String, usize)> = numstat
                .iter()
                .filter(|(p, _)| !tests.is_match(p) && !p.ends_with(".md"))
                .collect();
            facts.files_total = counted.len();
            facts.lines_total = counted.iter().map(|(_, n)| n).sum();
            facts.tests_touched = facts.files.iter().any(|f| tests.is_match(f));
            if let Some(step) = run.current_step().filter(|s| !s.files.is_empty()) {
                facts.unplanned = facts
                    .files
                    .iter()
                    .filter(|f| !tests.is_match(f) && !step.files.iter().any(|p| f.ends_with(p.as_str())))
                    .cloned()
                    .collect();
            }
            facts.sensitive = numstat
                .iter()
                .filter(|(p, _)| sensitive.is_match(p))
                .map(|(p, _)| p.clone())
                .collect();
            facts.measured = true;
        }
        // The step's own test runs first: the dev approved the command with the plan.
        let mut commands: Vec<String> = run
            .current_step()
            .map(|s| s.test_command.clone())
            .filter(|c| !c.is_empty())
            .into_iter()
            .collect();
        commands.extend(self.config.checks.step.iter().cloned());
        if let Some(def) = self.catalog.stage(Stage::Construir) {
            commands.extend(def.checks.iter().cloned());
        }
        facts.checks = self.run_checks(&commands);
        Ok(facts)
    }

    /// Every criterion the review calls met must cite something Passo can find: a file that exists (and the line,
    /// when one is cited), or a test name in a test file or in a file the run touched. Cheap models invent evidence.
    fn check_evidence(&self, run: &Run, review: &spec::Review) -> std::result::Result<(), Invalid> {
        let root = std::fs::canonicalize(&self.layout.root).unwrap_or(self.layout.root.clone());
        let mut errors = vec![];
        // Each searchable file is read once per hand-in, whatever the number of criteria.
        let mut searchable: Option<Vec<String>> = None;
        for (i, item) in review.criteria.iter().enumerate().filter(|(_, item)| item.ok) {
            let field = format!("`criterios[{i}].evidencia`");
            let refs = spec::evidence_refs(&item.evidence);
            if refs.is_empty() {
                errors.push(format!(
                    "{field} não cita arquivo, linha nem teste; aponte onde está a prova (arquivo:linha ou `nome_do_teste`)"
                ));
                continue;
            }
            let mut confirmed = false;
            // A wrong reference next to a right one still refuses: cited evidence has to be exact.
            let mut problems = vec![];
            for r in &refs {
                if let Some(path) = &r.path {
                    match project_file(&root, path) {
                        Located::Outside => problems.push(format!("{field} cita {path}, que está fora do projeto")),
                        Located::Folder => {
                            problems.push(format!("{field} cita {path}, que é uma pasta: aponte o arquivo"))
                        }
                        Located::Missing => {}
                        Located::File(file) => {
                            let lines = std::fs::read_to_string(&file).map(|t| t.lines().count()).unwrap_or(0);
                            match r.line {
                                Some(n) if n == 0 || n as usize > lines => {
                                    problems.push(format!("{field} cita {path}:{n}, mas o arquivo tem {lines} linhas"))
                                }
                                _ => confirmed = true,
                            }
                        }
                    }
                } else if let Some(symbol) = &r.symbol {
                    let texts = searchable.get_or_insert_with(|| {
                        self.searchable_files(run)
                            .iter()
                            .filter_map(|f| std::fs::read_to_string(f).ok())
                            .collect()
                    });
                    if texts.iter().any(|t| t.contains(symbol.as_str())) {
                        confirmed = true;
                    }
                }
            }
            if !problems.is_empty() {
                errors.extend(problems);
            } else if !confirmed {
                let cited: Vec<String> = refs
                    .iter()
                    .map(|r| match (&r.path, r.line, &r.symbol) {
                        (Some(p), Some(n), _) => format!("{p}:{n}"),
                        (Some(p), None, _) => p.clone(),
                        (None, _, Some(s)) => format!("`{s}`"),
                        _ => String::new(),
                    })
                    .collect();
                errors.push(format!(
                    "{field} cita {}, mas nada disso existe no projeto; aponte um arquivo, uma linha ou um teste que existam",
                    cited.join(", ")
                ));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(Invalid(errors))
        }
    }

    /// Where a cited test name may live: test files, the files the plan's steps name, and (with git) the files
    /// this run changed. The same folders are left out as anywhere else in the project.
    fn searchable_files(&self, run: &Run) -> Vec<PathBuf> {
        let root = &self.layout.root;
        let git = self.uses_git(run).then(|| self.git());
        let tests = globs(&self.config.tests.patterns).ok();
        let mut files: Vec<String> = crate::infra::project_files(root, git.as_ref())
            .into_iter()
            .filter(|f| tests.as_ref().is_some_and(|t| t.is_match(f)))
            .collect();
        files.extend(run.steps.iter().flat_map(|s| s.files.iter().cloned()));
        if let (Some(git), Some(base)) = (&git, run.workspace.as_ref().and_then(|w| w.base.as_deref())) {
            files.extend(git.numstat(base).unwrap_or_default().into_iter().map(|(path, _)| path));
        }
        files.retain(|f| !crate::infra::ignored_path(f));
        files.sort();
        files.dedup();
        files.into_iter().map(|f| root.join(f)).collect()
    }

    fn baseline_path(&self, run: &Run) -> PathBuf {
        self.layout.run_dir(&run.id).join("baseline.json")
    }

    fn save_baseline(&self, run: &Run) {
        if !self.uses_git(run) {
            return;
        }
        if let Ok(dirty) = self.git().dirty() {
            std::fs::write(
                self.baseline_path(run),
                serde_json::to_string(&dirty).unwrap_or_default(),
            )
            .ok();
        }
    }

    /// A read-only stage that changed files broke the lock (a shell redirect the hook could not see).
    fn check_read_only(&self, run: &Run) -> Result<()> {
        if !self.uses_git(run) {
            return Ok(());
        }
        let before: Vec<String> = std::fs::read_to_string(self.baseline_path(run))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let changed: Vec<String> = self
            .git()
            .dirty()?
            .into_iter()
            .filter(|f| !before.contains(f))
            .collect();
        if changed.is_empty() {
            return Ok(());
        }
        let text = format!(
            "{} é só leitura, mas estes arquivos mudaram: {}",
            run.stage.label(),
            changed.join(", ")
        );
        self.store.log(&run.id).append(
            &run.id,
            &[draft(run, "lock.breach", Actor::Agent, text.clone())],
            self.clock.now(),
        )?;
        Err(Rejected(format!(
            "{text}.\nDesfaça essas mudanças (rascunhos vão em `mktemp -d`) e entregue de novo."
        ))
        .into())
    }

    // -- views -----------------------------------------------------------------------------------------------

    /// Token usage from the run's transcripts, oldest first. Each file is read again only when it changed since
    /// the last time (`runs/<id>/usage.json` keeps its length, its modification time and what it held).
    pub fn usage(&self, run: &Run) -> Vec<Usage> {
        let transcripts = self.store.transcripts(&run.id);
        let path = self.layout.usage(&run.id);
        let cached: Vec<UsageFile> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let mut files: Vec<UsageFile> = vec![];
        for platform in passo_adapters::registry() {
            let paths: Vec<PathBuf> = transcripts
                .iter()
                .filter(|t| t.platform == platform.id())
                .map(|t| PathBuf::from(&t.path))
                .collect();
            if paths.is_empty() {
                continue;
            }
            for file in platform.usage_files(&paths) {
                let Ok(meta) = std::fs::metadata(&file) else { continue };
                let len = meta.len();
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0);
                let key = file.display().to_string();
                let samples = match cached
                    .iter()
                    .find(|c| c.path == key && c.len == len && c.mtime == mtime)
                {
                    Some(hit) => hit.samples.clone(),
                    None => platform.usage_in(&file),
                };
                files.push(UsageFile {
                    path: key,
                    len,
                    mtime,
                    samples,
                });
            }
        }
        if files != cached
            && let Ok(text) = serde_json::to_string(&files)
        {
            passo_store::layout::write_atomic(&path, text.as_bytes()).ok();
        }
        let mut out: Vec<Usage> = files.into_iter().flat_map(|f| f.samples).collect();
        out.sort_by_key(|u| u.t);
        out
    }

    /// Billable tokens (input + output + cache writes) the run spent since it began.
    pub fn run_tokens(&self, run: &Run) -> u64 {
        self.usage(run)
            .iter()
            .filter(|u| u.t >= run.created_at)
            .map(|u| u.input + u.output + u.cache_write)
            .sum()
    }

    /// Measures the active run's tokens and hands the total to the engine when it matters: it changed, or it is
    /// past the limit with no gate open (a gate that was open when the limit passed has since closed).
    pub fn spend(&self, run: Run) -> Result<Run> {
        if !run.active() {
            return Ok(run);
        }
        let tokens = self.run_tokens(&run);
        let max = self.config.profile(run.size).max_tokens;
        let pending = max > 0 && tokens > max && !run.tokens_accepted && run.gate.is_none();
        if tokens == run.tokens && !pending {
            return Ok(run);
        }
        self.transition(&run, Command::Spent(tokens), None)
    }

    pub fn metrics(&self, run: &Run) -> Result<metrics::Metrics> {
        let events = self.store.read_log(&run.id).read()?;
        Ok(metrics::compute(run, &events, &self.usage(run), self.clock.now()))
    }

    /// The series across runs, for `passo stats --all` and the dashboard: runs created on or after `since`.
    pub fn stats_all(&self, since: Option<chrono::DateTime<chrono::Utc>>) -> Result<metrics::Aggregate> {
        let mut runs = vec![];
        for run in self.store.list() {
            if since.is_some_and(|since| run.created_at < since) {
                continue;
            }
            let metrics = self.metrics(&run)?;
            runs.push((RunSummary::from(&run), metrics));
        }
        Ok(metrics::aggregate(&runs))
    }

    pub fn snapshot(&self, id: Option<&str>) -> Result<Snapshot> {
        let run = match id {
            Some(id) => Some(self.store.load(id)?),
            None => self.store.latest(),
        };
        let size = run.as_ref().map(|r| r.size).unwrap_or(self.config.default_size);
        let profile = self.config.profile(size);
        let stages = Stage::ALL
            .iter()
            .map(|stage| {
                let agent = self.catalog.agent_for(*stage);
                StageInfo {
                    id: stage.id(),
                    label: stage.label(),
                    mode: profile.mode(*stage),
                    agent: agent.map(|a| a.name.clone()),
                    tier: agent.map(|a| a.model.clone()),
                }
            })
            .collect();
        let (events, metrics, audit) = match &run {
            Some(run) => {
                let events = self.store.read_log(&run.id).read()?;
                let metrics = metrics::compute(run, &events, &self.usage(run), self.clock.now());
                let audit = self.verify_audit(run)?;
                (events, Some(metrics), Some(audit))
            }
            None => (vec![], None, None),
        };
        let map_nodes: Vec<String> = match run.as_ref().filter(|r| r.turn() == Turn::Agent) {
            Some(r) => {
                let files = self.map_files(r);
                self.map_selection(r, &files)
                    .map(|s| {
                        s.nodes
                            .iter()
                            .filter(|n| n.id != "projeto")
                            .map(|n| n.id.clone())
                            .collect()
                    })
                    .unwrap_or_default()
            }
            None => vec![],
        };
        let hook_warning = run.as_ref().filter(|r| r.active()).and_then(|r| self.hook_warning(r));
        let snapshot = Snapshot {
            generated_at: self.clock.now(),
            project: self
                .layout
                .root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("projeto")
                .to_string(),
            rail: run.as_ref().map(|r| render::rail(r, &self.config)),
            gate_text: run.as_ref().filter(|r| r.gate.is_some()).map(|r| self.gate_text(r)),
            next_text: run.as_ref().map(|r| self.next_text(r)),
            runs: self.store.list().iter().map(RunSummary::from).collect(),
            run,
            events,
            metrics,
            stages,
            audit,
            platforms: self.platforms().iter().map(|p| p.label().to_string()).collect(),
            checks_step: self.config.checks.step.clone(),
            checks_final: self.config.checks.final_.clone(),
            map_nodes,
            hook_warning,
            max_tokens: self.config.profile(size).max_tokens,
            series: self.stats_all(None).ok(),
            learn: match passo_store::learn::load_task(&self.layout) {
                Some(task) if task.delivered() => Some(self.learn_text(&task)),
                Some(_) => Some("O agente está aprendendo com os runs (passo learn).".into()),
                None => self.learn_hint(),
            },
        };
        // Belt and braces: what the report and the dashboard show is redacted again, so an old log comes out clean.
        Ok(snapshot.redacted(|text| self.redact(text)))
    }
}

/// The commit trailer that names the last audit event: `Passo-Audit: <sha256>`.
pub const AUDIT_TRAILER: &str = "Passo-Audit";

/// Each platform knows its own model names; the config only has to say what differs from them.
fn fill_platform_models(config: &mut Config) {
    for platform in passo_adapters::registry() {
        for tier in passo_core::config::TIERS {
            if let Some(model) = platform.default_model(tier) {
                config
                    .models
                    .entry(tier.to_string())
                    .or_default()
                    .entry(platform.id().to_string())
                    .or_insert_with(|| model.to_string());
            }
        }
    }
}

/// Attached projects as `(name, root)`, their paths resolved against this project's root (and canonical when they
/// exist, so they compare with the paths hooks report).
pub(crate) fn attached_roots(root: &Path, config: &Config) -> Vec<(String, PathBuf)> {
    config
        .projects
        .iter()
        .map(|p| {
            let path = root.join(&p.path);
            (p.name.clone(), std::fs::canonicalize(&path).unwrap_or(path))
        })
        .collect()
}

fn draft(run: &Run, kind: &'static str, actor: Actor, text: String) -> EventDraft {
    EventDraft {
        kind,
        actor,
        stage: run.stage,
        text,
        data: json!({}),
    }
}

pub(crate) fn globs(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).with_context(|| format!("padrão inválido: {pattern}"))?);
    }
    Ok(builder.build()?)
}

/// Where an evidence path points, as far as the project is concerned.
enum Located {
    File(PathBuf),
    Folder,
    /// Absolute, climbing out with `..`, escaping by a symlink, or inside a folder the project never counts
    /// (`.git`, `.passo`, `target`…).
    Outside,
    /// Not on disk: maybe not a path at all.
    Missing,
}

fn project_file(root: &Path, path: &str) -> Located {
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        || crate::infra::ignored_path(path)
    {
        return Located::Outside;
    }
    let joined = root.join(relative);
    let Ok(real) = std::fs::canonicalize(&joined) else {
        return Located::Missing;
    };
    if !real.starts_with(root) {
        return Located::Outside;
    }
    if real.is_dir() {
        Located::Folder
    } else {
        Located::File(real)
    }
}

/// A file path, `-` for stdin, or the content itself.
pub(crate) fn read_input(text: &str) -> Result<String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(
            Rejected("entregue um arquivo: passo done <arquivo.json> (ou - para ler da entrada)".into()).into(),
        );
    }
    if text == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
        return Ok(buf);
    }
    let path = Path::new(text);
    if !text.contains('{') && path.is_file() {
        return std::fs::read_to_string(path).with_context(|| format!("lendo {text}"));
    }
    if !text.contains('{') {
        return Err(Rejected(format!("arquivo não encontrado: {text}")).into());
    }
    Ok(text.to_string())
}

/// `prioridade-tarefas-3f2a`: readable in branch names, unique enough per project.
pub fn make_id(request: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    const STOP: [&str; 14] = [
        "a", "o", "as", "os", "de", "do", "da", "dos", "das", "e", "em", "um", "uma", "para",
    ];
    let words: Vec<String> = request
        .split(|c: char| !c.is_alphanumeric())
        .map(fold)
        .filter(|w| w.len() > 1 && !STOP.contains(&w.as_str()))
        .take(3)
        .collect();
    let slug = if words.is_empty() {
        "run".to_string()
    } else {
        words.join("-")
    };
    let nanos = now.timestamp_nanos_opt().unwrap_or(0) as u64;
    format!("{slug}-{:04x}", (nanos ^ (nanos >> 17)) & 0xffff)
}

fn fold(word: &str) -> String {
    word.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c => c,
        })
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// One transcript file as last read, for `App::usage`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct UsageFile {
    path: String,
    len: u64,
    mtime: u64,
    samples: Vec<Usage>,
}
