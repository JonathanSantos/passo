//! `passo doctor` for hooks: runs each installed platform's hook end to end and says whether the lock holds.
//! A hook that fails lets everything through, so this is the place that proves it does not.

use std::path::{Path, PathBuf};
use std::time::Duration;

use passo_adapters::{HookEvent, Platform};
use serde_json::json;

use crate::infra::run_with_input;
use crate::service::App;

const TIMEOUT: Duration = Duration::from_secs(5);

/// One line per installed platform: `Hook (Claude Code): ✓ responde em 14 ms · binário … · 5 eventos`, or what
/// is wrong.
pub fn check_hooks(app: &App) -> Vec<String> {
    app.platforms()
        .iter()
        .map(|p| format!("Hook ({}): {}", p.label(), check(app, p.as_ref())))
        .collect()
}

fn check(app: &App, platform: &dyn Platform) -> String {
    let root = &app.layout.root;
    let Some(command) = platform.hook_command(root, HookEvent::PreToolUse) else {
        return format!("✗ o hook não está instalado: rode passo init --agent {}", platform.id());
    };
    let program = passo_core::shell::lex(&command)
        .commands
        .first()
        .and_then(|c| c.words.first().map(|w| w.text.clone()))
        .unwrap_or_default();
    let resolved = if program.contains('/') {
        let path = PathBuf::from(&program);
        if !path.is_file() {
            return format!("✗ o hook aponta para {program}, que não existe: rode passo init de novo");
        }
        path
    } else {
        match on_path(&program) {
            Some(path) => path,
            None => {
                return format!(
                    "✗ comando \"{program}\" não está no PATH deste shell. Se o agente é aberto de um atalho, ele tampouco acha: rode passo init de novo, que grava o caminho absoluto"
                );
            }
        }
    };
    let env = environment();
    let version = run_with_input(&format!("'{}' --version", resolved.display()), root, "", &env, TIMEOUT)
        .map(|r| r.stdout.split_whitespace().last().unwrap_or("?").to_string())
        .unwrap_or_else(|| "?".into());
    let mine = env!("CARGO_PKG_VERSION");
    let version_note = if version == mine {
        version.clone()
    } else {
        format!("{version} (este é {mine}: rode passo init de novo)")
    };

    let payload = json!({
        "cwd": root, "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": { "command": "passo ok" },
    })
    .to_string();
    let Some(ran) = run_with_input(&command, root, &payload, &env, TIMEOUT) else {
        return format!(
            "✗ o hook não respondeu em {}s: a trava não está valendo",
            TIMEOUT.as_secs()
        );
    };
    if !ran.stdout.contains("\"permissionDecision\":\"deny\"") {
        let why = last_error(root)
            .or_else(|| ran.stderr.lines().last().map(str::to_string))
            .unwrap_or_default();
        return format!("✗ o hook respondeu \"permitir\" a \"passo ok\": a trava não está valendo (erro: {why})");
    }
    let prompt = json!({ "cwd": root, "hook_event_name": "UserPromptSubmit", "prompt": "status?" }).to_string();
    if let Some(command) = platform.hook_command(root, HookEvent::UserPrompt)
        && run_with_input(&command, root, &prompt, &env, TIMEOUT).is_some_and(|r| !r.stdout.trim().is_empty())
    {
        return "✗ o hook de prompt decidiu algo com um prompt comum (\"status?\")".into();
    }
    let events = HookEvent::ALL
        .iter()
        .filter(|e| platform.hook_command(root, **e).is_some())
        .count();
    let events_note = if events == HookEvent::ALL.len() {
        format!("{events} eventos")
    } else {
        format!(
            "✗ só {events} de {} eventos instalados: rode passo init de novo",
            HookEvent::ALL.len()
        )
    };
    format!(
        "✓ responde em {} ms · binário {} {version_note} · {events_note}",
        ran.ms,
        resolved.display()
    )
}

/// A minimal environment, as an agent launched from a desktop shortcut might have: the PATH, the home, Passo's home.
fn environment() -> Vec<(&'static str, String)> {
    ["PATH", "HOME", "PASSO_HOME"]
        .into_iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k, v)))
        .collect()
}

fn on_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|p| p.is_file())
}

fn last_error(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(passo_store::Layout::new(root).hook_errors()).ok()?;
    let last: serde_json::Value = serde_json::from_str(text.lines().last()?).ok()?;
    last.get("error").and_then(|e| e.as_str()).map(str::to_string)
}
