//! Runs on disk: the run itself, the active pointer, plan versions and the transcripts hooks report.

use std::collections::BTreeMap;
use std::path::PathBuf;

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
        let current = std::fs::read_to_string(&ignore).unwrap_or_default();
        let mut text = if current.is_empty() {
            "runs/\ncurrent\n*.lock\n".to_string()
        } else {
            current.clone()
        };
        for line in ["map/.task.json", "hook-errors.log"] {
            if !text.lines().any(|l| l.trim() == line) {
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(line);
                text.push('\n');
            }
        }
        if text != current {
            std::fs::write(ignore, text)?;
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

    /// Copies the run's log and plan versions to `.passo/audit/<id>/`, the folder that goes into git. Returns the
    /// folder, relative to the project root.
    pub fn archive(&self, id: &str) -> Result<PathBuf> {
        let dir = self.layout.audit_dir(id);
        std::fs::create_dir_all(&dir)?;
        let mut files = vec![self.layout.events(id)];
        if let Ok(entries) = std::fs::read_dir(self.layout.run_dir(id)) {
            files.extend(entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("plan-v"))
            }));
        }
        for file in files.iter().filter(|f| f.is_file()) {
            let name = file.file_name().context("nome do arquivo")?;
            std::fs::copy(file, dir.join(name))?;
        }
        Ok(dir.strip_prefix(&self.layout.root).unwrap_or(&dir).to_path_buf())
    }

    /// Counts how many times the stop hook blocked the agent at the same spot.
    pub fn count_stop(&self, id: &str, key: &str) -> Result<u32> {
        bump(&self.layout.stops(id), key)
    }

    /// Counts a tool call for the action budget; `key` names the step and attempt (or the stage).
    pub fn count_tool(&self, id: &str, key: &str) -> Result<u32> {
        bump(&self.layout.budget(id), key)
    }
}

/// Adds one to a counter in a small JSON map. Hooks run in parallel, so the update holds a file lock.
fn bump(path: &std::path::Path, key: &str) -> Result<u32> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock = std::fs::File::create(path.with_extension("lock"))?;
    lock.lock().context("trava do contador")?;
    let mut counts: BTreeMap<String, u32> = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let count = counts.entry(key.to_string()).or_insert(0);
    *count += 1;
    let value = *count;
    write_atomic(path, serde_json::to_string(&counts)?.as_bytes())?;
    Ok(value)
}
