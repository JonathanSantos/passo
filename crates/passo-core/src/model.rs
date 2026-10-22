//! The vocabulary of a run: stages, sizes, who acts, and the records a run keeps.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::spec::{Brief, Plan, Question};

/// The five stages, always in this order. Nothing outside the core can reorder or remove them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Refinar,
    Planejar,
    Construir,
    Verificar,
    Entregar,
}

impl Stage {
    pub const ALL: [Stage; 5] = [
        Stage::Refinar,
        Stage::Planejar,
        Stage::Construir,
        Stage::Verificar,
        Stage::Entregar,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Stage::Refinar => "refinar",
            Stage::Planejar => "planejar",
            Stage::Construir => "construir",
            Stage::Verificar => "verificar",
            Stage::Entregar => "entregar",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Stage::Refinar => "Refinar",
            Stage::Planejar => "Planejar",
            Stage::Construir => "Construir",
            Stage::Verificar => "Verificar",
            Stage::Entregar => "Entregar",
        }
    }

    pub fn index(self) -> usize {
        Stage::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    pub fn parse(text: &str) -> Option<Stage> {
        Stage::ALL.into_iter().find(|s| s.id() == text.trim().to_lowercase())
    }

    /// Stages where the agent may write to the project.
    pub fn writes(self) -> bool {
        self == Stage::Construir
    }
}

/// Request sizes, Brazilian t-shirt style. The size picks how each stage runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    Pp,
    P,
    M,
    G,
}

impl Size {
    pub const ALL: [Size; 4] = [Size::Pp, Size::P, Size::M, Size::G];

    pub fn id(self) -> &'static str {
        match self {
            Size::Pp => "pp",
            Size::P => "p",
            Size::M => "m",
            Size::G => "g",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Size::Pp => "PP · ajuste",
            Size::P => "P · pequeno",
            Size::M => "M · médio",
            Size::G => "G · grande",
        }
    }

    pub fn parse(text: &str) -> Option<Size> {
        Size::ALL.into_iter().find(|s| s.id() == text.trim().to_lowercase())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Actor {
    Agent,
    Dev,
    Passo,
}

impl Actor {
    pub fn label(self) -> &'static str {
        match self {
            Actor::Agent => "agente",
            Actor::Dev => "dev",
            Actor::Passo => "passo",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Active,
    Done,
    Aborted,
}

/// Whose move it is. Exactly one answer at any time: that is what makes the flow deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Turn {
    Agent,
    Dev,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GateKind {
    /// Approve the plan (and answer the refine questions).
    Aprovar,
    /// Accept the delivery. Every size has it; no configuration removes it.
    Aceitar,
    /// The agent asked something mid-run.
    Pergunta,
    /// Attempts or rounds ran out.
    Travado,
    /// The diff went past what the size allows.
    Limite,
    /// The run spent more tokens than the size allows.
    Tokens,
}

impl GateKind {
    pub fn label(self) -> &'static str {
        match self {
            GateKind::Aprovar => "aprovar o plano",
            GateKind::Aceitar => "aceitar a entrega",
            GateKind::Pergunta => "responder a pergunta",
            GateKind::Travado => "destravar",
            GateKind::Limite => "limite do tamanho",
            GateKind::Tokens => "limite de tokens",
        }
    }
}

/// A dev turn: nothing moves until the dev answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gate {
    pub kind: GateKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// Questions this gate answers (ids from `Run::questions`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<String>,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    #[default]
    Todo,
    Doing,
    Done,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// The test that proves this step; empty when the step has nothing to test (docs, config).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub test: String,
    /// Ids of the criteria this step meets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<String>,
    /// The command that runs this step's test; Passo runs it before the step checks.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub test_command: String,
    /// Done under an earlier plan: it stays, linked to no criterion.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub legacy: bool,
    /// Model tier for the current attempt when it differs from the agent's (the cheap one already failed here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    pub status: StepStatus,
    pub attempts: u32,
    /// Added by verify or by a rejected delivery, not by the planner.
    #[serde(default)]
    pub fix: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Files the step actually changed, as git saw them when it was done.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
    /// A review already covered this step.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reviewed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Criterion {
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    pub command: String,
    pub ok: bool,
    /// Tail of the output.
    pub output: String,
    pub ms: u64,
}

/// What the dev changed in the request, and when. The audit trail of intent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub at: DateTime<Utc>,
    pub by: Actor,
    pub kind: ChangeKind,
    pub stage: Stage,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// The step in progress when it happened (its files tell `passo learn` what the change was about).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    /// The dev picked an answer to a question.
    Resposta,
    /// The dev rejected the plan or the delivery.
    Recusa,
    /// The dev changed the request mid-run.
    Mudanca,
    /// The size changed.
    Tamanho,
    /// The plan changed after it was shown (a new version).
    Plano,
    /// A hint the dev gave to unblock the agent.
    Dica,
}

impl ChangeKind {
    pub fn label(self) -> &'static str {
        match self {
            ChangeKind::Resposta => "resposta",
            ChangeKind::Recusa => "recusa",
            ChangeKind::Mudanca => "mudança no pedido",
            ChangeKind::Tamanho => "tamanho",
            ChangeKind::Plano => "nova versão do plano",
            ChangeKind::Dica => "dica",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Commit the run started from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Branch the dev was on (delivery merges back into it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// How strong the proof behind a delivery is, from what Passo itself ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Proof {
    Forte,
    Media,
    Fraca,
    Nenhuma,
}

impl Proof {
    pub fn label(self) -> &'static str {
        match self {
            Proof::Forte => "forte",
            Proof::Media => "média",
            Proof::Fraca => "fraca",
            Proof::Nenhuma => "nenhuma",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub request: String,
    pub size: Size,
    pub stage: Stage,
    pub status: Status,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<Brief>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<Plan>,
    /// Plan versions shown so far.
    #[serde(default)]
    pub plan_version: u32,
    /// Fingerprint of the plan the dev approved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_plan: Option<String>,
    #[serde(default)]
    pub questions: Vec<Question>,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default)]
    pub criteria: Vec<Criterion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<Gate>,
    /// Feedback to show the agent on its next turn (a rejection, a change, a hint, a failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
    /// Decisions taken during the run, shown to the agent from then on.
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub rounds: u32,
    /// Last checks Passo ran.
    #[serde(default)]
    pub checks: Vec<CheckResult>,
    /// The final checks passed and the reviewer is next.
    #[serde(default)]
    pub reviewing: bool,
    /// The dev accepted going past the size limit.
    #[serde(default)]
    pub limit_accepted: bool,
    /// The final checks passed; Passo is measuring the diff coverage.
    #[serde(default)]
    pub awaiting_coverage: bool,
    /// The diff coverage of the last verify, when configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<crate::coverage::DiffCoverage>,
    /// Billable tokens the run spent, as last measured.
    #[serde(default)]
    pub tokens: u64,
    /// The dev accepted going past the size's token limit.
    #[serde(default)]
    pub tokens_accepted: bool,
    #[serde(default)]
    pub changes: Vec<Change>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Workspace>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof: Option<Proof>,
    /// The reviewer's word on the project map areas it was shown; applied to the map on delivery.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub map_checks: Vec<crate::spec::MapCheck>,
}

impl Run {
    pub fn new(id: impl Into<String>, request: impl Into<String>, size: Size, now: DateTime<Utc>) -> Run {
        Run {
            id: id.into(),
            request: request.into(),
            size,
            stage: Stage::Refinar,
            status: Status::Active,
            created_at: now,
            updated_at: now,
            brief: None,
            plan: None,
            plan_version: 0,
            approved_plan: None,
            questions: vec![],
            steps: vec![],
            criteria: vec![],
            gate: None,
            feedback: None,
            notes: vec![],
            rounds: 0,
            checks: vec![],
            reviewing: false,
            limit_accepted: false,
            awaiting_coverage: false,
            coverage: None,
            tokens: 0,
            tokens_accepted: false,
            changes: vec![],
            workspace: None,
            proof: None,
            map_checks: vec![],
        }
    }

    pub fn active(&self) -> bool {
        self.status == Status::Active
    }

    pub fn turn(&self) -> Turn {
        match (self.status, &self.gate) {
            (Status::Active, Some(_)) => Turn::Dev,
            (Status::Active, None) => Turn::Agent,
            _ => Turn::None,
        }
    }

    /// Steps done since the last review, once there was one: the next review has to read them. The first review
    /// covers everything through the criteria.
    pub fn unreviewed_steps(&self) -> Vec<&Step> {
        if !self.steps.iter().any(|s| s.reviewed) {
            return vec![];
        }
        self.steps
            .iter()
            .filter(|s| s.status == StepStatus::Done && !s.reviewed)
            .collect()
    }

    pub fn current_step(&self) -> Option<&Step> {
        self.steps.iter().find(|s| s.status == StepStatus::Doing)
    }

    pub fn current_step_mut(&mut self) -> Option<&mut Step> {
        self.steps.iter_mut().find(|s| s.status == StepStatus::Doing)
    }

    pub fn open_questions(&self) -> Vec<&Question> {
        self.questions.iter().filter(|q| q.answer.is_none()).collect()
    }

    /// One line on where the run is, for status bars and hooks.
    pub fn headline(&self) -> String {
        let place = match (self.stage, self.current_step()) {
            (Stage::Construir, Some(step)) => {
                let index = self.steps.iter().position(|s| s.id == step.id).unwrap_or(0) + 1;
                format!("{} · passo {}/{}", self.stage.label(), index, self.steps.len())
            }
            (stage, _) => stage.label().to_string(),
        };
        let turn = match self.turn() {
            Turn::Agent => "vez do agente".to_string(),
            Turn::Dev => format!(
                "vez do dev: {}",
                self.gate.as_ref().map(|g| g.kind.label()).unwrap_or("")
            ),
            Turn::None if self.status == Status::Done => "concluído".to_string(),
            Turn::None => "interrompido".to_string(),
        };
        format!("{} · {} · {}", self.id, place, turn)
    }
}

/// `412k`, `1,2M`: how tokens read in the terminal and the panel.
pub fn tokens_label(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6).replace('.', ",")
    } else {
        format!("{}k", (n + 500) / 1000)
    }
}
