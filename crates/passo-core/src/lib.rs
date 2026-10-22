//! Passo core: the vocabulary, the flow and the lock. Pure logic, no IO.
//!
//! - [`model`]: stages, sizes, the run and what it records.
//! - [`config`]: configuration as data, with the size profiles (how each size runs each stage).
//! - [`spec`]: what agents hand in (brief, plan, review) and how it is validated.
//! - [`flow`]: the deterministic state machine.
//! - [`answers`]: the dev's decisions, from the CLI or the chat.
//! - [`lock`]: what the agent may do right now.

pub mod answers;
pub mod config;
pub mod flow;
pub mod lock;
pub mod map;
pub mod model;
pub mod redact;
pub mod shell;
pub mod spec;

pub use config::{Config, Mode, Profile};
pub use flow::{Command, Effect, Engine, EventDraft, FlowError, Outcome, StepFacts};
pub use model::*;
