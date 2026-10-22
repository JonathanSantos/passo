//! Passo store: everything on disk.
//!
//! - [`layout`]: the `.passo/` folder.
//! - [`runs`]: runs, the active run, plan versions, transcripts.
//! - [`audit`]: the hash-chained event log.
//! - [`config`]: `passo.yaml` in layers, with `extends`.
//! - [`map`]: the project map an exploring agent writes, in `.passo/map/`.
//! - [`learn`]: `passo learn` in progress and the lessons already decided on, in `.passo/learn/`.
//! - [`catalog`]: agents, stage instructions and checklists, built in and overridden by the project.

pub mod assets;
pub mod audit;
pub mod catalog;
pub mod config;
pub mod frontmatter;
pub mod layout;
pub mod learn;
pub mod map;
pub mod runs;

pub use audit::{Event, EventLog};
pub use catalog::{AgentDef, Catalog, Checklist, StageDef};
pub use layout::Layout;
pub use runs::{Store, Transcript};
