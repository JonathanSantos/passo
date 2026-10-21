//! Passo app: the use cases that tie the core, the store and the adapters together.
//!
//! - [`service`]: start, next, done, ask and the dev's decisions; effects (checks, commits, delivery).
//! - [`render`]: what the agent and the dev read.
//! - [`hooks`]: the lock and chat approvals, for any platform.
//! - [`metrics`]: time, dev turns, tokens per stage and model.
//! - [`setup`]: `passo init` and `passo sync`.
//! - [`repomap`]: the repository map refining and planning read.
//! - [`infra`]: clock, shell checks and git, behind traits.

pub mod hooks;
pub mod infra;
pub mod metrics;
pub mod render;
pub mod repomap;
pub mod service;
pub mod setup;
pub mod snapshot;

pub use service::{App, DevAction, DoneInput, Rejected, Reporter, Source};
pub use snapshot::Snapshot;
