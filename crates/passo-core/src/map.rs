//! The project map an exploring agent writes, and the choice of what each stage reads from it. Pure: the map comes
//! in already loaded, the target in already measured, and the same input gives the same selection.

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::model::Stage;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Map {
    pub project: Option<Node>,
    pub areas: Vec<Node>,
    pub libs: Vec<Node>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Project,
    #[default]
    Area,
    Lib,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// `tarefas`; an attached project's area carries its prefix: `api/armazenamento`.
    pub id: String,
    pub kind: NodeKind,
    /// Globs relative to the root of the node's project.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Ids of other areas.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Names of the libs the area uses.
    #[serde(default)]
    pub libs: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Files that back what the node says.
    #[serde(default)]
    pub sources: Vec<String>,
    /// Commit the node was written or last confirmed at.
    #[serde(default)]
    pub commit: Option<String>,
    /// The Markdown, without the frontmatter.
    #[serde(default)]
    pub body: String,
}

impl Node {
    /// Lines the node takes in a context: a heading line and its body.
    pub fn lines(&self) -> usize {
        1 + self.body.lines().count()
    }

    fn globs(&self) -> Option<GlobSet> {
        let mut builder = GlobSetBuilder::new();
        for path in &self.paths {
            builder.add(Glob::new(path).ok()?);
        }
        builder.build().ok()
    }

    /// Whether the node is an area of an attached project (its id carries the project's prefix).
    pub fn attached(&self) -> bool {
        self.id.contains('/')
    }
}

/// What a stage is working on: its files (from the brief, the step or the diff) and its words (the request).
pub struct Target<'a> {
    pub stage: Stage,
    pub files: &'a [String],
    pub words: &'a str,
}

#[derive(Debug, Default)]
pub struct Selection<'a> {
    pub nodes: Vec<&'a Node>,
    /// Target files no area covers.
    pub uncovered: Vec<String>,
}

/// Picks what the stage reads, in order, until the line budget runs out (`0` turns the map off for the stage).
pub fn select<'a>(map: &'a Map, target: &Target, budget_lines: usize) -> Selection<'a> {
    if budget_lines == 0 || target.stage == Stage::Entregar {
        return Selection::default();
    }
    let words = target.words.to_lowercase();
    let local: Vec<(&Node, GlobSet)> = map
        .areas
        .iter()
        .filter(|a| !a.attached())
        .filter_map(|a| a.globs().map(|g| (a, g)))
        .collect();
    let covered = |area: &Node| -> usize {
        local
            .iter()
            .find(|(a, _)| a.id == area.id)
            .map(|(_, globs)| target.files.iter().filter(|f| globs.is_match(f.as_str())).count())
            .unwrap_or(0)
    };
    let named = |area: &Node| {
        area.tags
            .iter()
            .chain(&area.paths)
            .any(|w| !w.is_empty() && contains_word(&words, &w.to_lowercase()))
    };

    // Areas that cover more of the target first; the map's order breaks ties.
    let mut chosen: Vec<(&Node, usize)> = match target.stage {
        Stage::Refinar => map.areas.iter().filter(|a| named(a)).map(|a| (a, 0)).collect(),
        Stage::Planejar => map
            .areas
            .iter()
            .map(|a| (a, covered(a)))
            .filter(|(a, n)| *n > 0 || named(a))
            .collect(),
        Stage::Construir | Stage::Verificar => map
            .areas
            .iter()
            .map(|a| (a, covered(a)))
            .filter(|(_, n)| *n > 0)
            .collect(),
        Stage::Entregar => vec![],
    };
    chosen.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let mut areas: Vec<&Node> = chosen.into_iter().map(|(a, _)| a).collect();

    if target.stage == Stage::Planejar {
        let hop: Vec<&Node> = areas
            .iter()
            .flat_map(|a| a.depends_on.iter())
            .filter_map(|id| map.areas.iter().find(|a| &a.id == id))
            .collect();
        for area in hop {
            if !areas.iter().any(|a| a.id == area.id) {
                areas.push(area);
            }
        }
    }

    let mut libs: Vec<&Node> = vec![];
    if target.stage != Stage::Refinar {
        for name in areas.iter().flat_map(|a| a.libs.iter()) {
            if let Some(lib) = map.libs.iter().find(|l| &l.id == name)
                && !libs.iter().any(|l| l.id == lib.id)
            {
                libs.push(lib);
            }
        }
    }

    let mut selection = Selection::default();
    let mut used = 0;
    if let Some(project) = &map.project {
        used += project.lines();
        selection.nodes.push(project);
    }
    for node in areas.into_iter().chain(libs) {
        if used + node.lines() > budget_lines {
            break;
        }
        used += node.lines();
        selection.nodes.push(node);
    }
    if matches!(target.stage, Stage::Planejar | Stage::Construir | Stage::Verificar) {
        selection.uncovered = target
            .files
            .iter()
            .filter(|f| !local.iter().any(|(_, g)| g.is_match(f.as_str())))
            .cloned()
            .collect();
    }
    selection
}

/// `word` appears in `text` on its own, not inside a longer word.
pub fn contains_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + word.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(id: &str, paths: &[&str], deps: &[&str], libs: &[&str], tags: &[&str], lines: usize) -> Node {
        Node {
            id: id.into(),
            kind: NodeKind::Area,
            paths: paths.iter().map(|s| s.to_string()).collect(),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            libs: libs.iter().map(|s| s.to_string()).collect(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            body: "linha\n".repeat(lines),
            ..Default::default()
        }
    }

    fn map() -> Map {
        Map {
            project: Some(Node {
                id: "projeto".into(),
                kind: NodeKind::Project,
                body: "stack".into(),
                ..Default::default()
            }),
            areas: vec![
                area(
                    "tarefas",
                    &["src/tasks.js"],
                    &["armazenamento"],
                    &["commander"],
                    &["tarefas", "cli"],
                    3,
                ),
                area("armazenamento", &["src/store/**"], &[], &["sqlite"], &["banco"], 3),
                area("http", &["src/http/**"], &["tarefas"], &[], &["api"], 3),
                area("ui", &["web/**"], &[], &[], &["tela"], 3),
                area("infra", &["deploy/**"], &[], &[], &["deploy"], 3),
            ],
            libs: vec![
                Node {
                    id: "commander".into(),
                    kind: NodeKind::Lib,
                    body: "uso".into(),
                    ..Default::default()
                },
                Node {
                    id: "sqlite".into(),
                    kind: NodeKind::Lib,
                    body: "uso".into(),
                    ..Default::default()
                },
            ],
        }
    }

    fn ids(s: &Selection) -> Vec<String> {
        s.nodes.iter().map(|n| n.id.clone()).collect()
    }

    fn files(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn refine_picks_areas_named_in_the_request_without_hops_or_libs() {
        let map = map();
        let none = files(&[]);
        let s = select(
            &map,
            &Target {
                stage: Stage::Refinar,
                files: &none,
                words: "Adicionar prioridade às tarefas",
            },
            100,
        );
        assert_eq!(ids(&s), vec!["projeto", "tarefas"]);
    }

    #[test]
    fn plan_covers_files_hops_one_dependency_and_brings_libs() {
        let map = map();
        let f = files(&["src/http/routes.js"]);
        let s = select(
            &map,
            &Target {
                stage: Stage::Planejar,
                files: &f,
                words: "",
            },
            100,
        );
        assert_eq!(
            ids(&s),
            vec!["projeto", "http", "tarefas", "commander"],
            "one hop: http → tarefas, not → armazenamento"
        );
    }

    #[test]
    fn build_and_verify_take_only_covering_areas_and_their_libs() {
        let map = map();
        let f = files(&["src/store/db.js", "src/tasks.js", "src/store/x.js", "src/novo/a.js"]);
        for stage in [Stage::Construir, Stage::Verificar] {
            let s = select(
                &map,
                &Target {
                    stage,
                    files: &f,
                    words: "tela",
                },
                100,
            );
            assert_eq!(
                ids(&s),
                vec!["projeto", "armazenamento", "tarefas", "sqlite", "commander"],
                "{stage:?}"
            );
            assert_eq!(s.uncovered, vec!["src/novo/a.js"]);
        }
    }

    #[test]
    fn the_budget_cuts_in_order_and_zero_turns_it_off() {
        let map = map();
        let f = files(&["src/store/db.js", "src/store/x.js", "src/tasks.js"]);
        let s = select(
            &map,
            &Target {
                stage: Stage::Construir,
                files: &f,
                words: "",
            },
            6,
        );
        assert_eq!(
            ids(&s),
            vec!["projeto", "armazenamento"],
            "project 1 line + armazenamento 4; tarefas does not fit"
        );
        assert!(
            select(
                &map,
                &Target {
                    stage: Stage::Construir,
                    files: &f,
                    words: ""
                },
                0
            )
            .nodes
            .is_empty()
        );
        assert!(
            select(
                &map,
                &Target {
                    stage: Stage::Entregar,
                    files: &f,
                    words: ""
                },
                100
            )
            .nodes
            .is_empty()
        );
    }

    #[test]
    fn the_same_input_gives_the_same_selection() {
        let map = map();
        let f = files(&["web/a.js", "deploy/x.yaml", "src/tasks.js"]);
        let run = || {
            ids(&select(
                &map,
                &Target {
                    stage: Stage::Planejar,
                    files: &f,
                    words: "api",
                },
                100,
            ))
        };
        assert_eq!(run(), run());
    }
}
