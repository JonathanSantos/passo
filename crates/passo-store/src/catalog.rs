//! Agents, stage instructions and checklists: built in, then overridden by the project's `.passo/` files.
//!
//! Extending never touches the core:
//! - `.passo/agents/<nome>.md` with `extends: construtor` adds to that agent; without `extends` it replaces it
//!   (same name) or creates a new agent.
//! - `.passo/stages/<etapa>.md` changes how a stage is done: which agent, extra checks, extra instructions
//!   (appended, or `mode: replace`). A stage can never be switched off; sizes decide who runs it.
//! - `.passo/checklists/<nome>.md` adds a domain checklist for planning and review.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, bail};
use passo_core::{Size, Stage};

use crate::assets;
use crate::frontmatter::{self, Doc};
use crate::layout::Layout;

#[derive(Debug, Clone)]
pub struct AgentDef {
    pub name: String,
    pub description: String,
    /// Model tier (`leve`, `forte`), resolved per platform by the config.
    pub model: String,
    /// Tools the platform should grant (platform names); empty = the platform's default.
    pub tools: Vec<String>,
    pub body: String,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct StageDef {
    pub stage: Stage,
    pub agent: Option<String>,
    /// Extra checks after each step (only meaningful in `construir`) or before review (`verificar`).
    pub checks: Vec<String>,
    /// `todos`, `pp`, `p`, `m`, `g` and any other heading.
    pub sections: BTreeMap<String, String>,
    pub sources: Vec<String>,
}

impl StageDef {
    /// The instructions for a size: what every size does, then what this size adds.
    pub fn instructions(&self, size: Size) -> String {
        [self.sections.get("todos"), self.sections.get(size.id())]
            .into_iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    pub fn section(&self, key: &str) -> Option<&str> {
        self.sections.get(key).map(String::as_str)
    }
}

#[derive(Debug, Clone)]
pub struct Checklist {
    pub name: String,
    pub title: String,
    /// Words in the request that bring this checklist in.
    pub matches: Vec<String>,
    pub body: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct Catalog {
    pub agents: BTreeMap<String, AgentDef>,
    pub stages: BTreeMap<Stage, StageDef>,
    pub checklists: Vec<Checklist>,
    /// The project's conventions (`.passo/context.md`), shown to every agent.
    pub context: Option<String>,
}

const STAGE_KEYS: [&str; 5] = ["agent", "checks", "mode", "description", "name"];

impl Catalog {
    pub fn builtin() -> Catalog {
        let mut catalog = Catalog {
            agents: BTreeMap::new(),
            stages: BTreeMap::new(),
            checklists: vec![],
            context: None,
        };
        for (name, text) in assets::AGENTS {
            catalog
                .apply_agent(name, frontmatter::parse(text), "embutido")
                .expect("built-in agent");
        }
        for (name, text) in assets::STAGES {
            let stage = Stage::parse(name).expect("built-in stage");
            catalog
                .apply_stage(stage, frontmatter::parse(text), "embutido")
                .expect("built-in stage");
        }
        for (name, text) in assets::CHECKLISTS {
            catalog
                .checklists
                .push(checklist(name, frontmatter::parse(text), "embutido"));
        }
        catalog
    }

    pub fn load(layout: &Layout) -> Result<Catalog> {
        let mut catalog = Catalog::builtin();
        for (name, path) in markdown_files(&layout.agents()) {
            let doc = frontmatter::parse(&std::fs::read_to_string(&path)?);
            catalog.apply_agent(&name, doc, &path.display().to_string())?;
        }
        for (name, path) in markdown_files(&layout.stages()) {
            let Some(stage) = Stage::parse(&name) else {
                bail!(
                    "{}: `{name}` não é uma etapa. As etapas são: {}",
                    path.display(),
                    Stage::ALL.map(|s| s.id()).join(", ")
                );
            };
            let doc = frontmatter::parse(&std::fs::read_to_string(&path)?);
            catalog.apply_stage(stage, doc, &path.display().to_string())?;
        }
        for (name, path) in markdown_files(&layout.checklists()) {
            let doc = frontmatter::parse(&std::fs::read_to_string(&path)?);
            let item = checklist(&name, doc, &path.display().to_string());
            catalog.checklists.retain(|c| c.name != item.name);
            catalog.checklists.push(item);
        }
        catalog.context = std::fs::read_to_string(layout.context())
            .ok()
            .filter(|t| !t.trim().is_empty());
        catalog.validate()?;
        Ok(catalog)
    }

    fn apply_agent(&mut self, file_name: &str, doc: Doc, source: &str) -> Result<()> {
        let name = doc.text("name").unwrap_or_else(|| file_name.to_string());
        let base = match doc.text("extends") {
            Some(parent) => match self.agents.get(&parent) {
                Some(agent) => Some(agent.clone()),
                None => bail!("{source}: extends `{parent}`, mas esse agente não existe"),
            },
            None => None,
        };
        let agent = match base {
            Some(mut agent) => {
                agent.name = name.clone();
                if let Some(d) = doc.text("description") {
                    agent.description = d;
                }
                if let Some(m) = doc.text("model") {
                    agent.model = m;
                }
                if doc.fields.contains_key("tools") {
                    agent.tools = doc.list("tools");
                }
                if !doc.body.is_empty() {
                    agent.body = format!("{}\n\n{}", agent.body, doc.body);
                }
                agent.sources.push(source.to_string());
                agent
            }
            None => AgentDef {
                name: name.clone(),
                description: doc.text("description").unwrap_or_default(),
                model: doc.text("model").unwrap_or_else(|| "leve".into()),
                tools: doc.list("tools"),
                body: doc.body,
                sources: vec![source.to_string()],
            },
        };
        self.agents.insert(name, agent);
        Ok(())
    }

    fn apply_stage(&mut self, stage: Stage, doc: Doc, source: &str) -> Result<()> {
        for key in doc.fields.keys() {
            if ["skip", "enabled", "disabled", "off"].contains(&key.as_str()) {
                bail!(
                    "{source}: etapas não podem ser desligadas. Mude como a etapa é feita (agent, checks, instruções) ou use os tamanhos (`[sizes.pp] {} = \"auto\"`).",
                    stage.id()
                );
            }
            if !STAGE_KEYS.contains(&key.as_str()) {
                bail!("{source}: campo `{key}` desconhecido (use {})", STAGE_KEYS.join(", "));
            }
        }
        let replace = doc.text("mode").as_deref() == Some("replace");
        let sections = frontmatter::sections(&doc.body);
        let entry = self.stages.entry(stage).or_insert_with(|| StageDef {
            stage,
            agent: None,
            checks: vec![],
            sections: BTreeMap::new(),
            sources: vec![],
        });
        if let Some(agent) = doc.text("agent") {
            entry.agent = Some(agent);
        }
        entry.checks.extend(doc.list("checks"));
        if replace {
            entry.sections = sections;
        } else {
            for (key, text) in sections {
                let slot = entry.sections.entry(key).or_default();
                if !slot.is_empty() {
                    slot.push_str("\n\n");
                }
                slot.push_str(&text);
            }
        }
        entry.sources.push(source.to_string());
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        for def in self.stages.values() {
            if let Some(agent) = &def.agent
                && !self.agents.contains_key(agent)
            {
                bail!(
                    "a etapa {} usa o agente `{agent}`, que não existe (há: {})",
                    def.stage.id(),
                    self.agents.keys().cloned().collect::<Vec<_>>().join(", ")
                );
            }
        }
        Ok(())
    }

    pub fn stage(&self, stage: Stage) -> Option<&StageDef> {
        self.stages.get(&stage)
    }

    pub fn agent_for(&self, stage: Stage) -> Option<&AgentDef> {
        self.stage(stage)
            .and_then(|s| s.agent.as_ref())
            .and_then(|name| self.agents.get(name))
    }

    /// Checklists whose words appear in the text (the request, the brief).
    pub fn checklists_for(&self, text: &str) -> Vec<&Checklist> {
        let text = text.to_lowercase();
        self.checklists
            .iter()
            .filter(|c| c.matches.iter().any(|m| contains_word(&text, &m.to_lowercase())))
            .collect()
    }
}

fn contains_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + word.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

fn checklist(name: &str, doc: Doc, source: &str) -> Checklist {
    Checklist {
        name: name.to_string(),
        title: doc.text("title").unwrap_or_else(|| name.to_string()),
        matches: doc.list("match"),
        body: doc.body,
        source: source.to_string(),
    }
}

fn markdown_files(dir: &Path) -> Vec<(String, std::path::PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut files: Vec<(String, std::path::PathBuf)> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .filter_map(|p| Some((p.file_stem()?.to_str()?.to_string(), p.clone())))
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> Layout {
        let dir = std::env::temp_dir().join(format!("passo-catalog-{}-{}", std::process::id(), files.len()));
        std::fs::remove_dir_all(&dir).ok();
        for (path, text) in files {
            let path = dir.join(".passo").join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Layout::new(dir)
    }

    #[test]
    fn builtin_has_an_agent_per_agent_stage() {
        let catalog = Catalog::builtin();
        for stage in [Stage::Refinar, Stage::Planejar, Stage::Construir, Stage::Verificar] {
            assert!(catalog.agent_for(stage).is_some(), "{stage:?}");
            assert!(!catalog.stage(stage).unwrap().instructions(Size::M).is_empty());
        }
    }

    #[test]
    fn a_project_agent_extends_and_a_stage_switches_to_it() {
        let layout = project(&[
            (
                "agents/construtor-rust.md",
                "---\nextends: construtor\nmodel: forte\n---\nUse `thiserror` para erros.",
            ),
            (
                "stages/construir.md",
                "---\nagent: construtor-rust\nchecks: [cargo clippy]\n---\n## m\nRode clippy antes.",
            ),
        ]);
        let catalog = Catalog::load(&layout).unwrap();
        let agent = catalog.agent_for(Stage::Construir).unwrap();
        assert_eq!(agent.name, "construtor-rust");
        assert_eq!(agent.model, "forte");
        assert!(
            agent.body.contains("thiserror") && agent.body.len() > 100,
            "keeps the base body"
        );
        let stage = catalog.stage(Stage::Construir).unwrap();
        assert_eq!(stage.checks, vec!["cargo clippy"]);
        assert!(stage.instructions(Size::M).contains("Rode clippy antes."));
        std::fs::remove_dir_all(&layout.root).ok();
    }

    #[test]
    fn stages_cannot_be_switched_off() {
        let layout = project(&[("stages/verificar.md", "---\nskip: true\n---\n")]);
        let err = Catalog::load(&layout).unwrap_err().to_string();
        assert!(err.contains("não podem ser desligadas"), "{err}");
        std::fs::remove_dir_all(&layout.root).ok();
    }

    #[test]
    fn checklists_match_whole_words() {
        let catalog = Catalog::builtin();
        let names: Vec<&str> = catalog
            .checklists_for("criar endpoint REST de tarefas")
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(names.contains(&"api-http"));
        assert!(catalog.checklists_for("clientes").iter().all(|c| c.name != "cli"));
    }
}
