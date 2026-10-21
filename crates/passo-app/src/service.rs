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
        Ok(App {
            store: Store::new(layout.clone()),
            layout,
            config,
            layers: loaded.layers,
            catalog,
            bin: "passo".into(),
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

    fn git(&self) -> Git {
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
        Ok(self.next_text(&run))
    }

    /// Lists the repository tree for the run (on start, and again when the dev changes the request).
    fn write_tree(&self, run: &Run) {
        let git = self.uses_git(run).then(|| self.git());
        let tree = crate::tree::build(&self.layout.root, git.as_ref(), &self.config, 60);
        passo_store::layout::write_atomic(&self.layout.tree(&run.id), tree.as_bytes()).ok();
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
        match self.store.latest() {
            Some(run) => Ok(self.next_text(&run)),
            None => Ok("Nenhum run ainda. Comece com: passo start \"pedido do dev\" --size m".into()),
        }
    }

    pub fn next_text(&self, run: &Run) -> String {
        let platforms = self.platforms();
        let delegation = |agent: &str| match platforms.as_slice() {
            [one] => one.delegation(agent),
            _ => format!(
                "Se a sua plataforma roda subagentes, use `passo-{agent}` e passe a ele esta saída inteira; senão, faça você mesmo, seguindo o papel abaixo."
            ),
        };
        let tree = std::fs::read_to_string(self.layout.tree(&run.id)).ok();
        render::next(&View {
            run,
            config: &self.config,
            catalog: &self.catalog,
            delegation: &delegation,
            bin: &self.bin,
            tree: tree.as_deref(),
        })
    }

    pub fn done(&self, input: DoneInput) -> Result<String> {
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
                let review = self.valid(&run, spec::parse_review(&text, &ids))?;
                self.valid(&run, self.check_evidence(&run, &review))?;
                Command::SubmitReview(review)
            }
            stage => bail!("nada a entregar em {}; rode `passo next`", stage.label()),
        };
        let run = self.transition(&run, command, None)?;
        Ok(self.next_text(&run))
    }

    pub fn ask(&self, question: &str, options: &[String]) -> Result<String> {
        let run = self.active()?;
        let question = self.valid(&run, spec::question_from_cli(String::new(), question, options))?;
        let run = self.transition(&run, Command::Ask(question), None)?;
        Ok(self.next_text(&run))
    }

    pub fn decide(&self, action: DevAction, source: Source) -> Result<Run> {
        let run = self.active()?;
        self.ensure_run_branch(&run)?;
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
        let Some(run) = self.store.latest() else {
            return "Nenhum run ainda. No agente, peça algo \"com o passo\", ou rode: passo start \"pedido\"".into();
        };
        let mut out = format!("{}\n{}\n", run.headline(), render::rail(&run, &self.config));
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("Pedido: {}\n", run.request));
        match run.turn() {
            Turn::Dev => {
                out.push('\n');
                out.push_str(&self.gate_text(&run));
                out.push_str("\nNo terminal: passo ok · passo ok 1b · passo no \"o que mudar\"\n");
            }
            Turn::Agent => out.push_str("\nO agente está trabalhando. Acompanhe com `passo ui`.\n"),
            Turn::None => {}
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
        let log = self.store.log(&run.id);
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

    fn invalid(&self, run: &Run, invalid: Invalid) -> anyhow::Error {
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
        commands
            .iter()
            .map(|c| self.runner.run(c, &self.layout.root, timeout))
            .collect()
    }

    fn deliver(&self, run: &Run) -> Vec<EventDraft> {
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
        let archived = self
            .store
            .archive(&run.id)
            .and_then(|dir| self.commit_message(run, "log de auditoria do run").map(|m| (dir, m)))
            .and_then(|(dir, message)| git.commit_paths(&message, &[&dir.display().to_string()]));
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
        let root = &self.layout.root;
        let mut errors = vec![];
        let mut searchable: Option<Vec<PathBuf>> = None;
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
            let mut wrong_line = None;
            for r in &refs {
                if let Some(path) = &r.path {
                    let file = root.join(path);
                    if file.is_dir() && r.line.is_none() {
                        confirmed = true;
                    } else if file.is_file() {
                        let lines = std::fs::read_to_string(&file).map(|t| t.lines().count()).unwrap_or(0);
                        match r.line {
                            Some(n) if n == 0 || n as usize > lines => {
                                wrong_line.get_or_insert(format!(
                                    "{field} cita {path}:{n}, mas o arquivo tem {lines} linha(s)"
                                ));
                            }
                            _ => confirmed = true,
                        }
                    }
                } else if let Some(symbol) = &r.symbol {
                    let files = searchable.get_or_insert_with(|| self.searchable_files(run));
                    if files
                        .iter()
                        .any(|f| std::fs::read_to_string(f).is_ok_and(|t| t.contains(symbol.as_str())))
                    {
                        confirmed = true;
                    }
                }
            }
            // A cited line that does not exist is invented, even next to a real file.
            if let Some(wrong) = wrong_line {
                errors.push(wrong);
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

    /// Where a cited test name may live: test files, and the files this run changed.
    fn searchable_files(&self, run: &Run) -> Vec<PathBuf> {
        let root = &self.layout.root;
        let git = self.uses_git(run).then(|| self.git());
        let tests = globs(&self.config.tests.patterns).ok();
        let mut files: Vec<String> = crate::infra::project_files(root, git.as_ref())
            .into_iter()
            .filter(|f| tests.as_ref().is_some_and(|t| t.is_match(f)))
            .collect();
        if let (Some(git), Some(base)) = (&git, run.workspace.as_ref().and_then(|w| w.base.as_deref())) {
            files.extend(git.numstat(base).unwrap_or_default().into_iter().map(|(path, _)| path));
        }
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

    pub fn usage(&self, run: &Run) -> Vec<Usage> {
        let transcripts = self.store.transcripts(&run.id);
        let mut out = vec![];
        for platform in passo_adapters::registry() {
            let paths: Vec<PathBuf> = transcripts
                .iter()
                .filter(|t| t.platform == platform.id())
                .map(|t| PathBuf::from(&t.path))
                .collect();
            if !paths.is_empty() {
                out.extend(platform.usage(&paths));
            }
        }
        out
    }

    pub fn metrics(&self, run: &Run) -> Result<metrics::Metrics> {
        let events = self.store.log(&run.id).read()?;
        Ok(metrics::compute(run, &events, &self.usage(run), self.clock.now()))
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
                let log = self.store.log(&run.id);
                let events = log.read()?;
                let metrics = metrics::compute(run, &events, &self.usage(run), self.clock.now());
                let audit = self.verify_audit(run)?;
                (events, Some(metrics), Some(audit))
            }
            None => (vec![], None, None),
        };
        Ok(Snapshot {
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
        })
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

fn draft(run: &Run, kind: &'static str, actor: Actor, text: String) -> EventDraft {
    EventDraft {
        kind,
        actor,
        stage: run.stage,
        text,
        data: json!({}),
    }
}

fn globs(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).with_context(|| format!("padrão inválido: {pattern}"))?);
    }
    Ok(builder.build()?)
}

/// A file path, `-` for stdin, or the content itself.
fn read_input(text: &str) -> Result<String> {
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
