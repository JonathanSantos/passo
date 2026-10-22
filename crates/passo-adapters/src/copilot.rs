//! GitHub Copilot in VS Code (agent mode): `.github/skills`, `.github/agents/*.agent.md`, `.github/hooks/*.json`
//! and a managed block in `.github/copilot-instructions.md`.
//!
//! Copilot's hook format has moved more than once. Replies carry both the top-level fields and
//! `hookSpecificOutput`, and payloads are read under both the snake_case and camelCase keys.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::files;
use crate::payload;
use crate::{HookEvent, HookInput, HookReply, InstallContext, Platform, Usage};

pub struct Copilot;

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
        match tool.as_str() {
            "ler" => out.extend(["read", "search"].map(String::from)),
            "editar" => out.push("edit".into()),
            "terminal" => out.push("execute".into()),
            other => out.push(other.to_string()),
        }
    }
    out
}

const INSTRUCTIONS: &str = "## Passo\n\nEste projeto usa o **Passo** para conduzir mudanças de código: refinar, planejar, construir, verificar e entregar, com o dev aprovando. Para qualquer pedido de código, siga a skill `passo` (`.github/skills/passo/SKILL.md`): comece com `passo start` e siga a saída do `passo next`.";

impl Platform for Copilot {
    fn id(&self) -> &'static str {
        "copilot"
    }

    fn label(&self) -> &'static str {
        "GitHub Copilot (VS Code)"
    }

    /// No reliable signal yet: VS Code sets `VSCODE_PID` and `TERM_PROGRAM=vscode` in every integrated terminal,
    /// the dev's own included, so they cannot tell the agent apart.
    fn agent_env(&self, _env: &dyn Fn(&str) -> Option<String>) -> bool {
        false
    }

    fn hook_command(&self, root: &Path, event: HookEvent) -> Option<String> {
        let file = files::read_json(&root.join(".github/hooks/passo.json"));
        let entries = file.get("hooks")?.get(event_name(event))?.as_array()?;
        let marker = format!(" hook {} --platform copilot", event.id());
        entries
            .iter()
            .filter_map(|e| e.get("command").or_else(|| e.get("bash"))?.as_str())
            .find(|c| c.contains(&marker))
            .map(str::to_string)
    }

    fn installed(&self, root: &Path) -> bool {
        root.join(".github/skills/passo").exists()
    }

    fn default_model(&self, tier: &str) -> Option<&'static str> {
        match tier {
            "leve" => Some("Claude Haiku 5.5"),
            "forte" => Some("Claude Sonnet 5.5"),
            _ => None,
        }
    }

    fn install(&self, ctx: &InstallContext) -> Result<Vec<PathBuf>> {
        let mut written = vec![files::write(ctx.root, ".github/skills/passo/SKILL.md", ctx.skill)?];
        for agent in ctx.agents {
            let mut head = vec![
                format!("name: passo-{}", agent.name),
                format!("description: {}", files::yaml_text(&agent.description)),
            ];
            let tools = tools(&agent.tools);
            if !tools.is_empty() {
                head.push(format!("tools: {}", files::yaml_list(&tools)));
            }
            if let Some(model) = &agent.model {
                head.push(format!("model: {}", files::yaml_text(model)));
            }
            let body = format!(
                "---\n{}\n---\n\n{}\n\nA tarefa chega pela saída do `passo next`: siga-a à risca e entregue como ela diz.\n",
                head.join("\n"),
                agent.body.trim()
            );
            written.push(files::write(
                ctx.root,
                &format!(".github/agents/passo-{}.agent.md", agent.name),
                &body,
            )?);
        }

        let mut hooks = Map::new();
        for event in HookEvent::ALL {
            let command = format!("{} hook {} --platform copilot", ctx.bin, event.id());
            files::merge_hooks(
                &mut hooks,
                event_name(event),
                json!({ "type": "command", "command": command, "bash": command, "timeout": 15, "timeoutSec": 15 }),
            );
        }
        written.push(files::write(
            ctx.root,
            ".github/hooks/passo.json",
            &(serde_json::to_string_pretty(&json!({ "version": 1, "hooks": hooks }))? + "\n"),
        )?);
        written.push(files::managed_block(
            ctx.root,
            ".github/copilot-instructions.md",
            INSTRUCTIONS,
        )?);
        Ok(written)
    }

    fn parse_hook(&self, _event: HookEvent, raw: &Value) -> HookInput {
        let cwd = payload::text(raw, &["cwd", "workspaceFolder"]).map(PathBuf::from);
        HookInput {
            tool: payload::tool(
                raw,
                cwd.as_deref(),
                &["tool_name", "toolName"],
                &["tool_input", "toolArgs", "toolInput"],
            ),
            prompt: payload::text(raw, &["prompt", "userPrompt", "user_prompt"]),
            transcript: payload::text(raw, &["transcript_path", "transcriptPath"]).map(PathBuf::from),
            agent_transcript: payload::text(raw, &["agent_transcript_path", "agentTranscriptPath"]).map(PathBuf::from),
            project_dir: None,
            cwd,
        }
    }

    fn render(&self, event: HookEvent, reply: &HookReply) -> Option<Value> {
        let name = event_name(event);
        match reply {
            HookReply::Allow => None,
            HookReply::Deny(reason) => {
                let reason = format!("Passo: {reason}");
                Some(json!({
                    "permissionDecision": "deny",
                    "permissionDecisionReason": reason,
                    "hookSpecificOutput": {
                        "hookEventName": name, "permissionDecision": "deny", "permissionDecisionReason": reason,
                    }
                }))
            }
            HookReply::Block(reason) => Some(json!({ "decision": "block", "reason": reason })),
            HookReply::Context(text) => Some(json!({
                "additionalContext": text,
                "hookSpecificOutput": { "hookEventName": name, "additionalContext": text }
            })),
        }
    }

    fn usage_in(&self, _file: &Path) -> Vec<Usage> {
        // VS Code does not expose Copilot's token counts to hooks yet.
        vec![]
    }

    fn delegation(&self, agent: &str) -> String {
        format!(
            "Se puder rodar subagentes, use o agente `passo-{agent}` e passe a ele esta saída inteira; senão, faça você mesmo, seguindo o papel abaixo."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentSpec;

    #[test]
    fn installs_agents_hooks_and_keeps_instructions() {
        let root = std::env::temp_dir().join(format!("passo-copilot-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join(".github")).unwrap();
        std::fs::write(root.join(".github/copilot-instructions.md"), "# Time\nUse tabs.\n").unwrap();
        let agents = vec![AgentSpec {
            name: "revisor".into(),
            description: "Confere: critérios".into(),
            model: Some("Claude Sonnet 5.5".into()),
            tools: vec!["ler".into()],
            body: "corpo".into(),
        }];
        let ctx = InstallContext {
            root: &root,
            bin: "passo",
            skill: "skill",
            agents: &agents,
        };
        Copilot.install(&ctx).unwrap();
        Copilot.install(&ctx).unwrap();
        let instructions = std::fs::read_to_string(root.join(".github/copilot-instructions.md")).unwrap();
        assert!(instructions.starts_with("# Time\nUse tabs."));
        assert_eq!(instructions.matches("passo:start").count(), 1);
        let agent = std::fs::read_to_string(root.join(".github/agents/passo-revisor.agent.md")).unwrap();
        assert!(agent.contains("tools: ['read', 'search']"));
        let hooks: Value =
            serde_json::from_str(&std::fs::read_to_string(root.join(".github/hooks/passo.json")).unwrap()).unwrap();
        assert_eq!(hooks["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn reads_camel_case_payloads() {
        let raw = json!({"cwd": "/p", "toolName": "editFiles", "toolArgs": {"filePath": "/p/a.ts"}});
        let input = Copilot.parse_hook(HookEvent::PreToolUse, &raw);
        let tool = input.tool.unwrap();
        assert_eq!(tool.kind, passo_core::lock::ToolKind::Write);
    }
}
