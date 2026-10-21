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
