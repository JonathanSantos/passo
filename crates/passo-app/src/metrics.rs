//! Metrics, computed from the audit log and the transcripts. Nothing is counted twice: the log is the source.
//!
//! Two numbers say whether Passo is worth it: how much the dev had to step in (`dev_turns`, `dev_wait_ms`) and how
//! much the agents spent (`tokens`, per stage and model).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use passo_adapters::Usage;
use passo_core::{Actor, Run, Stage};
use passo_store::Event;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub calls: u32,
}

impl Tokens {
    pub fn add(&mut self, usage: &Usage) {
        self.input += usage.input;
        self.output += usage.output;
        self.cache_read += usage.cache_read;
        self.cache_write += usage.cache_write;
        self.calls += 1;
    }

    /// Tokens that cost like fresh input or output (cache reads are cheap and counted apart).
    pub fn billable(&self) -> u64 {
        self.input + self.output + self.cache_write
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StageMetrics {
    pub stage: Option<Stage>,
    pub ms: i64,
    pub visits: u32,
    pub tokens: Tokens,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    pub total_ms: i64,
    /// Time the run waited on the dev (gates open).
    pub dev_wait_ms: i64,
    /// Times the dev had to decide something.
    pub dev_turns: u32,
    pub stages: Vec<StageMetrics>,
    pub steps_done: u32,
    pub steps_failed: u32,
    pub rounds: u32,
    pub plan_versions: u32,
    pub changes: u32,
    pub tool_calls: u32,
    pub lock_denied: u32,
    pub invalid_outputs: u32,
    pub tokens: Tokens,
    pub by_model: BTreeMap<String, Tokens>,
}

pub fn compute(run: &Run, events: &[Event], usage: &[Usage], now: DateTime<Utc>) -> Metrics {
    let end = if run.active() { now } else { run.updated_at };
    let mut m = Metrics {
        total_ms: (end - run.created_at).num_milliseconds().max(0),
        rounds: run.rounds,
        plan_versions: run.plan_version,
        changes: run.changes.len() as u32,
        stages: Stage::ALL
            .iter()
            .map(|s| StageMetrics {
                stage: Some(*s),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };

    // Stage windows: from each `stage.entered` to the next one (or the end).
    let entries: Vec<(DateTime<Utc>, Stage)> = events
        .iter()
        .filter(|e| e.kind == "stage.entered")
        .map(|e| (e.t, e.stage))
        .collect();
    let window = |t: DateTime<Utc>| entries.iter().rev().find(|(start, _)| *start <= t).map(|(_, s)| *s);
    for (i, (start, stage)) in entries.iter().enumerate() {
        let stop = entries.get(i + 1).map(|(t, _)| *t).unwrap_or(end);
        let slot = &mut m.stages[stage.index()];
        slot.ms += (stop - *start).num_milliseconds().max(0);
        slot.visits += 1;
    }

    // Dev turns: from `gate.opened` to the dev's next move.
    let mut open: Option<DateTime<Utc>> = None;
    for event in events {
        match (event.kind.as_str(), event.actor) {
            ("gate.opened", _) => open = Some(event.t),
            (_, Actor::Dev) => {
                if let Some(start) = open.take() {
                    m.dev_turns += 1;
                    m.dev_wait_ms += (event.t - start).num_milliseconds().max(0);
                }
            }
            _ => {}
        }
        match event.kind.as_str() {
            "step.done" => m.steps_done += 1,
            "step.failed" => m.steps_failed += 1,
            "tool" => m.tool_calls += 1,
            "lock.denied" => m.lock_denied += 1,
            "output.invalid" => m.invalid_outputs += 1,
            _ => {}
        }
    }
    if let Some(start) = open
        && run.active()
    {
        m.dev_wait_ms += (now - start).num_milliseconds().max(0);
    }

    for sample in usage.iter().filter(|u| u.t >= run.created_at && u.t <= end) {
        m.tokens.add(sample);
        m.by_model.entry(sample.model.clone()).or_default().add(sample);
        if let Some(stage) = window(sample.t) {
            m.stages[stage.index()].tokens.add(sample);
        }
    }
    m
}

pub fn duration(ms: i64) -> String {
    let secs = ms / 1000;
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}min {}s", secs / 60, secs % 60),
        _ => format!("{}h {}min", secs / 3600, (secs % 3600) / 60),
    }
}

pub fn count(n: u64) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..1_000_000 => format!("{:.1}k", n as f64 / 1000.0),
        _ => format!("{:.2}M", n as f64 / 1_000_000.0),
    }
}
