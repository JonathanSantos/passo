//! The app end to end, on a real git repository in a temp folder.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use passo_adapters::HookEvent;
use passo_app::{App, DevAction, DoneInput, Reporter, Snapshot, Source, hooks, setup};
use passo_core::{GateKind, Stage, Status, Turn};

/// Stands in for the HTML report: writes the run id where the report would go.
struct FileReporter;

impl Reporter for FileReporter {
    fn write(&self, snapshot: &Snapshot, path: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, snapshot.run.as_ref().map(|r| r.id.clone()).unwrap_or_default())?;
        Ok(())
    }
}

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str) -> Project {
        let root = std::env::temp_dir().join(format!("passo-app-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.txt"), "tarefas\n").unwrap();
        let project = Project {
            root: std::fs::canonicalize(&root).unwrap(),
        };
        project.git(&["init", "-q", "-b", "main"]);
        project.git(&["config", "user.email", "t@t"]);
        project.git(&["config", "user.name", "t"]);
        setup::init(&project.root, &["claude".into()], "passo").unwrap();
        std::fs::write(
            project.root.join(".passo/passo.toml"),
            "[checks]\nstep = [\"test -f src/lib.txt\"]\nfinal = [\"true\"]\n",
        )
        .unwrap();
        project.git(&["add", "-A"]);
        project.git(&["commit", "-q", "-m", "init"]);
        project
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git").args(args).current_dir(&self.root).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn app(&self) -> App {
        App::open(&self.root).unwrap()
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn draft(&self, name: &str, json: &str) -> String {
        let path = std::env::temp_dir().join(format!("passo-draft-{}-{name}.json", std::process::id()));
        std::fs::write(&path, json).unwrap();
        path.display().to_string()
    }

    fn done(&self, text: &str) -> String {
        self.app()
            .done(DoneInput {
                text: text.into(),
                ..Default::default()
            })
            .unwrap()
    }

    fn hook(&self, event: HookEvent, payload: serde_json::Value) -> Option<String> {
        hooks::handle(
            "claude",
            event,
            &payload.to_string(),
            &self.root,
            Some(Arc::new(FileReporter)),
        )
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

fn tool(root: &Path, name: &str, input: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "cwd": root, "tool_name": name, "tool_input": input })
}

#[test]
fn standard_run_branches_commits_and_merges() {
    let p = Project::new("standard");
    let out = p.app().start("Adicionar prioridade às tarefas", None).unwrap();
    assert!(out.contains("# Agente: refinador"), "{out}");
    assert!(p.git(&["rev-parse", "--abbrev-ref", "HEAD"]).starts_with("passo/"));

    // The lock: refine is read-only.
    let denied = p.hook(
        HookEvent::PreToolUse,
        tool(&p.root, "Write", serde_json::json!({"file_path": "src/x.txt"})),
    );
    assert!(denied.unwrap().contains("só leitura"));

    let brief = p.draft(
        "brief",
        r#"{"entendimento":"prioridade","escopo":["campo"],"perguntas":[
        {"pergunta":"Padrão?","opcoes":[{"texto":"média"},{"texto":"baixa"}]}]}"#,
    );
    let out = p.done(&brief);
    assert!(out.contains("# Agente: planejador"), "{out}");
    let plan = p.draft(
        "plan",
        r#"{"titulo":"Prioridade","criterios":["tem prioridade"],
        "passos":[{"titulo":"Campo","arquivos":["src/lib.txt"],"teste":"prioridade_test"}]}"#,
    );
    let out = p.done(&plan);
    assert!(out.contains("# Vez do dev") && out.contains("A ★ média"), "{out}");

    // The agent cannot approve its own plan; the dev approves from the chat.
    let denied = p.hook(
        HookEvent::PreToolUse,
        tool(&p.root, "Bash", serde_json::json!({"command": "passo ok"})),
    );
    assert!(denied.unwrap().contains("decisão do dev"));
    let reply = p
        .hook(
            HookEvent::UserPrompt,
            serde_json::json!({"cwd": p.root, "prompt": "ok 1b"}),
        )
        .unwrap();
    assert!(reply.contains("respondeu pelo chat"), "{reply}");
    let run = p.app().active().unwrap();
    assert_eq!(run.stage, Stage::Planejar, "a different answer sends the plan back");
    p.done(&plan);
    p.app().decide(DevAction::Approve(vec![]), Source::Cli).unwrap();

    // Build: a step with a test must touch a test file.
    p.write("src/lib.txt", "tarefas com prioridade\n");
    let out = p.done("campo prioridade");
    assert!(out.contains("nenhum arquivo de teste mudou"), "{out}");
    p.write("tests/prioridade_test.txt", "ok\n");
    let out = p.done("campo prioridade com teste");
    assert!(out.contains("# Agente: revisor"), "{out}");
    let review = p.draft(
        "review",
        r#"{"criterios":[{"id":"C1","ok":true,"evidencia":"tests/prioridade_test.txt"}]}"#,
    );
    let out = p.done(&review);
    assert!(out.contains("Aceitar a entrega"), "{out}");

    let run = p.app().decide(DevAction::Approve(vec![]), Source::Web).unwrap();
    assert_eq!(run.status, Status::Done);
    assert_eq!(p.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    assert!(
        std::fs::read_to_string(p.root.join("src/lib.txt"))
            .unwrap()
            .contains("prioridade")
    );
    assert!(p.git(&["log", "--oneline"]).contains("S1: Campo"));

    // Every commit of the run names the last audit event, and the log itself was merged with the code.
    let step_commit = p.git(&["log", "-1", "--format=%B", "--grep=S1: Campo"]);
    assert!(step_commit.contains("Passo-Audit: "), "{step_commit}");
    let archived = p.root.join(".passo/audit").join(&run.id).join("events.jsonl");
    assert!(archived.is_file());
    assert!(p.git(&["ls-files", ".passo/audit"]).contains("events.jsonl"));
    assert!(
        p.git(&["log", "-1", "--format=%B"]).contains("Passo-Audit: "),
        "merge commit carries the anchor"
    );

    let app = p.app();
    let log = app.store.log(&run.id);
    assert!(log.verify().unwrap().is_ok());
    assert!(app.verify_audit(&run).unwrap().is_ok());

    // Cutting lines off the end keeps the chain valid but no longer matches what git recorded.
    let text = std::fs::read_to_string(log.path()).unwrap();
    let cut: Vec<&str> = text.lines().take(3).collect();
    std::fs::write(log.path(), cut.join("\n") + "\n").unwrap();
    assert!(log.verify().unwrap().is_ok(), "a prefix is a valid chain");
    let err = app.verify_audit(&run).unwrap().unwrap_err();
    assert!(err.contains("não está no log"), "{err}");
    std::fs::write(log.path(), text).unwrap();
    let events = log.read().unwrap();
    assert!(events.iter().any(|e| e.kind == "lock.denied"));
    assert!(events.iter().any(|e| e.data["via"] == "chat"));
    let metrics = app.metrics(&run).unwrap();
    assert_eq!(metrics.dev_turns, 3);
    assert_eq!(metrics.steps_failed, 1);
    assert!(metrics.lock_denied >= 2);
}

#[test]
fn pp_goes_straight_to_build_and_stops_past_the_limit() {
    let p = Project::new("pp");
    let out = p.app().start("Trocar o título", Some(passo_core::Size::Pp)).unwrap();
    assert!(out.contains("# Agente: construtor"), "{out}");
    for i in 0..4 {
        p.write(&format!("src/f{i}.txt"), "x\n");
    }
    p.done("quatro arquivos");
    let run = p.app().active().unwrap();
    assert_eq!(run.gate.as_ref().unwrap().kind, GateKind::Limite);
    p.app().decide(DevAction::Approve(vec![]), Source::Cli).unwrap();
    let run = p.app().active().unwrap();
    assert_eq!(run.gate.as_ref().unwrap().kind, GateKind::Aceitar);
}

#[test]
fn invalid_output_is_rejected_precisely_and_counted() {
    let p = Project::new("invalid");
    p.app().start("Algo médio", None).unwrap();
    let bad = p.draft("bad", r#"{"escopo":[]}"#);
    let err = p
        .app()
        .done(DoneInput {
            text: bad,
            ..Default::default()
        })
        .unwrap_err()
        .to_string();
    assert!(err.contains("`entendimento`") && err.contains("`escopo`"), "{err}");
    let app = p.app();
    let run = app.active().unwrap();
    assert_eq!(run.stage, Stage::Refinar);
    assert_eq!(app.metrics(&run).unwrap().invalid_outputs, 1);
}

#[test]
fn stop_hook_blocks_only_on_the_agents_turn_and_gives_up_after_three() {
    let p = Project::new("stop");
    p.app().start("Algo", None).unwrap();
    let payload = serde_json::json!({ "cwd": p.root });
    for _ in 0..3 {
        assert!(p.hook(HookEvent::Stop, payload.clone()).unwrap().contains("block"));
    }
    assert!(p.hook(HookEvent::Stop, payload.clone()).is_none());
    assert_eq!(p.app().active().unwrap().turn(), Turn::Agent);
}

#[test]
fn stop_keeps_the_work_on_the_branch_and_goes_back() {
    let p = Project::new("stop-branch");
    p.app().start("Trocar o título", Some(passo_core::Size::Pp)).unwrap();
    p.write("src/lib.txt", "meio feito\n");
    p.app().decide(DevAction::Stop(String::new()), Source::Cli).unwrap();
    assert_eq!(p.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    assert_eq!(
        std::fs::read_to_string(p.root.join("src/lib.txt")).unwrap(),
        "tarefas\n"
    );
    let branch = p.git(&["branch", "--list", "passo/*", "--format=%(refname:short)"]);
    assert!(p.git(&["log", "--oneline", &branch]).contains("run interrompido"));
    assert!(
        p.git(&["ls-tree", "-r", "--name-only", &branch])
            .contains(".passo/audit/")
    );
}

#[test]
fn accepting_from_the_chat_writes_the_report() {
    let p = Project::new("chat-report");
    p.app().start("Trocar o título", Some(passo_core::Size::Pp)).unwrap();
    p.write("src/lib.txt", "título novo\n");
    p.done("título");
    let run = p.app().active().unwrap();
    assert_eq!(run.gate.as_ref().unwrap().kind, GateKind::Aceitar);
    let reply = p
        .hook(
            HookEvent::UserPrompt,
            serde_json::json!({"cwd": p.root, "prompt": "ok"}),
        )
        .unwrap();
    assert!(reply.contains("respondeu pelo chat"), "{reply}");
    let report = p.root.join(".passo/runs").join(&run.id).join("report.html");
    assert_eq!(std::fs::read_to_string(report).unwrap(), run.id);
}

#[test]
fn a_second_run_is_not_judged_by_the_first_runs_anchor() {
    let p = Project::new("second-run");
    p.app().start("Trocar o título", Some(passo_core::Size::Pp)).unwrap();
    p.write("src/lib.txt", "título novo\n");
    p.done("título");
    p.app().decide(DevAction::Approve(vec![]), Source::Cli).unwrap();
    assert!(p.git(&["log", "-1", "--format=%B"]).contains("Passo-Audit: "));
    p.app().start("Trocar a cor", Some(passo_core::Size::Pp)).unwrap();
    let run = p.app().active().unwrap();
    assert!(p.app().verify_audit(&run).unwrap().is_ok(), "no commit of its own yet");
    // Stopping right away still leaves an anchored commit behind.
    p.app().decide(DevAction::Stop(String::new()), Source::Cli).unwrap();
    let run = p.app().store.load(&run.id).unwrap();
    assert!(p.app().verify_audit(&run).unwrap().is_ok());
}

#[test]
fn off_the_run_branch_nothing_is_committed() {
    let p = Project::new("branch-guard");
    p.app().start("Trocar o título", Some(passo_core::Size::Pp)).unwrap();
    p.git(&["switch", "-q", "main"]);
    p.write("src/lib.txt", "mudança no lugar errado\n");
    let err = p
        .app()
        .done(DoneInput {
            text: "título".into(),
            ..Default::default()
        })
        .unwrap_err()
        .to_string();
    assert!(err.contains("main") && err.contains("passo/"), "{err}");
    assert_eq!(
        p.git(&["log", "--oneline"]).lines().count(),
        1,
        "main keeps only the first commit"
    );
    let err = p
        .app()
        .decide(DevAction::Stop(String::new()), Source::Cli)
        .unwrap_err()
        .to_string();
    assert!(err.contains("git switch passo/"), "{err}");
    p.git(&["checkout", "-q", "--", "src/lib.txt"]);
}

#[test]
fn dirty_tree_refuses_to_start() {
    let p = Project::new("dirty");
    p.write("src/wip.txt", "wip\n");
    let err = p.app().start("Algo", None).unwrap_err().to_string();
    assert!(err.contains("src/wip.txt"), "{err}");
}
