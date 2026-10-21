//! Passo app: the use cases that tie the core, the store and the adapters together.
//!
//! - [`service`]: start, next, done, ask and the dev's decisions; effects (checks, commits, delivery).
//! - [`render`]: what the agent and the dev read.
//! - [`mapping`]: `passo map`: an agent explores the project and Passo checks and writes the map.
//! - [`hooks`]: the lock and chat approvals, for any platform.
//! - [`metrics`]: time, dev turns, tokens per stage and model.
//! - [`setup`]: `passo init` and `passo sync`.
//! - [`tree`]: the repository tree refining and planning read.
//! - [`infra`]: clock, shell checks and git, behind traits.

pub mod hooks;
pub mod infra;
pub mod mapping;
pub mod metrics;
pub mod render;
pub mod service;
pub mod setup;
pub mod snapshot;
pub mod tree;

pub use service::{App, DevAction, DoneInput, Rejected, Reporter, Source};
pub use snapshot::Snapshot;
