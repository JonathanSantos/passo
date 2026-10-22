//! Hook handling, platform-free: the adapter reads the payload and writes the reply; this decides.
//!
//! A hook that fails would stall the agent, so every error becomes "say nothing" (`PASSO_DEBUG=1` prints it).

use std::path::Path;
use std::sync::Arc;

use passo_adapters::{HookEvent, HookReply, Platform};
use passo_core::answers::{ChatDecision, parse_chat};
use passo_core::lock::{self, ToolUse, Verdict};
use passo_core::{Actor, EventDraft, Run, Stage, Turn};
use passo_store::{Layout, Transcript};
use serde_json::{Value, json};

use crate::service::{App, DevAction, Reporter, Source};

/// After this many blocks at the same spot the agent may stop: a hook must not trap it in a loop.
const MAX_STOP_BLOCKS: u32 = 3;

/// `reporter` writes the delivery report when a decision taken here (an approval typed in the chat) ends the run.
pub fn handle(
    platform: &str,
    event: HookEvent,
    stdin: &str,
    cwd: &Path,
    reporter: Option<Arc<dyn Reporter>>,
) -> Option<String> {
    let platform = passo_adapters::get(platform)?;
    let raw: Value = serde_json::from_str(stdin).unwrap_or(Value::Null);
    match decide(platform.as_ref(), event, &raw, cwd, reporter) {
        Ok(reply) => platform.render(event, &reply).map(|v| v.to_string()),
        Err(e) => {
            if std::env::var_os("PASSO_DEBUG").is_some() {
                eprintln!("passo hook: {e:#}");
            }
            // The hook still lets the call through (a broken hook must not trap the agent), but never silently.
            let input = platform.parse_hook(event, &raw);
            let candidates = [input.project_dir, input.cwd, Some(cwd.into())];
            if let Some(layout) = candidates.into_iter().flatten().find_map(|dir| Layout::find(&dir)) {
                record_failure(&layout, platform.id(), event, &format!("{e:#}"));
            }
            None
        }
    }
}

/// Lines kept in `hook-errors.log`.
const MAX_ERROR_LINES: usize = 200;

/// Writes the failure where `passo next`, `status`, the dashboard and `doctor` see it. Uses only the layout: the
/// app is often what failed to open (an invalid `passo.yaml`).
fn record_failure(layout: &Layout, platform: &str, event: HookEvent, error: &str) {
    let error = passo_core::redact::redact(error.lines().next().unwrap_or(error));
    let path = layout.hook_errors();
    let Ok(lock) = std::fs::File::create(path.with_extension("lock")) else {
        return;
    };
    if lock.lock().is_err() {
        return;
    }
    let line =
        json!({ "t": chrono::Utc::now(), "event": event.id(), "platform": platform, "error": error }).to_string();
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    lines.push(line);
    let skip = lines.len().saturating_sub(MAX_ERROR_LINES);
    std::fs::write(&path, lines[skip..].join("\n") + "\n").ok();
    drop(lock);

    // Once per distinct reason, the run's own log says so too.
    let store = passo_store::Store::new(layout.clone());
    let Some(run) = store
        .current_id()
        .and_then(|id| store.load(&id).ok())
        .filter(|r| r.active())
    else {
        return;
    };
    let log = store.log(&run.id);
    let text = format!("Hook {} falhou: {error}", event.id());
    if log
        .read()
        .map(|events| events.iter().any(|e| e.kind == "hook.failed" && e.text == text))
        .unwrap_or(true)
    {
        return;
    }
    let draft = EventDraft {
        kind: "hook.failed",
        actor: Actor::Passo,
        stage: run.stage,
        text,
        data: json!({ "platform": platform }),
    };
    log.append(&run.id, &[draft], chrono::Utc::now()).ok();
}

fn decide(
    platform: &dyn Platform,
    event: HookEvent,
    raw: &Value,
    cwd: &Path,
    reporter: Option<Arc<dyn Reporter>>,
) -> anyhow::Result<HookReply> {
    let input = platform.parse_hook(event, raw);
    let candidates = [input.project_dir.clone(), input.cwd.clone(), Some(cwd.into())];
    let Some(layout) = candidates.into_iter().flatten().find_map(|dir| Layout::find(&dir)) else {
        return Ok(HookReply::Allow);
    };
    let mut app = App::at(layout)?;
    if let Some(reporter) = reporter {
        app = app.with_reporter(reporter);
    }
    let run = app.store.active();
    let mapping = run.is_none() && app.mapping();
    let activity = lock::Activity::of(run.as_ref(), mapping);
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
            None if mapping => HookReply::Context(
                "Passo: há um mapeamento do projeto em andamento. Rode `passo next` para ver a tarefa.".into(),
            ),
            None => HookReply::Allow,
        }),
        HookEvent::UserPrompt => {
            let (Some(run), Some(prompt)) = (&run, &input.prompt) else {
                return Ok(HookReply::Allow);
            };
            let Some(gate) = run.gate.as_ref().filter(|_| run.turn() == Turn::Dev) else {
                return Ok(HookReply::Allow);
            };
            let Some(decision) = parse_chat(prompt, gate.kind) else {
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
                ChatDecision::Hint(hint) => {
                    return Ok(HookReply::Context(format!("Passo: {hint} Repasse isso ao dev.")));
                }
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
            let attached: Vec<std::path::PathBuf> = crate::service::attached_roots(&app.layout.root, &app.config)
                .into_iter()
                .map(|(_, r)| r)
                .collect();
            let verdict = lock::judge_with(&tool, &root, &attached, activity);
            let mut warning = None;
            if mapping {
                let (kind, text) = match &verdict {
                    Verdict::Allow => ("tool", app.redact(&describe(&tool, &root))),
                    Verdict::Deny(reason) => ("lock.denied", reason.clone()),
                };
                app.map_log_event(
                    kind,
                    text,
                    json!({ "tool": tool.name, "call": app.redact(&describe(&tool, &root)) }),
                );
            }
            if let Some(run) = run.as_ref().filter(|r| r.active()) {
                let step = run.current_step().filter(|_| run.stage == Stage::Construir);
                let (kind, text) = match &verdict {
                    Verdict::Allow => ("tool", app.redact(&describe(&tool, &root))),
                    Verdict::Deny(reason) => ("lock.denied", reason.clone()),
                };
                let mut drafts = vec![EventDraft {
                    kind,
                    actor: Actor::Agent,
                    stage: run.stage,
                    text,
                    data: json!({
                        "tool": tool.name,
                        "call": app.redact(&describe(&tool, &root)),
                        "step": step.map(|s| s.id.clone()),
                        "attempt": step.map(|s| s.attempts),
                    }),
                }];
                // Only calls the lock let through count against the budget.
                if run.turn() == Turn::Agent && matches!(verdict, Verdict::Allow) {
                    warning = budget(&app, run, &mut drafts)?;
                }
                app.store.log(&run.id).append(&run.id, &drafts, chrono::Utc::now())?;
            }
            Ok(match (verdict, warning) {
                (Verdict::Deny(reason), _) => HookReply::Deny(reason),
                (Verdict::Allow, Some(warning)) => HookReply::Context(warning),
                (Verdict::Allow, None) => HookReply::Allow,
            })
        }
        HookEvent::Stop => {
            if mapping {
                let Some(reason) = lock::judge_stop(activity) else {
                    return Ok(HookReply::Allow);
                };
                return Ok(if app.map_count_stop() <= MAX_STOP_BLOCKS {
                    HookReply::Block(reason)
                } else {
                    HookReply::Allow
                });
            }
            let (Some(run), Some(reason)) = (&run, lock::judge_stop(activity)) else {
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

/// Counts the call against the step's action budget. Past the limit the agent is told, never stopped: once at the
/// limit, then every 20 calls. The first time also leaves a `step.budget` event.
fn budget(app: &App, run: &Run, drafts: &mut Vec<EventDraft>) -> anyhow::Result<Option<String>> {
    const REPEAT: u32 = 20;
    let limit = app.config.profile(run.size).max_tool_calls;
    if limit == 0 {
        return Ok(None);
    }
    let (key, place) = match run.current_step().filter(|_| run.stage == Stage::Construir) {
        Some(step) => (
            format!("{}:{}:{}", run.stage.id(), step.id, step.attempts),
            "neste passo",
        ),
        None => (run.stage.id().to_string(), "nesta etapa"),
    };
    let count = app.store.count_tool(&run.id, &key)?;
    if count <= limit || !(count - limit - 1).is_multiple_of(REPEAT) {
        return Ok(None);
    }
    if count == limit + 1 {
        drafts.push(EventDraft {
            kind: "step.budget",
            actor: Actor::Passo,
            stage: run.stage,
            text: format!("Passou de {limit} ações {place}"),
            data: json!({ "key": key }),
        });
    }
    let done = if run.stage == Stage::Construir {
        "passo done"
    } else {
        "passo done <arquivo>"
    };
    Ok(Some(format!(
        "Passo: já foram {count} ações {place} (limite {limit}). Entregue com {done}, ou pergunte com passo ask se travou."
    )))
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
