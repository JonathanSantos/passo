//! `passo map`: an agent explores the project and hands in its map; Passo checks every field against the disk and
//! writes `.passo/map/`. Not one of the five stages: read-only, no branch, no step commits, no delivery to accept.

use std::path::Path;

use anyhow::{Result, bail};
use passo_core::spec::{self, Invalid, MAP_TEMPLATE};
use passo_core::{Actor, EventDraft, Stage};
use passo_store::EventLog;
use passo_store::map::{self as store_map, MapTask};
use serde_json::json;

use crate::infra::project_files;
use crate::service::{App, DoneInput, Rejected, globs, read_input};

/// The mapping log names its events after this pseudo-run.
const LOG_RUN: &str = "mapa";

impl App {
    /// Whether a mapping is in progress.
    pub fn mapping(&self) -> bool {
        store_map::load_task(&self.layout).is_some()
    }

    fn map_log(&self, kind: &'static str, actor: Actor, text: String, data: serde_json::Value) {
        // Mapping has no stage of its own; it reads like refining does.
        let draft = EventDraft {
            kind,
            actor,
            stage: Stage::Refinar,
            text,
            data,
        };
        EventLog::new(self.layout.map_log())
            .append(LOG_RUN, &[draft], self.now())
            .ok();
    }

    /// Logs a tool call or a lock denial that happened while mapping.
    pub fn map_log_event(&self, kind: &'static str, text: String, data: serde_json::Value) {
        self.map_log(kind, Actor::Agent, text, data);
    }

    pub fn map_start(&self) -> Result<String> {
        if let Some(run) = self.store.active() {
            return Err(Rejected(format!(
                "termine ou interrompa o run antes de mapear ({})",
                run.headline()
            ))
            .into());
        }
        self.store.init()?;
        if self.mapping() {
            return Ok(self.map_text());
        }
        let git = self.git();
        let repo = git.is_repo();
        let task = MapTask {
            started_at: self.now(),
            commit: if repo { git.head() } else { None },
            projects: std::iter::once(self.layout.root.display().to_string())
                .chain(
                    crate::service::attached_roots(&self.layout.root, &self.config)
                        .into_iter()
                        .map(|(_, root)| root.display().to_string()),
                )
                .collect(),
            baseline: if repo { git.dirty().unwrap_or_default() } else { vec![] },
            stops: 0,
        };
        store_map::save_task(&self.layout, &task)?;
        self.map_log(
            "map.started",
            Actor::Agent,
            "Mapeamento começou".into(),
            json!({ "commit": task.commit }),
        );
        Ok(self.map_text())
    }

    /// The explorer's task: role, the tree, the current map (to update rather than redo), the format, how to hand in.
    pub fn map_text(&self) -> String {
        let git = self.git();
        let tree = crate::tree::build(
            &self.layout.root,
            git.is_repo().then_some(&git),
            &self.config,
            usize::MAX,
        );
        let current = store_map::load(&self.layout).ok().flatten();
        let changed: Vec<String> = current
            .as_ref()
            .and_then(|m| self.map_staleness(m))
            .map(|s| s.areas.into_iter().filter(|(_, c)| *c).map(|(id, _)| id).collect())
            .unwrap_or_default();
        let attached: Vec<(String, String)> = crate::service::attached_roots(&self.layout.root, &self.config)
            .into_iter()
            .filter(|(_, root)| root.is_dir())
            .map(|(name, root)| {
                let git = crate::infra::Git::new(&root);
                let tree = crate::tree::build(&root, git.is_repo().then_some(&git), &self.config, usize::MAX);
                (name, tree)
            })
            .collect();
        let agent = self.catalog.agents.get("explorador");
        crate::render::map_task(&crate::render::MapView {
            agent,
            delegation: &self.delegation("explorador"),
            tree: &tree,
            current: current.as_ref(),
            changed: &changed,
            attached: &attached,
            template: MAP_TEMPLATE,
            bin: &self.bin,
        })
    }

    pub fn map_done(&self, input: DoneInput) -> Result<String> {
        let Some(task) = store_map::load_task(&self.layout) else {
            bail!("não há mapeamento em andamento; comece com passo map");
        };
        let git = self.git();
        let repo = git.is_repo();
        // The explorer has a terminal: an install rewrites a tracked lockfile without the hook seeing it.
        if repo {
            let changed: Vec<String> = git
                .dirty()
                .unwrap_or_default()
                .into_iter()
                .filter(|f| !task.baseline.contains(f))
                .collect();
            if !changed.is_empty() {
                let text = format!(
                    "O mapeamento é só leitura, mas estes arquivos mudaram: {}",
                    changed.join(", ")
                );
                self.map_log("lock.breach", Actor::Agent, text.clone(), json!({ "files": changed }));
                return Err(Rejected(format!(
                    "{text}.\nDesfaça essas mudanças (nada de instalar ou gerar arquivos) e entregue de novo."
                ))
                .into());
            }
        }

        let text = read_input(&input.text)?;
        let root = &self.layout.root;
        let draft = spec::parse_map(&text, &manifest_libs(root)).map_err(|invalid| self.map_invalid(invalid))?;

        let tracked = project_files(root, repo.then_some(&git));
        let mut errors = vec![];
        for (i, area) in draft.areas.iter().enumerate() {
            for (j, pattern) in area.paths.iter().enumerate() {
                match globs(std::slice::from_ref(pattern)) {
                    Ok(set) if tracked.iter().any(|f| set.is_match(f)) => {}
                    Ok(_) => errors.push(format!(
                        "`areas[{i}].caminhos[{j}]` não casa nenhum arquivo rastreado: {pattern}"
                    )),
                    Err(_) => errors.push(format!("`areas[{i}].caminhos[{j}]` não é um glob válido: {pattern}")),
                }
            }
        }
        let missing = |path: &str| !root.join(path).exists();
        for (j, source) in draft.project.sources.iter().enumerate().filter(|(_, s)| missing(s)) {
            errors.push(format!("`projeto.fontes[{j}]` não existe: {source}"));
        }
        for (i, area) in draft.areas.iter().enumerate() {
            for (j, source) in area.sources.iter().enumerate().filter(|(_, s)| missing(s)) {
                errors.push(format!("`areas[{i}].fontes[{j}]` não existe: {source}"));
            }
        }
        for (i, lib) in draft.libs.iter().enumerate() {
            for (j, source) in lib.sources.iter().enumerate().filter(|(_, s)| missing(s)) {
                errors.push(format!("`libs[{i}].fontes[{j}]` não existe: {source}"));
            }
        }
        if !errors.is_empty() {
            return Err(self.map_invalid(Invalid(errors)));
        }

        let mut map = draft.into_map(if repo { git.head() } else { None });
        // A remap keeps what the reviewer and `passo learn` wrote in each area.
        let removed = match store_map::load(&self.layout) {
            Ok(Some(old)) => store_map::carry_notes(&old, &mut map),
            _ => vec![],
        };
        let (areas, libs) = (map.areas.len(), map.libs.len());
        store_map::save(&self.layout, &map)?;
        store_map::remove_task(&self.layout);
        self.map_log(
            "map.saved",
            Actor::Agent,
            format!("Mapa salvo: {areas} áreas, {libs} libs"),
            json!({ "areas": areas, "libs": libs }),
        );
        let removed = if removed.is_empty() {
            String::new()
        } else {
            format!("\nÁreas removidas (e as anotações delas): {}.", removed.join(", "))
        };
        Ok(format!(
            "Mapa salvo: {areas} áreas, {libs} libs. Commite .passo/map/.{removed}"
        ))
    }

    fn map_invalid(&self, invalid: Invalid) -> anyhow::Error {
        self.map_log(
            "map.invalid",
            Actor::Agent,
            format!("Mapa recusado: {} problema(s)", invalid.0.len()),
            json!({ "problems": invalid.0 }),
        );
        Rejected(format!(
            "O mapa tem problemas:\n{invalid}\nCorrija só esses pontos e rode `passo done` de novo."
        ))
        .into()
    }

    /// The dev gives up on a mapping in progress.
    pub fn map_cancel(&self) -> Result<String> {
        if !self.mapping() {
            bail!("não há mapeamento em andamento");
        }
        store_map::remove_task(&self.layout);
        self.map_log(
            "map.cancelled",
            Actor::Dev,
            "Mapeamento cancelado pelo dev".into(),
            json!({}),
        );
        Ok("Mapeamento cancelado.".into())
    }

    /// Counts a stop the hook blocked during the mapping; the hook gives up after a few.
    pub fn map_count_stop(&self) -> u32 {
        let Some(mut task) = store_map::load_task(&self.layout) else {
            return u32::MAX;
        };
        task.stops += 1;
        store_map::save_task(&self.layout, &task).ok();
        task.stops
    }
}

/// The files whose change means the project itself changed (the same ones `passo init` reads the stack from).
const MANIFESTS: [&str; 5] = [
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "requirements.txt",
    "go.mod",
];
/// Anotações kept per area; the oldest leave first.
const MAX_NOTES: usize = 5;

/// How old the map is, by git: which areas changed since they were written, and whether the project did.
#[derive(Debug, Clone, Default)]
pub struct Staleness {
    pub commit: Option<String>,
    pub days: Option<i64>,
    /// `(area id, changed since its commit)`; areas without a commit are left out.
    pub areas: Vec<(String, bool)>,
    pub manifests: bool,
    /// Top-level folders that appeared since the map, outside every area.
    pub new_dirs: Vec<String>,
}

impl Staleness {
    pub fn changed(&self) -> usize {
        self.areas.iter().filter(|(_, c)| *c).count()
    }

    /// The manifests changed, or more than a third of the areas did.
    pub fn worth_remapping(&self) -> bool {
        self.manifests || !self.new_dirs.is_empty() || (self.changed() * 3 > self.areas.len() && self.changed() > 0)
    }
}

impl App {
    /// How old the map is, by git. `None` without git.
    pub fn map_staleness(&self, map: &passo_core::map::Map) -> Option<Staleness> {
        let git = self.git();
        if !git.is_repo() {
            return None;
        }
        let mut out = Staleness::default();
        for area in map.areas.iter().filter(|a| !a.attached()) {
            let Some(commit) = &area.commit else { continue };
            let changed = git
                .changed_between(commit, &area.paths)
                .map(|f| !f.is_empty())
                .unwrap_or(true);
            out.areas.push((area.id.clone(), changed));
        }
        out.commit = map.project.iter().chain(&map.areas).find_map(|n| n.commit.clone());
        if let Some(commit) = &out.commit {
            out.days = git
                .commit_time(commit)
                .map(|t| (self.now().timestamp() - t).max(0) / 86_400);
            let manifests: Vec<String> = MANIFESTS.iter().map(|m| m.to_string()).collect();
            out.manifests = git
                .changed_between(commit, &manifests)
                .map(|f| !f.is_empty())
                .unwrap_or(false);
            if let Ok(before) = git.top_level(commit) {
                let covered = |dir: &str| {
                    map.areas
                        .iter()
                        .flat_map(|a| &a.paths)
                        .any(|p| p.split('/').next() == Some(dir))
                };
                let mut now: Vec<String> = project_files(&self.layout.root, Some(&git))
                    .into_iter()
                    .filter_map(|f| f.split_once('/').map(|(d, _)| d.to_string()))
                    .collect();
                now.sort();
                now.dedup();
                out.new_dirs = now.into_iter().filter(|d| !before.contains(d) && !covered(d)).collect();
            }
        }
        Some(out)
    }

    /// The line `start` opens with when the map has aged; also leaves `map.stale` in the run's log.
    pub(crate) fn map_stale_line(&self, run: &passo_core::Run) -> Option<String> {
        let map = self.map.as_ref().ok()?.as_ref()?;
        let stale = self.map_staleness(map).filter(Staleness::worth_remapping)?;
        let made = match (&stale.commit, stale.days) {
            (Some(c), Some(d)) => format!(" feito em {} ({d} dias)", &c[..c.len().min(7)]),
            (Some(c), None) => format!(" feito em {}", &c[..c.len().min(7)]),
            _ => String::new(),
        };
        let mut what = vec![];
        if stale.manifests {
            what.push("os manifestos mudaram".to_string());
        }
        if !stale.new_dirs.is_empty() {
            what.push(format!(
                "apareceram pastas fora do mapa ({})",
                stale.new_dirs.join(", ")
            ));
        }
        what.push(format!("{} de {} áreas mudaram", stale.changed(), stale.areas.len()));
        let line = format!(
            "Mapa do projeto{made}: {} desde então. Siga assim, ou peça ao agente para rodar passo map antes.",
            what.join("; ")
        );
        let draft = EventDraft {
            kind: "map.stale",
            actor: Actor::Passo,
            stage: run.stage,
            text: line.clone(),
            data: json!({ "changed": stale.changed(), "areas": stale.areas.len(), "manifests": stale.manifests }),
        };
        self.store.log(&run.id).append(&run.id, &[draft], self.now()).ok();
        Some(line)
    }

    /// Applies the reviewer's word to the map on delivery: confirmed areas get the run's last commit; an area it
    /// said is off keeps its commit and gets the note under `## Anotações`. Returns the touched paths, for git.
    pub(crate) fn apply_map_checks(&self, run: &passo_core::Run) -> Vec<String> {
        if run.map_checks.is_empty() {
            return vec![];
        }
        let Ok(Some(mut map)) = store_map::load(&self.layout) else {
            return vec![];
        };
        // Without git there is no commit to date a confirmed area by; `sem-git` still says the review confirmed it.
        let head = if self.uses_git(run) {
            self.git().head()
        } else {
            Some("sem-git".to_string())
        };
        let mut touched = false;
        for check in &run.map_checks {
            let Some(area) = map.areas.iter_mut().find(|a| a.id == check.id && !a.attached()) else {
                continue;
            };
            if check.ok {
                if head.is_some() {
                    area.commit = head.clone();
                    touched = true;
                }
            } else if !check.note.is_empty() {
                annotate(area, &format!("run {}", run.id), &check.note);
                touched = true;
            }
        }
        if !touched {
            return vec![];
        }
        store_map::save(&self.layout, &map)
            .map(|paths| paths.into_iter().map(|p| p.display().to_string()).collect())
            .unwrap_or_default()
    }
}

/// Adds `- (<origin>) <note>` under `## Anotações` (`run <id>`, `aprendido`), keeping the newest few.
pub(crate) fn annotate(area: &mut passo_core::map::Node, origin: &str, note: &str) {
    const HEADING: &str = "## Anotações";
    let (text, mut notes) = match area.body.split_once(HEADING) {
        Some((text, notes)) => (
            text.trim_end().to_string(),
            notes
                .lines()
                .map(str::trim)
                .filter(|l| l.starts_with("- "))
                .map(str::to_string)
                .collect::<Vec<_>>(),
        ),
        None => (area.body.trim_end().to_string(), vec![]),
    };
    notes.push(format!("- ({origin}) {}", note.trim()));
    let skip = notes.len().saturating_sub(MAX_NOTES);
    area.body = format!("{text}\n\n{HEADING}\n{}", notes[skip..].join("\n"));
}

/// Dependencies the manifests declare: an area may cite them even without a `libs` entry.
pub fn manifest_libs(root: &Path) -> Vec<String> {
    let mut out = vec![];
    if let Ok(text) = std::fs::read_to_string(root.join("package.json"))
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
    {
        for key in ["dependencies", "devDependencies", "peerDependencies"] {
            if let Some(deps) = json.get(key).and_then(|d| d.as_object()) {
                out.extend(deps.keys().cloned());
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string(root.join("Cargo.toml")) {
        let mut in_deps = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_deps = line.contains("dependencies");
            } else if in_deps && let Some((name, _)) = line.split_once('=') {
                out.push(name.trim().to_string());
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string(root.join("go.mod")) {
        out.extend(text.lines().filter_map(|l| {
            let l = l.trim().trim_start_matches("require").trim();
            l.split_whitespace()
                .next()
                .filter(|m| m.contains('/'))
                .map(|m| m.rsplit('/').next().unwrap_or(m).to_string())
        }));
    }
    if let Ok(text) = std::fs::read_to_string(root.join("pyproject.toml")) {
        out.extend(text.lines().filter_map(|l| {
            let l = l.trim().trim_matches(|c| c == '"' || c == '\'' || c == ',');
            let name: String = l
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            (!name.is_empty() && l.len() > name.len() && "<>=~! ".contains(l[name.len()..].chars().next()?))
                .then_some(name)
        }));
    }
    out.sort();
    out.dedup();
    out
}
