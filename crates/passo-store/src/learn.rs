//! `passo learn` on disk: the task in progress (out of git) and the lessons already decided on (in git).

use anyhow::Result;
use passo_core::learn::Lesson;
use passo_core::spec::Proposal;

use crate::layout::{Layout, write_atomic};

/// A learning in progress: the lessons shown to the agent and, once delivered, the proposals the dev decides on.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LearnTask {
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub lessons: Vec<Lesson>,
    #[serde(default)]
    pub proposals: Vec<Proposal>,
    /// Times the stop hook kept the agent going.
    #[serde(default)]
    pub stops: u32,
}

impl LearnTask {
    /// Delivered: waiting on the dev, not on the agent.
    pub fn delivered(&self) -> bool {
        !self.proposals.is_empty()
    }
}

pub fn load_task(layout: &Layout) -> Option<LearnTask> {
    std::fs::read_to_string(layout.learn_task())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

pub fn save_task(layout: &Layout, task: &LearnTask) -> Result<()> {
    write_atomic(&layout.learn_task(), serde_json::to_string_pretty(task)?.as_bytes())?;
    Ok(())
}

pub fn remove_task(layout: &Layout) {
    std::fs::remove_file(layout.learn_task()).ok();
}

pub fn load_seen(layout: &Layout) -> Vec<String> {
    std::fs::read_to_string(layout.learn_seen())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Adds fingerprints to `seen.json`, keeping the order and leaving out repeats.
pub fn add_seen(layout: &Layout, prints: &[String]) -> Result<()> {
    let mut seen = load_seen(layout);
    for print in prints {
        if !seen.contains(print) {
            seen.push(print.clone());
        }
    }
    write_atomic(&layout.learn_seen(), serde_json::to_string_pretty(&seen)?.as_bytes())?;
    Ok(())
}
