//! `passo doctor` for hooks: runs each installed platform's hook end to end and says whether the lock holds.
//! A hook that fails lets everything through, so this is the place that proves it does not. It needs only the
//! layout: an invalid `passo.yaml`, the case that most often breaks a hook, must not stop it.
//!
//! The probe runs with `PASSO_PROBE=1`: the hook judges and answers, and writes nothing (no event, no budget, no
//! transcript, no decision taken from the chat).

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use passo_adapters::{HookEvent, Platform};
use passo_store::Layout;

use crate::infra::run_with_input;

const TIMEOUT: Duration = Duration::from_secs(5);

/// One line per installed platform: `Hook (…): ✓ responde em 14 ms · binário … · 5 eventos`, or what is wrong.
pub fn check_hooks(layout: &Layout) -> Vec<String> {
    let started = Utc::now();
    passo_adapters::registry()
        .into_iter()
        .filter(|p| p.installed(&layout.root))
        .map(|p| format!("Hook ({}): {}", p.label(), check(&layout.root, p.as_ref(), started)))
        .collect()
}

/// `Erros de hook: 3 registrados; o último em 21/10 22:05: …`, when there are any.
pub fn hook_errors(layout: &Layout) -> Option<String> {
    let text = std::fs::read_to_string(layout.hook_errors()).ok()?;
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let last: serde_json::Value = serde_json::from_str(lines.last()?).ok()?;
    let when = last
        .get("t")
        .and_then(|t| t.as_str())
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&chrono::Local).format("%d/%m %H:%M").to_string())
        .unwrap_or_default();
    let error = last.get("error").and_then(|e| e.as_str()).unwrap_or("");
    Some(format!(
        "Erros de hook: {} registrado(s); o último em {when}: {error}",
        lines.len()
    ))
}

fn check(root: &Path, platform: &dyn Platform, started: DateTime<Utc>) -> String {
    let Some(command) = platform.hook_command(root, HookEvent::PreToolUse) else {
        if let Some((found, expected)) = platform.hook_matcher(root, HookEvent::PreToolUse) {
            return format!(
                "✗ instalado com matcher diferente do esperado (achei \"{found}\", esperava \"{expected}\"): rode passo init --agent {}",
                platform.id()
            );
        }
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

    let payload = platform.probe(root, HookEvent::PreToolUse).to_string();
    let Some(ran) = run_with_input(&command, root, &payload, &env, TIMEOUT) else {
        return format!(
            "✗ o hook não respondeu em {}s: a trava não está valendo",
            TIMEOUT.as_secs()
        );
    };
    let reply: serde_json::Value = serde_json::from_str(ran.stdout.trim()).unwrap_or_default();
    if !platform.is_deny(&reply) {
        let why = last_error(root, started)
            .or_else(|| ran.stderr.lines().last().map(str::to_string))
            .unwrap_or_default();
        return format!("✗ o hook respondeu \"permitir\" a \"passo ok\": a trava não está valendo (erro: {why})");
    }
    let prompt = platform.probe(root, HookEvent::UserPrompt).to_string();
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

/// A minimal environment, as an agent launched from a desktop shortcut might have: the PATH, the home, Passo's home;
/// and the probe flag, so the hook writes nothing.
fn environment() -> Vec<(&'static str, String)> {
    let mut env: Vec<(&'static str, String)> = ["PATH", "HOME", "PASSO_HOME"]
        .into_iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k, v)))
        .collect();
    env.push(("PASSO_PROBE", "1".into()));
    env
}

fn on_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|p| p.is_file())
}

/// The hook's own error from this probe: only lines written after `since`, never an old one.
fn last_error(root: &Path, since: DateTime<Utc>) -> Option<String> {
    let text = std::fs::read_to_string(Layout::new(root).hook_errors()).ok()?;
    let last: serde_json::Value = serde_json::from_str(text.lines().last()?).ok()?;
    let at = last
        .get("t")
        .and_then(|t| t.as_str())
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())?;
    (at >= since).then(|| last.get("error").and_then(|e| e.as_str()).map(str::to_string))?
}
