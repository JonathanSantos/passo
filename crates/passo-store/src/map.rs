//! The project map on disk: Markdown with a YAML frontmatter, one file per node, kept in git.
//!
//! ```text
//! .passo/map/
//!   projeto.md        stack, how to run, how to test, the conventions that matter most
//!   areas/<id>.md     one per area (module, layer, folder with its own job)
//!   libs/<nome>.md    one per library the project wraps in its own way
//! ```
//!
//! A person may edit it by hand; agents write it only through Passo. `save` then `load` gives the same map back.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use passo_core::map::{Map, Node, NodeKind};
use serde_yaml_ng::{Mapping, Value};

use crate::frontmatter::{self, Doc};
use crate::layout::{Layout, write_atomic};

pub const PROJECT_FILE: &str = "projeto.md";
pub const PROJECT_ID: &str = "projeto";

fn keys(kind: NodeKind) -> &'static [&'static str] {
    match kind {
        NodeKind::Project => &["sources", "commit"],
        NodeKind::Area => &["paths", "depends_on", "libs", "tags", "sources", "commit"],
        NodeKind::Lib => &["tags", "sources", "commit"],
    }
}

/// Reads the map, or `None` when `.passo/map/` does not exist. Errors name the file (and the line when known).
pub fn load(layout: &Layout) -> Result<Option<Map>> {
    let dir = layout.project_map();
    if !dir.is_dir() {
        return Ok(None);
    }
    let mut map = Map::default();
    let project = dir.join(PROJECT_FILE);
    if project.is_file() {
        map.project = Some(read_node(&project, PROJECT_ID, NodeKind::Project)?);
    }
    for (id, path) in markdown_files(&dir.join("areas")) {
        map.areas.push(read_node(&path, &id, NodeKind::Area)?);
    }
    for (id, path) in markdown_files(&dir.join("libs")) {
        map.libs.push(read_node(&path, &id, NodeKind::Lib)?);
    }
    if map.project.is_none() && map.areas.is_empty() && map.libs.is_empty() {
        return Ok(None);
    }
    for area in &map.areas {
        for dep in &area.depends_on {
            if !dep.contains('/') && !map.areas.iter().any(|a| &a.id == dep) {
                let path = dir.join("areas").join(format!("{}.md", area.id));
                let line = line_of(&std::fs::read_to_string(&path).unwrap_or_default(), "depends_on");
                bail!(
                    "{}{}: depends_on cita `{dep}`, que não é uma área do mapa",
                    path.display(),
                    line
                );
            }
        }
    }
    Ok(Some(map))
}

/// The project's map plus each attached project's, their ids prefixed with the project's name (`api/armazenamento`,
/// and so their dependencies and libs). An attached project without a map adds nothing.
pub fn load_all(layout: &Layout, attached: &[(String, PathBuf)]) -> Result<Option<Map>> {
    let mut map = load(layout)?;
    for (name, root) in attached {
        let Some(other) = load(&Layout::new(root))? else {
            continue;
        };
        let prefix = |id: &str| {
            if id.contains('/') {
                id.to_string()
            } else {
                format!("{name}/{id}")
            }
        };
        let merged = map.get_or_insert_with(Map::default);
        for mut area in other.areas {
            area.id = prefix(&area.id);
            area.depends_on = area.depends_on.iter().map(|d| prefix(d)).collect();
            area.libs = area.libs.iter().map(|l| prefix(l)).collect();
            merged.areas.push(area);
        }
        for mut lib in other.libs {
            lib.id = prefix(&lib.id);
            merged.libs.push(lib);
        }
    }
    Ok(map)
}

fn read_node(path: &Path, id: &str, kind: NodeKind) -> Result<Node> {
    let text = std::fs::read_to_string(path)?;
    let doc = frontmatter::parse(&text).map_err(|e| anyhow!("{}: {e}", path.display()))?;
    let allowed = keys(kind);
    for key in doc.keys() {
        if !allowed.contains(&key.as_str()) {
            bail!(
                "{}{}: campo `{key}` desconhecido (use {})",
                path.display(),
                line_of(&text, &key),
                allowed.join(", ")
            );
        }
    }
    Ok(node_from(id, kind, &doc))
}

fn node_from(id: &str, kind: NodeKind, doc: &Doc) -> Node {
    Node {
        id: id.to_string(),
        kind,
        paths: doc.list("paths"),
        depends_on: doc.list("depends_on"),
        libs: doc.list("libs"),
        tags: doc.list("tags"),
        sources: doc.list("sources"),
        commit: doc.text("commit").filter(|c| !c.is_empty()),
        body: doc.body.clone(),
    }
}

/// ` linha N` for the line where `key:` starts in a frontmatter, or nothing.
fn line_of(text: &str, key: &str) -> String {
    text.lines()
        .position(|l| l.trim_start().starts_with(&format!("{key}:")))
        .map(|i| format!(" linha {}", i + 1))
        .unwrap_or_default()
}

/// A mapping in progress: when it began, at which commit, and what the lock and the stop hook keep about it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MapTask {
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub commit: Option<String>,
    /// Projects the explorer reads: the project itself, then attached ones.
    pub projects: Vec<String>,
    /// Files already changed when the mapping began: anything new at delivery broke the read-only rule.
    #[serde(default)]
    pub baseline: Vec<String>,
    /// Times the stop hook kept the agent going.
    #[serde(default)]
    pub stops: u32,
}

pub fn load_task(layout: &Layout) -> Option<MapTask> {
    std::fs::read_to_string(layout.map_task())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

pub fn save_task(layout: &Layout, task: &MapTask) -> Result<()> {
    write_atomic(&layout.map_task(), serde_json::to_string_pretty(task)?.as_bytes())?;
    Ok(())
}

pub fn remove_task(layout: &Layout) {
    std::fs::remove_file(layout.map_task()).ok();
}

/// Writes the map and removes areas and libs that left it. Returns the touched paths, relative to the project
/// root, for git.
pub fn save(layout: &Layout, map: &Map) -> Result<Vec<PathBuf>> {
    let dir = layout.project_map();
    let mut touched = vec![];
    let relative = |path: &Path| path.strip_prefix(&layout.root).unwrap_or(path).to_path_buf();
    if let Some(project) = &map.project {
        let path = dir.join(PROJECT_FILE);
        write_atomic(&path, render(project).as_bytes())?;
        touched.push(relative(&path));
    }
    for (folder, nodes) in [("areas", &map.areas), ("libs", &map.libs)] {
        let folder = dir.join(folder);
        std::fs::create_dir_all(&folder)?;
        for node in nodes.iter() {
            let path = folder.join(format!("{}.md", node.id));
            write_atomic(&path, render(node).as_bytes())?;
            touched.push(relative(&path));
        }
        for (id, path) in markdown_files(&folder) {
            if !nodes.iter().any(|n| n.id == id) {
                std::fs::remove_file(&path)?;
                touched.push(relative(&path));
            }
        }
    }
    Ok(touched)
}

/// Sections a remap keeps: what the reviewer and `passo learn` wrote, and what the dev added under them.
const KEPT_SECTIONS: [&str; 2] = ["## Anotações", "## Aprendido"];

/// Where the kept sections start in a body, if any.
fn kept_from(body: &str) -> Option<usize> {
    KEPT_SECTIONS.iter().filter_map(|h| body.find(h)).min()
}

/// A new map from `passo map` keeps, area by area, the `## Anotações` and `## Aprendido` sections of the map it
/// replaces: the new body replaces only what comes before them. Returns the areas that left the map, whose notes
/// go with them.
pub fn carry_notes(old: &Map, new: &mut Map) -> Vec<String> {
    for area in &mut new.areas {
        let Some(previous) = old.areas.iter().find(|a| a.id == area.id) else {
            continue;
        };
        let Some(at) = kept_from(&previous.body) else { continue };
        let fresh = match kept_from(&area.body) {
            Some(cut) => &area.body[..cut],
            None => area.body.as_str(),
        };
        area.body = format!("{}\n\n{}", fresh.trim_end(), previous.body[at..].trim());
    }
    old.areas
        .iter()
        .filter(|a| !a.attached() && !new.areas.iter().any(|n| n.id == a.id))
        .map(|a| a.id.clone())
        .collect()
}

/// The node as Markdown: frontmatter with its non-empty fields, in a fixed order, then the body.
pub fn render(node: &Node) -> String {
    let mut fields = Mapping::new();
    let list = |items: &[String]| Value::Sequence(items.iter().map(|s| Value::String(s.clone())).collect());
    for (key, items) in [
        ("paths", &node.paths),
        ("depends_on", &node.depends_on),
        ("libs", &node.libs),
        ("tags", &node.tags),
        ("sources", &node.sources),
    ] {
        if !items.is_empty() && keys(node.kind).contains(&key) {
            fields.insert(Value::String(key.into()), list(items));
        }
    }
    if let Some(commit) = &node.commit {
        fields.insert(Value::String("commit".into()), Value::String(commit.clone()));
    }
    let head = if fields.is_empty() {
        String::new()
    } else {
        serde_yaml_ng::to_string(&fields).unwrap_or_default()
    };
    format!("---\n{head}---\n{}\n", node.body.trim())
}

fn markdown_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut files: Vec<(String, PathBuf)> = entries
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

    fn layout(name: &str) -> Layout {
        let dir = std::env::temp_dir().join(format!("passo-map-store-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join(".passo")).unwrap();
        Layout::new(dir)
    }

    fn sample() -> Map {
        Map {
            project: Some(Node {
                id: PROJECT_ID.into(),
                kind: NodeKind::Project,
                sources: vec!["package.json".into()],
                commit: Some("1234e56".into()),
                body: "Node 22 · testar: npm test".into(),
                ..Default::default()
            }),
            areas: vec![
                Node {
                    id: "armazenamento".into(),
                    kind: NodeKind::Area,
                    paths: vec!["src/store/**".into()],
                    sources: vec!["src/store/db.js".into()],
                    body: "O banco em memória.".into(),
                    ..Default::default()
                },
                Node {
                    id: "tarefas".into(),
                    kind: NodeKind::Area,
                    paths: vec!["src/tasks.js".into(), "src/cli.js".into()],
                    depends_on: vec!["armazenamento".into()],
                    libs: vec!["commander".into()],
                    tags: vec!["cli".into(), "tarefas".into()],
                    sources: vec!["src/tasks.js".into()],
                    commit: Some("7033609".into()),
                    body: "Regras: as tarefas.\nTestes em `test/tasks.test.js`.".into(),
                },
            ],
            libs: vec![Node {
                id: "commander".into(),
                kind: NodeKind::Lib,
                sources: vec!["src/cli.js".into()],
                body: "Como o projeto a envolve.".into(),
                ..Default::default()
            }],
        }
    }

    #[test]
    fn save_then_load_is_lossless_and_removes_what_left() {
        let layout = layout("roundtrip");
        let map = sample();
        let written = save(&layout, &map).unwrap();
        assert!(written.contains(&PathBuf::from(".passo/map/areas/tarefas.md")));
        assert_eq!(load(&layout).unwrap().unwrap(), map);
        let mut smaller = map.clone();
        smaller.libs.clear();
        let touched = save(&layout, &smaller).unwrap();
        assert!(
            touched.contains(&PathBuf::from(".passo/map/libs/commander.md")),
            "removed files are reported"
        );
        assert!(load(&layout).unwrap().unwrap().libs.is_empty());
        std::fs::remove_dir_all(&layout.root).ok();
    }

    #[test]
    fn an_orphan_dependency_names_the_file() {
        let layout = layout("orphan");
        let dir = layout.project_map().join("areas");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.md"), "---\npaths: [a]\ndepends_on: [cache]\n---\ncorpo\n").unwrap();
        let err = load(&layout).unwrap_err().to_string();
        assert!(err.contains("areas/x.md linha 3") && err.contains("`cache`"), "{err}");
        std::fs::write(dir.join("x.md"), "---\npaths: [a]\nmodulo: y\n---\ncorpo\n").unwrap();
        let err = load(&layout).unwrap_err().to_string();
        assert!(err.contains("campo `modulo` desconhecido"), "{err}");
        std::fs::remove_dir_all(&layout.root).ok();
    }

    #[test]
    fn a_remap_keeps_the_notes_and_lists_the_areas_that_left() {
        let mut old = sample();
        old.areas[1].body = "Regras antigas.\n\n## Anotações\n- (run r1) o filtro mudou".into();
        let mut new = sample();
        new.areas.remove(0);
        new.areas[0].body = "Regras novas.".into();
        let removed = carry_notes(&old, &mut new);
        assert_eq!(removed, vec!["armazenamento".to_string()]);
        assert_eq!(
            new.areas[0].body,
            "Regras novas.\n\n## Anotações\n- (run r1) o filtro mudou"
        );
    }

    #[test]
    fn no_map_folder_is_no_map() {
        let layout = layout("none");
        assert_eq!(load(&layout).unwrap(), None);
        std::fs::remove_dir_all(&layout.root).ok();
    }
}
