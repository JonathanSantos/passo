//! Metrics, computed from the audit log and the transcripts. Nothing is counted twice: the log is the source.
//!
//! Two numbers say whether Passo is worth it: how much the dev had to step in (`dev_turns`, `dev_wait_ms`) and how
//! much the agents spent (`tokens`, per stage and model).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use passo_adapters::Usage;
use passo_core::model::Proof;
use passo_core::{Actor, Run, Size, Stage, Status};
use passo_store::Event;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::snapshot::RunSummary;

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
    /// Steps that moved to the strong tier after the cheap one failed.
    pub escalations: u32,
    /// Agent tool calls per build step.
    pub tools_per_step: BTreeMap<String, u32>,
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
            "tool" => {
                m.tool_calls += 1;
                if let Some(step) = event.data.get("step").and_then(Value::as_str) {
                    *m.tools_per_step.entry(step.to_string()).or_default() += 1;
                }
            }
            "lock.denied" => m.lock_denied += 1,
            "output.invalid" => m.invalid_outputs += 1,
            "step.escalated" => m.escalations += 1,
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

/// One run in the series.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunRow {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub size: Size,
    pub status: Status,
    pub proof: Option<Proof>,
    pub dev_turns: u32,
    pub dev_wait_ms: i64,
    pub steps_done: u32,
    pub steps_failed: u32,
    pub escalations: u32,
    /// Billable tokens; `None` when no transcript was read.
    pub tokens: Option<u64>,
    pub total_ms: i64,
    pub accepted_first: bool,
}

/// Median and 90th percentile (nearest rank).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Spread {
    pub median: f64,
    pub p90: f64,
}

impl Spread {
    pub fn of(values: &[f64]) -> Option<Spread> {
        if values.is_empty() {
            return None;
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len();
        let median = if n % 2 == 1 {
            sorted[n / 2]
        } else {
            (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
        };
        let rank = (0.9 * n as f64).ceil() as usize;
        Some(Spread {
            median,
            p90: sorted[rank.clamp(1, n) - 1],
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SizeSummary {
    pub size: Size,
    pub runs: usize,
    pub dev_turns: Option<Spread>,
    pub tokens: Option<Spread>,
    pub total_ms: Option<Spread>,
    /// Of the runs that ended delivered, the share accepted on the first delivery (0 to 1).
    pub accepted_first: Option<f64>,
}

/// One measure, the last five runs against the five before.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrendLine {
    pub measure: &'static str,
    pub before: f64,
    pub recent: f64,
    /// `↓` went down, `↑` went up, `=` stayed (for all three, down is better).
    pub arrow: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Aggregate {
    pub runs: Vec<RunRow>,
    pub by_size: Vec<SizeSummary>,
    pub trend: Vec<TrendLine>,
}

const TREND_WINDOW: usize = 5;

/// Reads one measure off a run, when it has it.
type Measure = fn(&RunRow) -> Option<f64>;

/// The series: one row per run (oldest first), a summary per size and the trend. Pure: no disk, no clock.
pub fn aggregate(runs: &[(RunSummary, Metrics)]) -> Aggregate {
    let mut rows: Vec<RunRow> = runs
        .iter()
        .map(|(run, m)| RunRow {
            id: run.id.clone(),
            created_at: run.created_at,
            size: run.size,
            status: run.status,
            proof: run.proof,
            dev_turns: m.dev_turns,
            dev_wait_ms: m.dev_wait_ms,
            steps_done: m.steps_done,
            steps_failed: m.steps_failed,
            escalations: m.escalations,
            tokens: (m.tokens.calls > 0).then(|| m.tokens.billable()),
            total_ms: m.total_ms,
            accepted_first: run.accepted_first,
        })
        .collect();
    rows.sort_by_key(|r| r.created_at);

    let by_size = Size::ALL
        .iter()
        .filter_map(|&size| {
            let of: Vec<&RunRow> = rows.iter().filter(|r| r.size == size).collect();
            if of.is_empty() {
                return None;
            }
            let delivered: Vec<&&RunRow> = of.iter().filter(|r| r.status == Status::Done).collect();
            Some(SizeSummary {
                size,
                runs: of.len(),
                dev_turns: Spread::of(&of.iter().map(|r| r.dev_turns as f64).collect::<Vec<_>>()),
                tokens: Spread::of(&of.iter().filter_map(|r| r.tokens.map(|t| t as f64)).collect::<Vec<_>>()),
                total_ms: Spread::of(&of.iter().map(|r| r.total_ms as f64).collect::<Vec<_>>()),
                accepted_first: (!delivered.is_empty())
                    .then(|| delivered.iter().filter(|r| r.accepted_first).count() as f64 / delivered.len() as f64),
            })
        })
        .collect();

    let split = rows.len().saturating_sub(TREND_WINDOW);
    let (older, recent) = rows.split_at(split);
    let before = &older[older.len().saturating_sub(TREND_WINDOW)..];
    let mut trend = vec![];
    if !before.is_empty() {
        let measures: [(&'static str, Measure); 3] = [
            ("vezes do dev", |r| Some(r.dev_turns as f64)),
            ("tokens", |r| r.tokens.map(|t| t as f64)),
            ("tempo", |r| Some(r.total_ms as f64)),
        ];
        for (measure, value) in measures {
            let median =
                |runs: &[RunRow]| Spread::of(&runs.iter().filter_map(value).collect::<Vec<_>>()).map(|s| s.median);
            if let (Some(before), Some(recent)) = (median(before), median(recent)) {
                let arrow = if recent < before {
                    "↓"
                } else if recent > before {
                    "↑"
                } else {
                    "="
                };
                trend.push(TrendLine {
                    measure,
                    before,
                    recent,
                    arrow,
                });
            }
        }
    }
    Aggregate {
        runs: rows,
        by_size,
        trend,
    }
}

/// `2026-10-01` as the start of that day, in UTC.
pub fn day(text: &str) -> Option<DateTime<Utc>> {
    chrono::NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|d| d.and_utc())
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn run(day: u32, size: Size, dev_turns: u32, tokens: u64, minutes: i64, first: bool) -> (RunSummary, Metrics) {
        let at = Utc.with_ymd_and_hms(2026, 10, day, 12, 0, 0).unwrap();
        let summary = RunSummary {
            id: format!("r{day}"),
            request: "x".into(),
            size,
            stage: Stage::Entregar,
            status: Status::Done,
            turn: passo_core::Turn::None,
            created_at: at,
            updated_at: at,
            proof: Some(Proof::Forte),
            accepted_first: first,
        };
        let metrics = Metrics {
            dev_turns,
            total_ms: minutes * 60_000,
            tokens: Tokens {
                input: tokens,
                calls: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        (summary, metrics)
    }

    #[test]
    fn median_p90_and_the_trend_per_size() {
        // Four m runs and two p runs; the last runs need the dev less.
        let runs = vec![
            run(1, Size::M, 5, 400_000, 30, false),
            run(2, Size::M, 4, 300_000, 20, true),
            run(3, Size::P, 3, 100_000, 10, true),
            run(4, Size::M, 2, 200_000, 15, true),
            run(5, Size::M, 1, 100_000, 10, true),
            run(6, Size::P, 1, 50_000, 5, true),
        ];
        let agg = aggregate(&runs);
        assert_eq!(agg.runs.len(), 6);
        let m = agg.by_size.iter().find(|s| s.size == Size::M).unwrap();
        assert_eq!(m.runs, 4);
        assert_eq!(m.dev_turns, Some(Spread { median: 3.0, p90: 5.0 }));
        assert_eq!(m.tokens.unwrap().median, 250_000.0);
        assert_eq!(m.accepted_first, Some(0.75));
        let p = agg.by_size.iter().find(|s| s.size == Size::P).unwrap();
        assert_eq!(p.dev_turns, Some(Spread { median: 2.0, p90: 3.0 }));

        // The last five (runs 2 to 6) against the one before (run 1): everything went down.
        let dev = agg.trend.iter().find(|t| t.measure == "vezes do dev").unwrap();
        assert_eq!((dev.before, dev.recent, dev.arrow), (5.0, 2.0, "↓"));
        assert!(agg.trend.iter().all(|t| t.arrow == "↓"), "{:?}", agg.trend);
    }

    #[test]
    fn five_runs_or_fewer_have_no_trend() {
        let runs: Vec<_> = (1..=5).map(|d| run(d, Size::P, 1, 1, 1, true)).collect();
        assert!(aggregate(&runs).trend.is_empty());
    }
}
