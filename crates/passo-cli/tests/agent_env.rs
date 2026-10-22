//! Dev decisions are refused inside an agent's environment, without depending on hooks.

use std::process::{Command, Output};

fn project() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("passo-cli-env-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).unwrap();
    let out = passo(&root, &[("PASSO_DEV", "1")], &["init", "--agent", "nenhum"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    root
}

fn passo(dir: &std::path::Path, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_passo"));
    command.env_clear().current_dir(dir).args(args);
    // Windows programs need SystemRoot to start.
    if let Some(root) = std::env::var_os("SYSTEMROOT") {
        command.env("SYSTEMROOT", root);
    }
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn dev_commands_are_refused_in_claude_codes_environment() {
    let root = project();
    for args in [
        &["ok"][..],
        &["stop"],
        &["no", "motivo"],
        &["size", "g"],
        &["change", "x"],
    ] {
        let out = passo(&root, &[("CLAUDECODE", "1")], args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", text(&out));
        assert!(
            text(&out).contains(&format!("`passo {}` é decisão do dev", args[0])),
            "{}",
            text(&out)
        );
    }
    let free = passo(&root, &[("CLAUDECODE", "1"), ("PASSO_DEV", "1")], &["ok"]);
    assert_ne!(free.status.code(), Some(2), "{}", text(&free));
    assert!(
        !text(&free).contains("decisão do dev"),
        "PASSO_DEV=1 lets the dev through: {}",
        text(&free)
    );
    let outside = passo(&root, &[], &["ok"]);
    assert!(!text(&outside).contains("decisão do dev"), "{}", text(&outside));
    let agent_command = passo(&root, &[("CLAUDECODE", "1")], &["status"]);
    assert!(
        agent_command.status.success(),
        "the agent's own commands still run: {}",
        text(&agent_command)
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_refusal_with_a_run_active_is_logged() {
    let root = std::env::temp_dir().join(format!("passo-cli-env-run-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).unwrap();
    assert!(passo(&root, &[], &["init", "--agent", "nenhum"]).status.success());
    let started = passo(&root, &[], &["start", "Algo pequeno", "--size", "p"]);
    assert!(started.status.success(), "{}", text(&started));
    let refused = passo(&root, &[("CLAUDECODE", "1")], &["ok"]);
    assert_eq!(refused.status.code(), Some(2), "{}", text(&refused));
    let id = std::fs::read_to_string(root.join(".passo/current")).unwrap();
    let log = std::fs::read_to_string(root.join(".passo/runs").join(id.trim()).join("events.jsonl")).unwrap();
    let denial = log
        .lines()
        .find(|l| l.contains("\"lock.denied\""))
        .expect("a lock.denied event");
    assert!(
        denial.contains("\"via\":\"cli\"") && denial.contains("decisão do dev"),
        "{denial}"
    );
    std::fs::remove_dir_all(root).ok();
}
