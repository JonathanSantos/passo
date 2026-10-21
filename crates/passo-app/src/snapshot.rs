//! Everything the dashboard and the report show, in one serializable value.

use chrono::{DateTime, Utc};
use passo_core::{Mode, Run, Size, Stage, Status, Turn};
use passo_store::Event;
use serde::Serialize;

use crate::metrics::Metrics;

#[derive(Debug, Clone, Serialize)]
pub struct RunSummary {
    pub id: String,
    pub request: String,
    pub size: Size,
    pub stage: Stage,
    pub status: Status,
    pub turn: Turn,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&Run> for RunSummary {
    fn from(run: &Run) -> Self {
        RunSummary {
            id: run.id.clone(),
            request: run.request.clone(),
            size: run.size,
            stage: run.stage,
            status: run.status,
            turn: run.turn(),
            created_at: run.created_at,
            updated_at: run.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StageInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub mode: Mode,
    pub agent: Option<String>,
    /// Model tier the stage's agent asks for.
    pub tier: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub generated_at: DateTime<Utc>,
    pub project: String,
    pub run: Option<Run>,
    pub events: Vec<Event>,
    pub metrics: Option<Metrics>,
    pub runs: Vec<RunSummary>,
    pub stages: Vec<StageInfo>,
    pub rail: Option<String>,
    /// What the dev reads at the open gate.
    pub gate_text: Option<String>,
    /// What the agent reads on `passo next`.
    pub next_text: Option<String>,
    /// `Ok(n events)` or where the chain broke.
    pub audit: Option<Result<usize, String>>,
    pub platforms: Vec<String>,
    pub checks_step: Vec<String>,
    pub checks_final: Vec<String>,
}
