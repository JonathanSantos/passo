//! Where things live. Everything Passo keeps for a project sits in `.passo/`.
//!
//! ```text
//! .passo/
//!   passo.yaml          config (extends, checks, sizes, models)
//!   context.md          project conventions every agent reads (optional)
//!   agents/<nome>.md    agent overrides and new agents
//!   stages/<etapa>.md   how a stage is done in this project
//!   checklists/*.md     domain checklists for planning and review
//!   current             id of the active run
//!   runs/<id>/          run.json, events.jsonl, plan-v<N>.json, map.md, sessions.json, report.html (not in git)
//!   audit/<id>/         the log and the plans as they were when the run ended, committed on the run's branch
//! ```

use std::path::{Path, PathBuf};

pub const DIR: &str = ".passo";

#[derive(Debug, Clone)]
pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Layout { root: root.into() }
    }

    /// The nearest ancestor of `start` with a `.passo/` folder. `~/.passo` is the global home, not a project.
    pub fn find(start: &Path) -> Option<Layout> {
        let home = home_dir();
        let mut dir = Some(start);
        while let Some(current) = dir {
            if current.join(DIR).is_dir() && Some(current.to_path_buf()) != home {
                return Some(Layout::new(current));
            }
            dir = current.parent();
        }
        None
    }

    pub fn passo(&self) -> PathBuf {
        self.root.join(DIR)
    }
    pub fn config(&self) -> PathBuf {
        self.passo().join(crate::config::FILE)
    }
    pub fn context(&self) -> PathBuf {
        self.passo().join("context.md")
    }
    pub fn agents(&self) -> PathBuf {
        self.passo().join("agents")
    }
    pub fn stages(&self) -> PathBuf {
        self.passo().join("stages")
    }
    pub fn checklists(&self) -> PathBuf {
        self.passo().join("checklists")
    }
    pub fn current(&self) -> PathBuf {
        self.passo().join("current")
    }
    pub fn runs(&self) -> PathBuf {
        self.passo().join("runs")
    }
    pub fn run_dir(&self, id: &str) -> PathBuf {
        self.runs().join(id)
    }
    pub fn run_file(&self, id: &str) -> PathBuf {
        self.run_dir(id).join("run.json")
    }
    pub fn events(&self, id: &str) -> PathBuf {
        self.run_dir(id).join("events.jsonl")
    }
    pub fn sessions(&self, id: &str) -> PathBuf {
        self.run_dir(id).join("sessions.json")
    }
    pub fn stops(&self, id: &str) -> PathBuf {
        self.run_dir(id).join("stops.json")
    }
    /// The repository map shown while refining and planning (a fact of the world, regenerated on a change).
    pub fn map(&self, id: &str) -> PathBuf {
        self.run_dir(id).join("map.md")
    }
    pub fn report(&self, id: &str) -> PathBuf {
        self.run_dir(id).join("report.html")
    }
    /// Where the run's log is kept for git: outside `runs/`, which stays ignored.
    pub fn audit_dir(&self, id: &str) -> PathBuf {
        self.passo().join("audit").join(id)
    }
    pub fn plan_version(&self, id: &str, version: u32) -> PathBuf {
        self.run_dir(id).join(format!("plan-v{version}.json"))
    }
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// `~/.passo`, or `$PASSO_HOME`.
pub fn global_home() -> Option<PathBuf> {
    std::env::var_os("PASSO_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(DIR)))
}

/// Writes through a temp file and a rename, so a reader never sees half a file.
pub fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(tmp, path)
}
