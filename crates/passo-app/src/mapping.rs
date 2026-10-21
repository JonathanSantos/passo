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
            projects: vec![self.layout.root.display().to_string()],
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
        let agent = self.catalog.agents.get("explorador");
        crate::render::map_task(&crate::render::MapView {
            agent,
            delegation: &self.delegation("explorador"),
            tree: &tree,
            current: current.as_ref(),
            template: MAP_TEMPLATE,
            bin: &self.bin,
        })
    }

    pub fn map_done(&self, input: DoneInput) -> Result<String> {
        let Some(task) = store_map::load_task(&self.layout) else {
            bail!("não há mapeamento em andamento; comece com passo map");
        };
        let text = read_input(&input.text)?;
        let root = &self.layout.root;
        let draft = spec::parse_map(&text, &manifest_libs(root)).map_err(|invalid| self.map_invalid(invalid))?;

        let git = self.git();
        let repo = git.is_repo();
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

        let map = draft.into_map(if repo { git.head() } else { None });
        let (areas, libs) = (map.areas.len(), map.libs.len());
        store_map::save(&self.layout, &map)?;
        store_map::remove_task(&self.layout);
        self.map_log(
            "map.saved",
            Actor::Agent,
            format!("Mapa salvo: {areas} áreas, {libs} libs"),
            json!({ "areas": areas, "libs": libs }),
        );
        Ok(format!("Mapa salvo: {areas} áreas, {libs} libs. Commite .passo/map/."))
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
