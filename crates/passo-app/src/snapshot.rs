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
    /// Project map areas and libs the agent was shown on its current turn.
    pub map_nodes: Vec<String>,
    /// Hooks failed since the run began.
    pub hook_warning: Option<String>,
}

impl Snapshot {
    /// The same snapshot with `redact` applied to every event text and data, every check output, the feedback and
    /// the rendered texts.
    pub fn redacted(mut self, redact: impl Fn(&str) -> String) -> Snapshot {
        fn scrub(value: &mut serde_json::Value, redact: &dyn Fn(&str) -> String) {
            match value {
                serde_json::Value::String(s) => *s = redact(s),
                serde_json::Value::Array(items) => items.iter_mut().for_each(|v| scrub(v, redact)),
                serde_json::Value::Object(map) => map.values_mut().for_each(|v| scrub(v, redact)),
                _ => {}
            }
        }
        for event in &mut self.events {
            event.text = redact(&event.text);
            scrub(&mut event.data, &redact);
        }
        if let Some(run) = &mut self.run {
            for check in &mut run.checks {
                check.command = redact(&check.command);
                check.output = redact(&check.output);
            }
            run.feedback = run.feedback.as_deref().map(&redact);
            run.notes = run.notes.iter().map(|n| redact(n)).collect();
        }
        self.checks_step = self.checks_step.iter().map(|c| redact(c)).collect();
        self.checks_final = self.checks_final.iter().map(|c| redact(c)).collect();
        self.next_text = self.next_text.as_deref().map(&redact);
        self.gate_text = self.gate_text.as_deref().map(&redact);
        self
    }
}
