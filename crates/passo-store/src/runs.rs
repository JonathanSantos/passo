//! Runs on disk: the run itself, the active pointer, plan versions and the transcripts hooks report.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use passo_core::Run;
use serde::{Deserialize, Serialize};

use crate::audit::EventLog;
use crate::layout::{Layout, write_atomic};

/// An agent transcript a hook reported, read later for token usage.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Transcript {
    pub platform: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct Store {
    pub layout: Layout,
}

impl Store {
    pub fn new(layout: Layout) -> Self {
        Store { layout }
    }

    /// Creates `.passo/` and its `.gitignore` (runs stay out of the project's history).
    pub fn init(&self) -> Result<()> {
        std::fs::create_dir_all(self.layout.runs())?;
        let ignore = self.layout.passo().join(".gitignore");
        if !ignore.exists() {
            std::fs::write(ignore, "runs/\ncurrent\n*.lock\n")?;
        }
        Ok(())
    }

    pub fn current_id(&self) -> Option<String> {
        std::fs::read_to_string(self.layout.current())
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    pub fn set_current(&self, id: Option<&str>) -> Result<()> {
        match id {
            Some(id) => write_atomic(&self.layout.current(), id.as_bytes())?,
            None => {
                std::fs::remove_file(self.layout.current()).ok();
            }
        }
        Ok(())
    }

    pub fn load(&self, id: &str) -> Result<Run> {
        let path = self.layout.run_file(id);
        let text = std::fs::read_to_string(&path).with_context(|| format!("run {id} não encontrado"))?;
        serde_json::from_str(&text).with_context(|| format!("run {id} ilegível"))
    }

    pub fn save(&self, run: &Run) -> Result<()> {
        write_atomic(
            &self.layout.run_file(&run.id),
            serde_json::to_string_pretty(run)?.as_bytes(),
        )?;
        if let Some(plan) = &run.plan {
            let path = self.layout.plan_version(&run.id, run.plan_version);
            if !path.exists() {
                write_atomic(&path, serde_json::to_string_pretty(plan)?.as_bytes())?;
            }
        }
        Ok(())
    }

    /// The run the project is working on, if any (finished runs are not active).
    pub fn active(&self) -> Option<Run> {
        let run = self.load(&self.current_id()?).ok()?;
        run.active().then_some(run)
    }

    /// The active run, or the latest one: what `status` and the dashboard show.
    pub fn latest(&self) -> Option<Run> {
        self.current_id()
            .and_then(|id| self.load(&id).ok())
            .or_else(|| self.list().into_iter().next())
    }

    /// All runs, newest first.
    pub fn list(&self) -> Vec<Run> {
        let Ok(entries) = std::fs::read_dir(self.layout.runs()) else {
            return vec![];
        };
        let mut runs: Vec<Run> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter_map(|id| self.load(&id).ok())
            .collect();
        runs.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        runs
    }

    pub fn log(&self, id: &str) -> EventLog {
        EventLog::new(self.layout.events(id))
    }

    pub fn transcripts(&self, id: &str) -> Vec<Transcript> {
        std::fs::read_to_string(self.layout.sessions(id))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn add_transcript(&self, id: &str, transcript: Transcript) -> Result<()> {
        let mut all = self.transcripts(id);
        if all.contains(&transcript) {
            return Ok(());
        }
        all.push(transcript);
        write_atomic(
            &self.layout.sessions(id),
            serde_json::to_string_pretty(&all)?.as_bytes(),
        )?;
        Ok(())
    }

    /// Counts how many times the stop hook blocked the agent at the same spot.
    pub fn count_stop(&self, id: &str, key: &str) -> Result<u32> {
        let path = self.layout.stops(id);
        let mut counts: BTreeMap<String, u32> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let count = counts.entry(key.to_string()).or_insert(0);
        *count += 1;
        let value = *count;
        write_atomic(&path, serde_json::to_string(&counts)?.as_bytes())?;
        Ok(value)
    }
}
