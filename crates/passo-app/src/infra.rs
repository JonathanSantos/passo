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
            let stderr = passo_core::redact::redact(String::from_utf8_lossy(&out.stderr).trim());
            bail!("git {}: {stderr}", args.join(" "));
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

    /// When a commit was made, in seconds since the epoch.
    pub fn commit_time(&self, commit: &str) -> Option<i64> {
        self.output(&["show", "-s", "--format=%ct", commit])
            .ok()?
            .trim()
            .parse()
            .ok()
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
    /// Files git tracks, relative to the repository root.
    pub fn tracked_files(&self) -> Result<Vec<String>> {
        Ok(self.output(&["ls-files"])?.lines().map(str::to_string).collect())
    }

    pub fn staged_files(&self) -> Result<Vec<String>> {
        Ok(self
            .output(&["diff", "--cached", "--name-only", "HEAD"])?
            .lines()
            .map(str::to_string)
            .collect())
    }

    /// Files under `paths` (globs) that changed between `from` and HEAD. An unknown commit is an error.
    pub fn changed_between(&self, from: &str, paths: &[String]) -> Result<Vec<String>> {
        let mut args = vec![
            "diff".to_string(),
            "--name-only".into(),
            from.to_string(),
            "HEAD".into(),
            "--".into(),
        ];
        args.extend(paths.iter().map(|p| format!(":(glob){p}")));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        Ok(self.output(&args)?.lines().map(str::to_string).collect())
    }

    /// Top-level names in a commit's tree.
    pub fn top_level(&self, commit: &str) -> Result<Vec<String>> {
        Ok(self
            .output(&["ls-tree", "--name-only", commit])?
            .lines()
            .map(str::to_string)
            .collect())
    }

    /// Files that differ between `base` and the working tree (committed or not).
    pub fn changed_since(&self, base: &str) -> Result<Vec<String>> {
        Ok(self
            .output(&["diff", "--name-only", base])?
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

    /// Commits only the given paths (staged or not), leaving the rest of the index as it is.
    pub fn commit_paths(&self, message: &str, paths: &[&str]) -> Result<Option<String>> {
        let mut add = vec!["add", "--"];
        add.extend(paths);
        self.output(&add)?;
        let mut unchanged = vec!["diff", "--cached", "--quiet", "--"];
        unchanged.extend(paths);
        if self.output(&unchanged).is_ok() {
            return Ok(None);
        }
        let mut commit = vec!["commit", "-q", "--no-verify", "-m", message, "--"];
        commit.extend(paths);
        self.output(&commit)?;
        Ok(self.head())
    }

    pub fn branch_exists(&self, name: &str) -> bool {
        self.output(&["rev-parse", "--verify", "--quiet", &format!("refs/heads/{name}")])
            .is_ok()
    }

    /// The value of a trailer (`Key: value`) in the newest commit reachable from `rev` whose message contains
    /// `subject` (a run's own commits start with `passo(<id>):`, so another run's commits never answer).
    pub fn last_trailer(&self, key: &str, subject: &str, rev: &str) -> Option<String> {
        let grep = format!("--grep={subject}");
        let format = format!("--format=%(trailers:key={key},valueonly)");
        self.output(&["log", "-1", "--fixed-strings", &grep, &format, rev])
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    pub fn merge(&self, branch: &str, message: &str) -> Result<()> {
        if let Err(e) = self.output(&["merge", "--no-ff", "-q", "-m", message, branch]) {
            self.output(&["merge", "--abort"]).ok();
            return Err(e);
        }
        Ok(())
    }
}

/// Folders no project listing looks into: tooling, dependencies and build output.
pub const IGNORED_DIRS: [&str; 6] = [".git", ".passo", "target", "node_modules", "dist", "build"];

/// The project's files, relative to `root`: what git tracks, or a walk of the folders when there is no git.
/// Whether a project-relative path sits in a folder the project never counts.
pub fn ignored_path(path: &str) -> bool {
    path.trim_start_matches("./")
        .split('/')
        .any(|part| IGNORED_DIRS.contains(&part))
}

pub fn project_files(root: &Path, git: Option<&Git>) -> Vec<String> {
    let ignored = ignored_path;
    if let Some(files) = git.and_then(|g| g.tracked_files().ok()) {
        return files.into_iter().filter(|f| !ignored(f)).collect();
    }
    let mut out = vec![];
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let Ok(rel) = path.strip_prefix(root) else { continue };
            let rel = rel.to_string_lossy().replace('\\', "/");
            if ignored(&rel) {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else {
                out.push(rel);
            }
        }
    }
    out.sort();
    out
}
