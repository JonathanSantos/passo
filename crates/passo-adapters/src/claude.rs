//! Claude Code: `.claude/skills`, `.claude/agents` and hooks in `.claude/settings.json`.

use std::collections::BTreeSet;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::files;
use crate::payload;
use crate::{HookEvent, HookInput, HookReply, InstallContext, Platform, Usage};

pub struct Claude;

const GUARDED_TOOLS: &str = "Read|Write|Edit|MultiEdit|NotebookEdit|Glob|Grep|Bash";

fn event_name(event: HookEvent) -> &'static str {
    match event {
        HookEvent::SessionStart => "SessionStart",
        HookEvent::UserPrompt => "UserPromptSubmit",
        HookEvent::PreToolUse => "PreToolUse",
        HookEvent::Stop => "Stop",
        HookEvent::SubagentStop => "SubagentStop",
    }
}

fn tools(abstract_tools: &[String]) -> Vec<String> {
    let mut out = vec![];
    for tool in abstract_tools {
        let names: &[&str] = match tool.as_str() {
            "ler" => &["Read", "Grep", "Glob"],
            "editar" => &["Edit", "Write", "MultiEdit"],
            "terminal" => &["Bash"],
            other => {
                out.push(other.to_string());
                continue;
            }
        };
        out.extend(names.iter().map(|n| n.to_string()));
    }
    out
}

impl Platform for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn label(&self) -> &'static str {
        "Claude Code"
    }

    /// Claude Code sets `CLAUDECODE` for the tools it runs, and subagents inherit it.
    fn agent_env(&self, env: &dyn Fn(&str) -> Option<String>) -> bool {
        env("CLAUDECODE").is_some_and(|v| !v.is_empty())
    }

    fn hook_command(&self, root: &Path, event: HookEvent) -> Option<String> {
        let settings = files::read_json(&root.join(".claude/settings.json"));
        let entries = settings.get("hooks")?.get(event_name(event))?.as_array()?;
        let marker = format!(" hook {} --platform claude", event.id());
        let entry = entries.iter().find(|e| e.to_string().contains(&marker))?;
        let expected = match event {
            HookEvent::PreToolUse => Some(GUARDED_TOOLS),
            HookEvent::SessionStart => Some("startup|resume|clear|compact"),
            _ => None,
        };
        if expected.is_some() && entry.get("matcher").and_then(Value::as_str) != expected {
            return None;
        }
        entry
            .get("hooks")?
            .as_array()?
            .iter()
            .find_map(|h| h.get("command")?.as_str().map(str::to_string))
    }

    fn installed(&self, root: &Path) -> bool {
        root.join(".claude/skills/passo").exists()
    }

    fn probe(&self, root: &Path, event: HookEvent) -> Value {
        match event {
            HookEvent::UserPrompt => json!({ "cwd": root, "hook_event_name": "UserPromptSubmit", "prompt": "status?" }),
            _ => json!({
                "cwd": root, "hook_event_name": "PreToolUse", "tool_name": "Bash",
                "tool_input": { "command": "passo ok" },
            }),
        }
    }

    fn is_deny(&self, reply: &Value) -> bool {
        reply["hookSpecificOutput"]["permissionDecision"] == "deny"
    }

    fn hook_matcher(&self, root: &Path, event: HookEvent) -> Option<(String, String)> {
        let settings = files::read_json(&root.join(".claude/settings.json"));
        let entries = settings.get("hooks")?.get(event_name(event))?.as_array()?;
        let marker = format!(" hook {} --platform claude", event.id());
        let entry = entries.iter().find(|e| e.to_string().contains(&marker))?;
        let expected = match event {
            HookEvent::PreToolUse => GUARDED_TOOLS,
            HookEvent::SessionStart => "startup|resume|clear|compact",
            _ => return None,
        };
        let found = entry.get("matcher").and_then(Value::as_str).unwrap_or("");
        (found != expected).then(|| (found.to_string(), expected.to_string()))
    }

    fn default_model(&self, tier: &str) -> Option<&'static str> {
        match tier {
            "leve" => Some("haiku"),
            "forte" => Some("sonnet"),
            _ => None,
        }
    }

    fn install(&self, ctx: &InstallContext) -> Result<Vec<PathBuf>> {
        let mut written = vec![files::write(ctx.root, ".claude/skills/passo/SKILL.md", ctx.skill)?];
        for agent in ctx.agents {
            let mut head = vec![
                format!("name: passo-{}", agent.name),
                format!("description: {}", files::yaml_text(&agent.description)),
            ];
            let tools = tools(&agent.tools);
            if !tools.is_empty() {
                head.push(format!("tools: {}", tools.join(", ")));
            }
            if let Some(model) = &agent.model {
                head.push(format!("model: {model}"));
            }
            let body = format!(
                "---\n{}\n---\n\n{}\n\nA tarefa chega pela saída do `passo next`: siga-a à risca e entregue como ela diz.\n",
                head.join("\n"),
                agent.body.trim()
            );
            written.push(files::write(
                ctx.root,
                &format!(".claude/agents/passo-{}.md", agent.name),
                &body,
            )?);
        }

        let settings_path = ctx.root.join(".claude/settings.json");
        let mut settings = files::read_json(&settings_path);
        let mut hooks = settings
            .get("hooks")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for event in HookEvent::ALL {
            let command = format!("{} hook {} --platform claude", ctx.bin, event.id());
            let mut entry = json!({ "hooks": [{ "type": "command", "command": command, "timeout": 15 }] });
            match event {
                HookEvent::PreToolUse => entry["matcher"] = json!(GUARDED_TOOLS),
                HookEvent::SessionStart => entry["matcher"] = json!("startup|resume|clear|compact"),
                _ => {}
            }
            files::merge_hooks(&mut hooks, event_name(event), entry);
        }
        settings.insert("hooks".into(), Value::Object(hooks));
        written.push(files::write(
            ctx.root,
            ".claude/settings.json",
            &(serde_json::to_string_pretty(&Value::Object(settings))? + "\n"),
        )?);
        Ok(written)
    }

    fn parse_hook(&self, _event: HookEvent, raw: &Value) -> HookInput {
        let cwd = payload::text(raw, &["cwd"]).map(PathBuf::from);
        HookInput {
            tool: payload::tool(raw, cwd.as_deref(), &["tool_name"], &["tool_input"]),
            prompt: payload::text(raw, &["prompt"]),
            transcript: payload::text(raw, &["transcript_path"]).map(PathBuf::from),
            agent_transcript: payload::text(raw, &["agent_transcript_path"]).map(PathBuf::from),
            // Claude Code tells hooks where the project is, even when the tool call runs elsewhere.
            project_dir: std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from),
            cwd,
        }
    }

    fn render(&self, event: HookEvent, reply: &HookReply) -> Option<Value> {
        let name = event_name(event);
        match reply {
            HookReply::Allow => None,
            HookReply::Deny(reason) => Some(json!({
                "hookSpecificOutput": {
                    "hookEventName": name,
                    "permissionDecision": "deny",
                    "permissionDecisionReason": format!("Passo: {reason}"),
                }
            })),
            HookReply::Block(reason) => Some(json!({ "decision": "block", "reason": reason })),
            // Stop has no context to add: the agent is done; the dev reads the message.
            HookReply::Context(text) if event == HookEvent::Stop => Some(json!({ "systemMessage": text })),
            HookReply::Context(text) => {
                Some(json!({ "hookSpecificOutput": { "hookEventName": name, "additionalContext": text } }))
            }
        }
    }

    fn usage_files(&self, transcripts: &[PathBuf]) -> Vec<PathBuf> {
        let mut files: BTreeSet<PathBuf> = transcripts.iter().cloned().collect();
        // Subagents keep their own transcripts next to the session's: <session>/subagents/*.jsonl.
        for transcript in transcripts {
            let dir = transcript.with_extension("").join("subagents");
            if let Ok(entries) = std::fs::read_dir(dir) {
                files.extend(
                    entries
                        .filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .filter(|p| p.extension().is_some_and(|x| x == "jsonl")),
                );
            }
        }
        files.into_iter().collect()
    }

    fn usage_in(&self, file: &Path) -> Vec<Usage> {
        let mut messages = std::collections::BTreeMap::new();
        read_usage(file, &mut messages);
        messages.into_values().collect()
    }

    fn delegation(&self, agent: &str) -> String {
        format!("Delegue ao subagente `passo-{agent}` (Task) e passe a ele esta saída inteira.")
    }
}

fn read_usage(path: &Path, messages: &mut std::collections::BTreeMap<String, Usage>) {
    let Ok(file) = std::fs::File::open(path) else { return };
    for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if entry.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let message = &entry["message"];
        let usage = &message["usage"];
        // A message streams into several lines with the same id; the counts grow, so keep the largest.
        let id = message
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| line.clone());
        let Some(t) = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.with_timezone(&Utc))
        else {
            continue;
        };
        let n = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
        let usage = Usage {
            t,
            model: message.get("model").and_then(Value::as_str).unwrap_or("?").to_string(),
            input: n("input_tokens"),
            output: n("output_tokens"),
            cache_read: n("cache_read_input_tokens"),
            cache_write: n("cache_creation_input_tokens"),
        };
        messages
            .entry(id)
            .and_modify(|seen: &mut Usage| {
                seen.input = seen.input.max(usage.input);
                seen.output = seen.output.max(usage.output);
                seen.cache_read = seen.cache_read.max(usage.cache_read);
                seen.cache_write = seen.cache_write.max(usage.cache_write);
            })
            .or_insert(usage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentSpec;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("passo-claude-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn install_keeps_team_hooks_and_is_idempotent() {
        let root = temp("install");
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(
            root.join(".claude/settings.json"),
            r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"say done"}]}]}}"#,
        )
        .unwrap();
        let agents = vec![AgentSpec {
            name: "construtor".into(),
            description: "Faz: um passo".into(),
            model: Some("haiku".into()),
            tools: vec!["ler".into(), "editar".into()],
            body: "corpo".into(),
        }];
        let ctx = InstallContext {
            root: &root,
            bin: "/bin/passo",
            skill: "skill",
            agents: &agents,
        };
        Claude.install(&ctx).unwrap();
        Claude.install(&ctx).unwrap();
        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(root.join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(settings["model"], "opus");
        assert_eq!(
            settings["hooks"]["Stop"].as_array().unwrap().len(),
            2,
            "team hook + one passo hook"
        );
        let agent = std::fs::read_to_string(root.join(".claude/agents/passo-construtor.md")).unwrap();
        assert!(agent.contains("model: haiku") && agent.contains("tools: Read, Grep, Glob, Edit, Write, MultiEdit"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn usage_counts_each_message_once_and_reads_subagents() {
        let root = temp("usage");
        let line = |id: &str, out: u64| {
            format!(
                r#"{{"type":"assistant","timestamp":"2026-10-20T12:00:00Z","message":{{"id":"{id}","model":"claude-haiku-5-5","usage":{{"input_tokens":10,"output_tokens":{out},"cache_read_input_tokens":100}}}}}}"#
            )
        };
        std::fs::write(root.join("s1.jsonl"), format!("{}\n{}\n", line("m1", 1), line("m1", 5))).unwrap();
        std::fs::create_dir_all(root.join("s1/subagents")).unwrap();
        std::fs::write(root.join("s1/subagents/agent-a.jsonl"), line("m2", 7)).unwrap();
        let usage = Claude.usage(&[root.join("s1.jsonl")]);
        assert_eq!(usage.len(), 2);
        assert_eq!(usage.iter().map(|u| u.output).sum::<u64>(), 12);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn agent_environment_is_read_from_claudecode() {
        assert!(Claude.agent_env(&|k| (k == "CLAUDECODE").then(|| "1".to_string())));
        assert!(!Claude.agent_env(&|_| None));
        assert!(!Claude.agent_env(&|k| (k == "CLAUDECODE").then(String::new)));
    }

    #[test]
    fn deny_dialect() {
        let out = Claude
            .render(HookEvent::PreToolUse, &HookReply::Deny("não".into()))
            .unwrap();
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny");
    }
}
