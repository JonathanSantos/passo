//! `passo doctor` runs the installed hook end to end.

use std::path::Path;
use std::process::{Command, Output};

fn passo(dir: &Path, path: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_passo"))
        .env_clear()
        .env("PATH", path)
        .env("HOME", dir)
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn doctor_proves_the_hook_answers() {
    let root = std::env::temp_dir().join(format!("passo-cli-doctor-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).unwrap();
    let bin_dir = Path::new(env!("CARGO_BIN_EXE_passo"))
        .parent()
        .unwrap()
        .display()
        .to_string();
    let path = format!("{bin_dir}:/usr/bin:/bin");
    let init = passo(&root, &path, &["init", "--agent", "claude"]);
    assert!(
        init.status.success() && text(&init).contains("Confira com: passo doctor"),
        "{}",
        text(&init)
    );

    let doctor = passo(&root, &path, &["doctor"]);
    let out = text(&doctor);
    assert!(out.contains("Hook (Claude Code): ✓ responde em"), "{out}");
    assert!(out.contains("· 5 eventos"), "{out}");

    let settings = root.join(".claude/settings.json");
    let broken = std::fs::read_to_string(&settings)
        .unwrap()
        .replace("\"passo hook", "\"/caminho/que/nao/existe/passo hook");
    std::fs::write(&settings, broken).unwrap();
    let out = text(&passo(&root, &path, &["doctor"]));
    assert!(
        out.contains("Hook (Claude Code): ✗ o hook aponta para /caminho/que/nao/existe/passo"),
        "{out}"
    );
    std::fs::remove_dir_all(root).ok();
}
