//! What the dev keeps repeating across runs, collected without a model: rejections, changes to the request, hints,
//! answers that differed from the recommendation, and the reviewer's observations. `passo learn` hands the groups
//! to an agent, who turns them into short conventions the dev approves.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::flow::fnv;
use crate::model::{ChangeKind, Run, Stage, Status};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LessonKind {
    Recusa,
    Mudanca,
    Dica,
    Resposta,
    /// An observation the reviewer left.
    Nota,
}

impl LessonKind {
    pub fn label(self) -> &'static str {
        match self {
            LessonKind::Recusa => "recusa",
            LessonKind::Mudanca => "mudança no pedido",
            LessonKind::Dica => "dica",
            LessonKind::Resposta => "resposta diferente da recomendada",
            LessonKind::Nota => "observação do revisor",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lesson {
    /// Stable id: FNV of the source line (run, kind and text).
    pub fingerprint: String,
    pub kind: LessonKind,
    pub text: String,
    pub run_id: String,
    pub at: DateTime<Utc>,
    pub stage: Stage,
    /// Files of the step in progress when it happened, when known.
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub lessons: Vec<Lesson>,
    /// Lessons from two or more runs.
    pub recurrent: bool,
}

impl Lesson {
    /// How the task shows it and how a proposal cites it: `run <id>: <kind>: <text>`.
    pub fn source(&self) -> String {
        source(&self.run_id, self.kind, &self.text)
    }
}

fn source(run_id: &str, kind: LessonKind, text: &str) -> String {
    format!("run {run_id}: {}: {}", kind.label(), text.trim())
}

/// The lesson a proposal's source points to: the source line itself, or any text with the fingerprint in it.
pub fn cited<'a>(lessons: &'a [Lesson], fonte: &str) -> Option<&'a Lesson> {
    let fonte = fonte.trim();
    let print = fnv(fonte.as_bytes());
    lessons
        .iter()
        .find(|l| l.fingerprint == print || fonte.contains(&l.fingerprint))
}

/// Lessons from finished runs (done or stopped), leaving out the ones already seen.
pub fn collect(runs: &[Run], seen: &[String]) -> Vec<Lesson> {
    let mut out = vec![];
    for run in runs.iter().filter(|r| r.status != Status::Active) {
        let files_of = |step: &Option<String>| -> Vec<String> {
            step.as_ref()
                .and_then(|id| run.steps.iter().find(|s| &s.id == id))
                .map(|s| s.files.clone())
                .unwrap_or_default()
        };
        for change in &run.changes {
            let kind = match change.kind {
                ChangeKind::Recusa => LessonKind::Recusa,
                ChangeKind::Mudanca => LessonKind::Mudanca,
                ChangeKind::Dica => LessonKind::Dica,
                ChangeKind::Resposta if change.before.is_some() && change.before != change.after => {
                    LessonKind::Resposta
                }
                _ => continue,
            };
            push(
                &mut out,
                seen,
                run,
                kind,
                &change.text,
                change.at,
                change.stage,
                files_of(&change.step),
            );
        }
        for note in run.notes.iter().filter_map(|n| n.strip_prefix("Revisão: ")) {
            push(
                &mut out,
                seen,
                run,
                LessonKind::Nota,
                note,
                run.updated_at,
                Stage::Verificar,
                vec![],
            );
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn push(
    out: &mut Vec<Lesson>,
    seen: &[String],
    run: &Run,
    kind: LessonKind,
    text: &str,
    at: DateTime<Utc>,
    stage: Stage,
    files: Vec<String>,
) {
    let fingerprint = fnv(source(&run.id, kind, text).as_bytes());
    if seen.contains(&fingerprint) || out.iter().any(|l: &Lesson| l.fingerprint == fingerprint) {
        return;
    }
    out.push(Lesson {
        fingerprint,
        kind,
        text: text.to_string(),
        run_id: run.id.clone(),
        at,
        stage,
        files,
    });
}

const STOP: [&str; 12] = [
    "sobre", "porque", "quando", "depois", "antes", "também", "tambem", "ainda", "sempre", "nunca", "muito", "outra",
];

/// Words of five letters or more, without accents, without the commonest ones.
fn words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(fold)
        .filter(|w| w.chars().count() >= 5 && !STOP.contains(&w.as_str()))
        .collect()
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
        .collect()
}

/// Groups lessons that share two or more words; a group with lessons from two or more runs is recurrent.
/// Recurrent groups come first; within that, the order lessons were collected in.
pub fn group(lessons: &[Lesson]) -> Vec<Group> {
    let bags: Vec<BTreeSet<String>> = lessons.iter().map(|l| words(&l.text)).collect();
    // Union-find over "shares two words".
    let mut parent: Vec<usize> = (0..lessons.len()).collect();
    fn root(parent: &mut [usize], i: usize) -> usize {
        let mut i = i;
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for a in 0..lessons.len() {
        for b in a + 1..lessons.len() {
            if bags[a].intersection(&bags[b]).count() >= 2 {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                if ra != rb {
                    parent[rb] = ra;
                }
            }
        }
    }
    let mut groups: Vec<(usize, Vec<Lesson>)> = vec![];
    for (i, lesson) in lessons.iter().enumerate() {
        let r = root(&mut parent, i);
        match groups.iter_mut().find(|(g, _)| *g == r) {
            Some((_, list)) => list.push(lesson.clone()),
            None => groups.push((r, vec![lesson.clone()])),
        }
    }
    let mut out: Vec<Group> = groups
        .into_iter()
        .map(|(_, lessons)| {
            let runs: BTreeSet<&str> = lessons.iter().map(|l| l.run_id.as_str()).collect();
            let recurrent = runs.len() >= 2;
            Group { lessons, recurrent }
        })
        .collect();
    out.sort_by_key(|g| !g.recurrent);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Actor, Change, Size};

    fn run(id: &str, rejections: &[&str]) -> Run {
        let mut run = Run::new(id, "pedido", Size::M, Utc::now());
        run.status = Status::Done;
        for text in rejections {
            run.changes.push(Change {
                at: Utc::now(),
                by: Actor::Dev,
                kind: ChangeKind::Recusa,
                stage: Stage::Entregar,
                text: text.to_string(),
                before: None,
                after: None,
                step: None,
            });
        }
        run
    }

    #[test]
    fn rejections_sharing_words_across_runs_are_recurrent() {
        let runs = vec![
            run("r1", &["a listagem veio em ordem alfabética"]),
            run("r2", &["listagem precisa da ordem de criação, não alfabética"]),
            run("r3", &["faltou testar o erro de rede"]),
        ];
        let lessons = collect(&runs, &[]);
        assert_eq!(lessons.len(), 3);
        let groups = group(&lessons);
        assert!(groups[0].recurrent && groups[0].lessons.len() == 2, "{groups:?}");
        assert!(!groups[1].recurrent);
    }

    #[test]
    fn the_same_rejection_twice_in_one_run_is_not_recurrent() {
        let runs = vec![run(
            "r1",
            &[
                "a listagem veio em ordem alfabética",
                "a listagem ainda em ordem alfabética",
            ],
        )];
        let groups = group(&collect(&runs, &[]));
        assert_eq!(groups.len(), 1);
        assert!(!groups[0].recurrent);
    }

    #[test]
    fn seen_lessons_and_active_runs_are_left_out() {
        let mut active = run("r2", &["outra recusa qualquer"]);
        active.status = Status::Active;
        let runs = vec![run("r1", &["a listagem veio em ordem alfabética"]), active];
        let first = collect(&runs, &[]);
        assert_eq!(first.len(), 1);
        assert!(collect(&runs, &[first[0].fingerprint.clone()]).is_empty());
    }
}
