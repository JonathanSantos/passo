//! The outside world, behind small traits so the service can be tested: the clock, shell checks and git.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use passo_core::CheckResult;

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

pub trait CheckRunner: Send + Sync {
    fn run(&self, command: &str, dir: &Path, timeout: Duration) -> CheckResult;
}

/// Runs checks with `sh -c`, keeping the tail of the output.
pub struct ShellRunner;

const TAIL_LINES: usize = 40;

impl CheckRunner for ShellRunner {
    fn run(&self, command: &str, dir: &Path, timeout: Duration) -> CheckResult {
        let started = Instant::now();
        let child = Command::new("sh")
            .arg("-c")
            .arg(format!("{command} 2>&1"))
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(e) => {
                return CheckResult {
                    command: command.into(),
                    ok: false,
                    output: e.to_string(),
                    ms: 0,
                };
            }
        };
        let mut stdout = child.stdout.take().expect("piped stdout");
        let reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            stdout.read_to_end(&mut buf).ok();
            buf
        });
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if started.elapsed() > timeout => {
                    child.kill().ok();
                    child.wait().ok();
                    break None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(30)),
                Err(_) => break None,
            }
        };
        let output = String::from_utf8_lossy(&reader.join().unwrap_or_default()).to_string();
        let lines: Vec<&str> = output.lines().collect();
        let mut tail = lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n");
        if status.is_none() {
            tail.push_str(&format!("\n(passou do tempo limite de {}s)", timeout.as_secs()));
        }
        CheckResult {
            command: command.into(),
            ok: status.is_some_and(|s| s.success()),
            output: tail,
            ms: started.elapsed().as_millis() as u64,
        }
    }
}

/// Git, through the `git` binary. Passo is the only one who commits during a run.
#[derive(Debug, Clone)]
pub struct Git {
    pub dir: PathBuf,
}

impl Git {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Git { dir: dir.into() }
    }

    fn output(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.dir)
            .stdin(Stdio::null())
            .output()
            .context("rodando git")?;
        if !out.status.success() {
            bail!(
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    }

    pub fn is_repo(&self) -> bool {
        self.output(&["rev-parse", "--is-inside-work-tree"])
            .is_ok_and(|o| o == "true")
    }

    pub fn head(&self) -> Option<String> {
        self.output(&["rev-parse", "HEAD"]).ok()
    }

    pub fn branch(&self) -> Option<String> {
        self.output(&["rev-parse", "--abbrev-ref", "HEAD"])
            .ok()
            .filter(|b| b != "HEAD")
    }

    /// Changed paths, ignoring Passo's own folder and the agent platform files `passo init` writes.
    pub fn dirty(&self) -> Result<Vec<String>> {
        let out = self.output(&["status", "--porcelain", "--untracked-files=all"])?;
        Ok(out
            .lines()
            .filter_map(|l| l.get(3..))
            .map(|p| p.trim_matches('"').to_string())
            .filter(|p| !p.starts_with(".passo/"))
            .collect())
    }

    pub fn create_branch(&self, name: &str) -> Result<()> {
        self.output(&["switch", "-c", name]).map(|_| ())
    }

    pub fn switch(&self, name: &str) -> Result<()> {
        self.output(&["switch", name]).map(|_| ())
    }

    pub fn stage_all(&self) -> Result<()> {
        self.output(&["add", "-A"]).map(|_| ())
    }

    /// Files staged against HEAD: what this step changed.
    pub fn staged_files(&self) -> Result<Vec<String>> {
        Ok(self
            .output(&["diff", "--cached", "--name-only", "HEAD"])?
            .lines()
            .map(str::to_string)
            .collect())
    }

    /// `(path, added + removed)` for everything staged against the run's base.
    pub fn numstat(&self, base: &str) -> Result<Vec<(String, usize)>> {
        Ok(self
            .output(&["diff", "--cached", "--numstat", base])?
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(3, '\t');
                let added = parts.next()?.parse::<usize>().unwrap_or(0);
                let removed = parts.next()?.parse::<usize>().unwrap_or(0);
                Some((parts.next()?.to_string(), added + removed))
            })
            .collect())
    }

    pub fn commit(&self, message: &str) -> Result<Option<String>> {
        if self.output(&["diff", "--cached", "--quiet"]).is_ok() {
            return Ok(None);
        }
        self.output(&["commit", "-q", "--no-verify", "-m", message])?;
        Ok(self.head())
    }

    pub fn merge(&self, branch: &str, message: &str) -> Result<()> {
        if let Err(e) = self.output(&["merge", "--no-ff", "-q", "-m", message, branch]) {
            self.output(&["merge", "--abort"]).ok();
            return Err(e);
        }
        Ok(())
    }
}
