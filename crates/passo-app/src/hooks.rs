//! Hook handling, platform-free: the adapter reads the payload and writes the reply; this decides.
//!
//! A hook that fails would stall the agent, so every error becomes "say nothing" (`PASSO_DEBUG=1` prints it).

use std::path::{Path, PathBuf};

use passo_adapters::{HookEvent, HookReply, Platform};
use passo_core::answers::{ChatDecision, parse_chat};
use passo_core::lock::{self, ToolUse, Verdict};
use passo_core::{Actor, EventDraft, Turn};
use passo_store::{Layout, Transcript};
use serde_json::{Value, json};

use crate::service::{App, DevAction, Source};

/// After this many blocks at the same spot the agent may stop: a hook must not trap it in a loop.
const MAX_STOP_BLOCKS: u32 = 3;

pub fn handle(platform: &str, event: HookEvent, stdin: &str, cwd: &Path) -> Option<String> {
    let platform = passo_adapters::get(platform)?;
    let raw: Value = serde_json::from_str(stdin).unwrap_or(Value::Null);
    match decide(platform.as_ref(), event, &raw, cwd) {
        Ok(reply) => platform.render(event, &reply).map(|v| v.to_string()),
        Err(e) => {
            if std::env::var_os("PASSO_DEBUG").is_some() {
                eprintln!("passo hook: {e:#}");
            }
            None
        }
    }
}

fn decide(platform: &dyn Platform, event: HookEvent, raw: &Value, cwd: &Path) -> anyhow::Result<HookReply> {
    let input = platform.parse_hook(event, raw);
    let candidates = [
        std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from),
        input.cwd.clone(),
        Some(cwd.into()),
    ];
    let Some(layout) = candidates.into_iter().flatten().find_map(|dir| Layout::find(&dir)) else {
        return Ok(HookReply::Allow);
    };
    let app = App::at(layout)?;
    let run = app.store.active();
    if let Some(run) = &run {
        for path in [&input.transcript, &input.agent_transcript].into_iter().flatten() {
            let transcript = Transcript {
                platform: platform.id().into(),
                path: path.display().to_string(),
            };
            app.store.add_transcript(&run.id, transcript)?;
        }
    }

    match event {
        HookEvent::SessionStart => Ok(match &run {
            Some(run) => HookReply::Context(format!(
                "Passo: há um run em andamento ({}). Rode `passo next` para ver o que ele precisa.",
                run.headline()
            )),
            None => HookReply::Allow,
        }),
        HookEvent::UserPrompt => {
            let (Some(run), Some(prompt)) = (&run, &input.prompt) else {
                return Ok(HookReply::Allow);
            };
            if run.turn() != Turn::Dev {
                return Ok(HookReply::Allow);
            }
            let Some(decision) = parse_chat(prompt) else {
                return Ok(HookReply::Allow);
            };
            let action = match decision {
                ChatDecision::Approve(d) => DevAction::Approve(
                    d.answers
                        .iter()
                        .map(|a| match a.index {
                            Some(i) => format!("{i}={}", a.value),
                            None => a.value.clone(),
                        })
                        .collect(),
                ),
                ChatDecision::Reject(reason) => DevAction::Reject(reason),
            };
            Ok(HookReply::Context(match app.decide(action, Source::Chat) {
                Ok(next) => format!(
                    "Passo: o dev respondeu pelo chat e o Passo registrou ({}). Rode `passo next` e siga a saída.",
                    next.headline()
                ),
                Err(e) => format!("Passo: o dev tentou responder pelo chat, mas: {e}. Mostre isso ao dev."),
            }))
        }
        HookEvent::PreToolUse => {
            let Some(tool) = input.tool else {
                return Ok(HookReply::Allow);
            };
            let root = std::fs::canonicalize(&app.layout.root).unwrap_or(app.layout.root.clone());
            let verdict = lock::judge(&tool, &root, run.as_ref());
            if let Some(run) = run.as_ref().filter(|r| r.active()) {
                let (kind, text) = match &verdict {
                    Verdict::Allow => ("tool", describe(&tool, &root)),
                    Verdict::Deny(reason) => ("lock.denied", reason.clone()),
                };
                let draft = EventDraft {
                    kind,
                    actor: Actor::Agent,
                    stage: run.stage,
                    text,
                    data: json!({ "tool": tool.name, "call": describe(&tool, &root) }),
                };
                app.store.log(&run.id).append(&run.id, &[draft], chrono::Utc::now())?;
            }
            Ok(match verdict {
                Verdict::Allow => HookReply::Allow,
                Verdict::Deny(reason) => HookReply::Deny(reason),
            })
        }
        HookEvent::Stop => {
            let (Some(run), Some(reason)) = (&run, lock::judge_stop(run.as_ref())) else {
                return Ok(HookReply::Allow);
            };
            let attempts = run.current_step().map(|s| s.attempts).unwrap_or(0);
            let key = format!(
                "{}:{}:{}:{}",
                run.stage.id(),
                run.current_step().map(|s| s.id.as_str()).unwrap_or(""),
                attempts,
                run.rounds
            );
            Ok(if app.store.count_stop(&run.id, &key)? <= MAX_STOP_BLOCKS {
                HookReply::Block(reason)
            } else {
                HookReply::Allow
            })
        }
        HookEvent::SubagentStop => Ok(HookReply::Allow),
    }
}

fn describe(tool: &ToolUse, root: &Path) -> String {
    if let Some(command) = &tool.command {
        let flat: String = command.split_whitespace().collect::<Vec<_>>().join(" ");
        return format!("{}: {}", tool.name, flat.chars().take(140).collect::<String>());
    }
    let files: Vec<String> = tool
        .paths
        .iter()
        .map(|p| {
            p.strip_prefix(root)
                .map(|r| r.display().to_string())
                .unwrap_or_else(|_| p.display().to_string())
        })
        .collect();
    if files.is_empty() {
        tool.name.clone()
    } else {
        format!("{} {}", tool.name, files.join(", "))
    }
}
