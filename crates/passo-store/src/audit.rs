//! The audit log: one JSON line per event, each chained to the previous one by SHA-256.
//!
//! Editing or deleting a past line breaks the chain, and `verify` says where. Hooks and the CLI append at the same
//! time, so every append takes a file lock.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use passo_core::{Actor, EventDraft, Stage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub t: DateTime<Utc>,
    pub run: String,
    pub stage: Stage,
    pub actor: Actor,
    pub kind: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
    pub prev: String,
    pub hash: String,
}

impl Event {
    fn digest(&self) -> String {
        let body = serde_json::json!({
            "seq": self.seq, "t": self.t, "run": self.run, "stage": self.stage, "actor": self.actor,
            "kind": self.kind, "text": self.text, "data": self.data, "prev": self.prev,
        });
        let mut hasher = Sha256::new();
        hasher.update(body.to_string().as_bytes());
        hex(&hasher.finalize())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub struct EventLog {
    path: PathBuf,
}

/// Bytes read from the end of the file to find the last line; grows when a line is longer.
const TAIL: u64 = 64 * 1024;

impl EventLog {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        EventLog { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Stamps and appends events, chaining each to the last line on disk.
    pub fn append(&self, run: &str, drafts: &[EventDraft], now: DateTime<Utc>) -> Result<Vec<Event>> {
        if drafts.is_empty() {
            return Ok(vec![]);
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let lock = File::create(self.path.with_extension("lock"))?;
        lock.lock().context("trava do log de eventos")?;
        let last = self.last()?;
        let (mut seq, mut prev) = last.map(|e| (e.seq, e.hash)).unwrap_or((0, "0".repeat(64)));
        let mut file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        let mut written = vec![];
        for draft in drafts {
            seq += 1;
            let mut event = Event {
                seq,
                t: now,
                run: run.to_string(),
                stage: draft.stage,
                actor: draft.actor,
                kind: draft.kind.to_string(),
                text: draft.text.clone(),
                data: if draft.data.as_object().is_some_and(|o| o.is_empty()) {
                    Value::Null
                } else {
                    draft.data.clone()
                },
                prev: prev.clone(),
                hash: String::new(),
            };
            event.hash = event.digest();
            prev = event.hash.clone();
            writeln!(file, "{}", serde_json::to_string(&event)?)?;
            written.push(event);
        }
        file.flush()?;
        drop(lock);
        Ok(written)
    }

    /// The last event, read from the tail of the file: hooks append on every tool call, so this stays cheap.
    pub fn last(&self) -> Result<Option<Event>> {
        let Ok(mut file) = File::open(&self.path) else {
            return Ok(None);
        };
        let len = file.metadata()?.len();
        let mut window = TAIL;
        loop {
            let start = len.saturating_sub(window);
            file.seek(SeekFrom::Start(start))?;
            let mut bytes = Vec::with_capacity((len - start) as usize);
            file.read_to_end(&mut bytes)?;
            let mut lines = bytes
                .split(|b| *b == b'\n')
                .rev()
                .filter(|l| !l.iter().all(u8::is_ascii_whitespace));
            let Some(line) = lines.next() else {
                return Ok(None);
            };
            // The last line is whole when the window holds a line before it, or the window is the whole file.
            if start == 0 || lines.next().is_some() {
                return Ok(Some(
                    serde_json::from_slice(line).context("linha inválida no log de eventos")?,
                ));
            }
            window *= 2;
        }
    }

    pub fn read(&self) -> Result<Vec<Event>> {
        let Ok(file) = File::open(&self.path) else {
            return Ok(vec![]);
        };
        let mut events = vec![];
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            events.push(serde_json::from_str(&line).context("linha inválida no log de eventos")?);
        }
        Ok(events)
    }

    /// Checks the chain. `Ok(n)` with the number of events, or the first broken sequence number and why.
    pub fn verify(&self) -> Result<std::result::Result<usize, (u64, String)>> {
        let events = self.read()?;
        let mut prev = "0".repeat(64);
        for (i, event) in events.iter().enumerate() {
            if event.seq != i as u64 + 1 {
                return Ok(Err((
                    event.seq,
                    format!("sequência esperada {}, achada {}", i + 1, event.seq),
                )));
            }
            if event.prev != prev {
                return Ok(Err((
                    event.seq,
                    "não aponta para o evento anterior (linha apagada ou fora de ordem)".into(),
                )));
            }
            if event.digest() != event.hash {
                return Ok(Err((event.seq, "conteúdo alterado depois de gravado".into())));
            }
            prev = event.hash.clone();
        }
        Ok(Ok(events.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(text: &str) -> EventDraft {
        EventDraft {
            kind: "x",
            actor: Actor::Passo,
            stage: Stage::Refinar,
            text: text.into(),
            data: serde_json::json!({}),
        }
    }

    #[test]
    fn chain_detects_tampering() {
        let dir = std::env::temp_dir().join(format!("passo-audit-{}", std::process::id()));
        let log = EventLog::new(dir.join("events.jsonl"));
        log.append("r", &[draft("a"), draft("b")], Utc::now()).unwrap();
        log.append("r", &[draft("c")], Utc::now()).unwrap();
        assert_eq!(log.verify().unwrap(), Ok(3));
        assert_eq!(log.last().unwrap().unwrap().seq, 3);
        let text = std::fs::read_to_string(log.path()).unwrap().replace("\"b\"", "\"B\"");
        std::fs::write(log.path(), text).unwrap();
        assert!(matches!(log.verify().unwrap(), Err((2, _))));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn last_line_is_found_past_the_tail_window() {
        let dir = std::env::temp_dir().join(format!("passo-audit-tail-{}", std::process::id()));
        let log = EventLog::new(dir.join("events.jsonl"));
        let long = "x".repeat(3 * TAIL as usize);
        log.append("r", &[draft("a"), draft(&long)], Utc::now()).unwrap();
        assert_eq!(log.last().unwrap().unwrap().seq, 2);
        log.append("r", &[draft("c")], Utc::now()).unwrap();
        assert_eq!(log.verify().unwrap(), Ok(3));
        std::fs::remove_dir_all(dir).ok();
    }
}
