//! Passo adapters: the only code that knows Claude Code or GitHub Copilot exist.
//!
//! Agent platforms change often (hook names, payload fields, file locations, model names). Each one lives behind
//! [`Platform`], and nothing outside this crate names a platform. Adding one is a new module plus a line in
//! [`registry`].

use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use passo_core::lock::ToolUse;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod claude;
pub mod copilot;
mod files;
mod payload;

/// The hook moments Passo listens to, whatever each platform calls them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    SessionStart,
    UserPrompt,
    PreToolUse,
    Stop,
    SubagentStop,
}

impl HookEvent {
    pub const ALL: [HookEvent; 5] = [
        HookEvent::SessionStart,
        HookEvent::UserPrompt,
        HookEvent::PreToolUse,
        HookEvent::Stop,
        HookEvent::SubagentStop,
    ];

    /// The name used on Passo's own command line: `passo hook <id>`.
    pub fn id(self) -> &'static str {
        match self {
            HookEvent::SessionStart => "session-start",
            HookEvent::UserPrompt => "user-prompt",
            HookEvent::PreToolUse => "pre-tool-use",
            HookEvent::Stop => "stop",
            HookEvent::SubagentStop => "subagent-stop",
        }
    }

    pub fn parse(text: &str) -> Option<HookEvent> {
        HookEvent::ALL.into_iter().find(|e| e.id() == text)
    }
}

/// What a hook payload means, in Passo's terms.
#[derive(Debug, Clone, Default)]
pub struct HookInput {
    pub cwd: Option<PathBuf>,
    /// The project root the platform itself reports (an environment variable, say), when it does.
    pub project_dir: Option<PathBuf>,
    pub tool: Option<ToolUse>,
    /// What the dev typed (prompt hooks only).
    pub prompt: Option<String>,
    pub transcript: Option<PathBuf>,
    /// A subagent's own transcript (subagent-stop).
    pub agent_transcript: Option<PathBuf>,
}

/// What Passo wants to say back.
#[derive(Debug, Clone, PartialEq)]
pub enum HookReply {
    /// Say nothing: the platform carries on.
    Allow,
    /// Refuse a tool call, with the reason the agent will read.
    Deny(String),
    /// Refuse to stop: the agent keeps going and reads the reason.
    Block(String),
    /// Add context to the session (start, prompt).
    Context(String),
}

/// An agent as a platform installs it. Tools are Passo's abstract names: `ler`, `editar`, `terminal`.
#[derive(Debug, Clone)]
pub struct AgentSpec {
    pub name: String,
    pub description: String,
    /// The platform's model name, already resolved from the tier.
    pub model: Option<String>,
    pub tools: Vec<String>,
    pub body: String,
}

pub struct InstallContext<'a> {
    pub root: &'a Path,
    /// How hooks call Passo (an absolute path, so hooks work outside the PATH).
    pub bin: &'a str,
    /// The skill text, with `{{bin}}` already replaced.
    pub skill: &'a str,
    pub agents: &'a [AgentSpec],
}

/// One model call, read from a transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub t: DateTime<Utc>,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

pub trait Platform: Send + Sync {
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    /// Whether `passo init` already set this platform up in the project.
    fn installed(&self, root: &Path) -> bool;
    /// The platform's model for a tier (`leve`, `forte`) when the config does not say.
    fn default_model(&self, tier: &str) -> Option<&'static str>;
    /// Writes the skill, the agents and the hooks into the project. Returns the files written.
    fn install(&self, ctx: &InstallContext) -> Result<Vec<PathBuf>>;
    /// Reads a hook payload.
    fn parse_hook(&self, event: HookEvent, raw: &Value) -> HookInput;
    /// Writes a reply in the platform's dialect; `None` prints nothing.
    fn render(&self, event: HookEvent, reply: &HookReply) -> Option<Value>;
    /// Token usage from the transcripts its hooks reported. Platforms without transcripts return nothing.
    fn usage(&self, transcripts: &[PathBuf]) -> Vec<Usage>;
    /// How the main agent hands a stage to a subagent on this platform.
    fn delegation(&self, agent: &str) -> String;
}

pub fn registry() -> Vec<Box<dyn Platform>> {
    vec![Box::new(claude::Claude), Box::new(copilot::Copilot)]
}

pub fn get(id: &str) -> Option<Box<dyn Platform>> {
    registry().into_iter().find(|p| p.id() == id)
}

pub fn ids() -> Vec<&'static str> {
    registry().iter().map(|p| p.id()).collect()
}
