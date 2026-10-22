//! `passo doctor` runs the installed hook end to end.

use std::path::Path;
use std::process::{Command, Output};

fn passo(dir: &Path, path: &str, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_passo"));
    command
        .env_clear()
        .env("PATH", path)
        // A home of its own: the project folder as HOME would make its .passo/ the global one, not the project's.
        .env("HOME", dir.join("home"))
        .current_dir(dir)
        .args(args);
    // Windows programs need these to start (SystemRoot) and Git Bash to run the hook.
    for key in ["SYSTEMROOT", "TEMP", "TMP", "PATHEXT", "COMSPEC", "PASSO_SHELL"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.output().unwrap()
}

/// The built binary's folder first, then the system tools (git, sh): a minimal PATH on Unix, the runner's on Windows.
fn test_path() -> String {
    let bin_dir = Path::new(env!("CARGO_BIN_EXE_passo")).parent().unwrap().to_path_buf();
    let rest: Vec<std::path::PathBuf> = if cfg!(windows) {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect()
    } else {
        vec!["/usr/bin".into(), "/bin".into()]
    };
    std::env::join_paths(std::iter::once(bin_dir).chain(rest))
        .unwrap()
        .to_string_lossy()
        .into_owned()
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
    // The real path (macOS links /var to /private/var), as Linux has it.
    let root = passo_core::lock::plain(std::fs::canonicalize(&root).unwrap());
    let path = test_path();
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

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn project(name: &str) -> (std::path::PathBuf, String) {
    let root = std::env::temp_dir().join(format!("passo-cli-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).unwrap();
    let root = passo_core::lock::plain(std::fs::canonicalize(&root).unwrap());
    let path = test_path();
    assert!(passo(&root, &path, &["init", "--agent", "claude"]).status.success());
    (root, path)
}

#[test]
fn a_broken_config_does_not_stop_the_hook_check() {
    let (root, path) = project("doctor-config");
    std::fs::write(root.join(".passo/passo.yaml"), "checks: [isso não é um mapa\n").unwrap();
    let out = text(&passo(&root, &path, &["doctor"]));
    assert!(
        out.contains("Hook (Claude Code): ✗ o hook respondeu \"permitir\""),
        "{out}"
    );
    assert!(out.contains("✗ configuração:"), "{out}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn the_probe_leaves_no_trace_in_the_run() {
    let (root, path) = project("doctor-probe");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.txt"), "a\n").unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"]);
    git(
        &root,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );
    let start = passo(&root, &path, &["start", "trocar o texto", "--size", "pp"]);
    assert!(start.status.success(), "{}", text(&start));
    let events = || {
        let runs = root.join(".passo/runs");
        let run = std::fs::read_dir(&runs).unwrap().next().unwrap().unwrap().path();
        std::fs::read_to_string(run.join("events.jsonl"))
            .unwrap()
            .lines()
            .count()
    };
    let before = events();
    let out = text(&passo(&root, &path, &["doctor"]));
    assert!(out.contains("Hook (Claude Code): ✓"), "{out}");
    assert_eq!(events(), before, "the probe wrote to the run's log");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_changed_matcher_is_not_called_missing() {
    let (root, path) = project("doctor-matcher");
    let settings = root.join(".claude/settings.json");
    let text_before = std::fs::read_to_string(&settings).unwrap();
    std::fs::write(
        &settings,
        text_before.replace("Read|Write|Edit|MultiEdit|NotebookEdit|Glob|Grep|Bash", "Bash"),
    )
    .unwrap();
    let out = text(&passo(&root, &path, &["doctor"]));
    assert!(out.contains("matcher diferente do esperado (achei \"Bash\""), "{out}");
    std::fs::remove_dir_all(root).ok();
}
