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
            project.root.join(".passo/passo.yaml"),
            "checks:\n  step: [test -f src/lib.txt]\n  final: [\"true\"]\n",
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
        "passos":[{"titulo":"Campo","arquivos":["src/lib.txt"],"teste":"prioridade_test","criterios":["C1"]}]}"#,
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

    let out = p.app().next().unwrap();
    assert!(out.contains("Atende: C1 tem prioridade"), "{out}");
    // Build: a step with a test must touch a test file.
    p.write("src/lib.txt", "tarefas com prioridade\n");
    let out = p.done("campo prioridade");
    assert!(out.contains("nenhum arquivo de teste mudou"), "{out}");
    p.write("tests/prioridade_test.txt", "ok\n");
    let out = p.done("campo prioridade com teste");
    assert!(out.contains("# Agente: revisor"), "{out}");
    assert!(
        out.contains("C1 ← S1 ["),
        "verify shows which steps met each criterion: {out}"
    );
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
            serde_json::json!({"cwd": p.root, "prompt": "aceito"}),
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

#[test]
fn init_writes_yaml_with_the_detected_preset() {
    let root = std::env::temp_dir().join(format!("passo-app-init-yaml-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("package.json"), "{}").unwrap();
    setup::init(&root, &[], "passo").unwrap();
    let text = std::fs::read_to_string(root.join(".passo/passo.yaml")).unwrap();
    assert!(text.contains("extends: [preset:node]"), "{text}");
    assert!(
        text.contains("  # step: []") && text.contains("  # final: []"),
        "the preset's checks are not replaced"
    );
    let app = App::open(&root).unwrap();
    assert_eq!(app.config.checks.final_, vec!["npm test --silent".to_string()]);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn an_old_toml_config_stops_init_and_open() {
    let root = std::env::temp_dir().join(format!("passo-app-legacy-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(root.join(".passo")).unwrap();
    std::fs::write(root.join(".passo/passo.toml"), "default_size = \"p\"\n").unwrap();
    let message = passo_store::config::TOML_MIGRATION;
    assert_eq!(setup::init(&root, &[], "passo").err().unwrap().to_string(), message);
    assert_eq!(App::open(&root).err().unwrap().to_string(), message);
    assert!(
        !root.join(".passo/passo.yaml").exists(),
        "nothing is written before the check"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn passo_runs_the_steps_own_test() {
    let p = Project::new("rodar");
    p.app().start("Algo pequeno", Some(passo_core::Size::P)).unwrap();
    let brief = p.draft("brief-rodar", r#"{"entendimento":"x","escopo":["a"]}"#);
    p.done(&brief);
    let plan = p.draft(
        "plan-rodar",
        r#"{"titulo":"T","criterios":["existe"],
        "passos":[{"titulo":"Criar","teste":"x existe","criterios":["C1"],"rodar":"test -f tests/x.txt"}]}"#,
    );
    let out = p.done(&plan);
    assert!(out.contains("autoriza o Passo a rodar"), "{out}");
    p.app().decide(DevAction::Approve(vec![]), Source::Cli).unwrap();
    assert!(p.app().next().unwrap().contains("o Passo roda: test -f tests/x.txt"));
    p.write("src/lib.txt", "mudou\n");
    let out = p.done("sem o teste ainda");
    assert!(out.contains("a checagem `test -f tests/x.txt` falhou"), "{out}");
    p.write("tests/x.txt", "ok\n");
    let out = p.done("com o teste");
    assert!(out.contains("# Agente: revisor"), "{out}");
}

#[test]
fn a_stuck_cheap_step_asks_for_the_strong_twin() {
    let p = Project::new("escalate");
    let twin = std::fs::read_to_string(p.root.join(".claude/agents/passo-construtor-forte.md")).unwrap();
    assert!(twin.contains("model: sonnet"), "{twin}");
    p.app().start("Algo médio", None).unwrap();
    let brief = p.draft("brief-esc", r#"{"entendimento":"x","escopo":["a"]}"#);
    p.done(&brief);
    let plan = p.draft(
        "plan-esc",
        r#"{"titulo":"T","criterios":["c"],"passos":[{"titulo":"S","criterios":["C1"]}]}"#,
    );
    p.done(&plan);
    p.app().decide(DevAction::Approve(vec![]), Source::Cli).unwrap();
    std::fs::remove_file(p.root.join("src/lib.txt")).unwrap();
    let out = p.done("quebrei a checagem");
    assert!(out.contains("# Agente: construtor (nível forte)"), "{out}");
    assert!(out.contains("passo-construtor-forte"), "{out}");
}

#[test]
fn invented_evidence_is_refused_and_real_evidence_passes() {
    let p = Project::new("evidence");
    p.app().start("Algo pequeno", Some(passo_core::Size::P)).unwrap();
    let brief = p.draft("brief-ev", r#"{"entendimento":"x","escopo":["a"]}"#);
    p.done(&brief);
    let plan = p.draft(
        "plan-ev",
        r#"{"titulo":"T","criterios":["tem prova"],"passos":[{"titulo":"S","teste":"prova","criterios":["C1"]}]}"#,
    );
    p.done(&plan);
    p.app().decide(DevAction::Approve(vec![]), Source::Cli).unwrap();
    p.write("tests/a_test.txt", "teste prova_tal\n");
    let out = p.done("com teste");
    assert!(out.contains("# Agente: revisor"), "{out}");

    let invented = p.draft(
        "rev-bad",
        r#"{"criterios":[{"id":"C1","ok":true,"evidencia":"src/lib.txt:420"}]}"#,
    );
    let err = p
        .app()
        .done(DoneInput {
            text: invented,
            ..Default::default()
        })
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("`criterios[0].evidencia` cita src/lib.txt:420, mas o arquivo tem 1 linhas"),
        "{err}"
    );
    let vague = p.draft(
        "rev-vague",
        r#"{"criterios":[{"id":"C1","ok":true,"evidencia":"conferi e funciona"}]}"#,
    );
    let err = p
        .app()
        .done(DoneInput {
            text: vague,
            ..Default::default()
        })
        .unwrap_err()
        .to_string();
    assert!(err.contains("não cita arquivo, linha nem teste"), "{err}");
    let app = p.app();
    assert_eq!(app.metrics(&app.active().unwrap()).unwrap().invalid_outputs, 2);

    let real = p.draft(
        "rev-ok",
        r#"{"criterios":[{"id":"C1","ok":true,"evidencia":"`prova_tal` em tests/a_test.txt:1"}]}"#,
    );
    let out = p.done(&real);
    assert!(out.contains("Aceitar a entrega"), "{out}");
}

#[test]
fn refining_shows_the_repository_tree() {
    let p = Project::new("map");
    let out = p.app().start("Algo médio", None).unwrap();
    assert!(out.contains("Árvore do repositório:"), "{out}");
    assert!(out.contains("src/ (1)"), "{out}");
    let tree = std::fs::read_to_string(
        p.root
            .join(".passo/runs")
            .join(p.app().active().unwrap().id)
            .join("tree.md"),
    )
    .unwrap();
    assert!(tree.lines().count() <= 60);
}

#[test]
fn the_builder_sees_only_its_neighbourhood() {
    let p = Project::new("narrow");
    p.app().start("Algo médio", None).unwrap();
    let brief = p.draft("brief-narrow", r#"{"entendimento":"x","escopo":["a"]}"#);
    p.done(&brief);
    let plan = p.draft(
        "plan-narrow",
        r#"{"titulo":"T","criterios":["c"],"passos":[{"titulo":"Alfa","criterios":["C1"]},{"titulo":"Beta"},{"titulo":"Gama"},{"titulo":"Delta"}]}"#,
    );
    p.done(&plan);
    p.app().decide(DevAction::Approve(vec![]), Source::Cli).unwrap();
    p.write("src/a.txt", "a\n");
    let out = p.done("alfa feito");
    assert!(out.contains("Passo anterior: ✓ S1 Alfa — alfa feito"), "{out}");
    assert!(out.contains("Depois deste: S3 Gama"), "{out}");
    assert!(!out.contains("Delta"), "the step after next is not shown: {out}");

    let app = p.app();
    let mut run = app.active().unwrap();
    run.notes = (1..=12).map(|i| format!("Revisão: nota {i:02}")).collect();
    app.store.save(&run).unwrap();
    let out = p.app().next().unwrap();
    let shown: Vec<&str> = out.lines().filter(|l| l.starts_with("- Revisão: nota ")).collect();
    assert_eq!(shown.len(), 10, "{out}");
    assert_eq!(
        shown.first(),
        Some(&"- Revisão: nota 03"),
        "the newest ten, oldest first"
    );
    assert_eq!(shown.last(), Some(&"- Revisão: nota 12"));
}

#[test]
fn accepting_from_the_chat_needs_aceito() {
    let p = Project::new("aceito");
    p.app().start("Trocar o título", Some(passo_core::Size::Pp)).unwrap();
    p.write("src/lib.txt", "título novo\n");
    p.done("troquei o título");
    assert_eq!(p.app().active().unwrap().gate.unwrap().kind, GateKind::Aceitar);
    let reply = p
        .hook(
            HookEvent::UserPrompt,
            serde_json::json!({"cwd": p.root, "prompt": "ok"}),
        )
        .unwrap();
    assert!(reply.contains("aceito"), "{reply}");
    assert!(p.app().store.active().is_some(), "a bare ok does not merge");
    p.hook(
        HookEvent::UserPrompt,
        serde_json::json!({"cwd": p.root, "prompt": "aceito"}),
    )
    .unwrap();
    assert!(p.app().store.active().is_none(), "aceito accepts the delivery");
}

#[test]
fn the_action_budget_warns_without_blocking() {
    let p = Project::new("budget");
    let config = std::fs::read_to_string(p.root.join(".passo/passo.yaml")).unwrap();
    std::fs::write(
        p.root.join(".passo/passo.yaml"),
        format!("{config}sizes:\n  m:\n    max_tool_calls: 3\n"),
    )
    .unwrap();
    p.git(&["commit", "-qam", "budget"]);
    p.app().start("Algo médio", None).unwrap();
    let read = || {
        p.hook(
            HookEvent::PreToolUse,
            tool(&p.root, "Read", serde_json::json!({"file_path": "src/lib.txt"})),
        )
    };
    for _ in 0..3 {
        assert_eq!(read(), None);
    }
    let warning = read().unwrap();
    assert!(warning.contains("já foram 4 ações nesta etapa (limite 3)"), "{warning}");
    assert!(!warning.contains("deny"), "{warning}");
    let denied = p.hook(
        HookEvent::PreToolUse,
        tool(&p.root, "Write", serde_json::json!({"file_path": "src/x.txt"})),
    );
    assert!(denied.unwrap().contains("deny"), "the lock still denies");
    let app = p.app();
    let run = app.active().unwrap();
    assert_eq!(
        app.store
            .log(&run.id)
            .read()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "step.budget")
            .count(),
        1
    );
}

const MAP_JSON: &str = r#"{
  "projeto": {"stack": "texto puro", "testar": "true", "convencoes": ["um arquivo por assunto"], "fontes": ["src/lib.txt"]},
  "areas": [{"id": "tarefas", "caminhos": ["src/lib.txt"], "libs": ["commander"], "tags": ["tarefas"],
             "resumo": "As tarefas.", "praticas": ["texto simples"], "testes": "nenhum ainda", "fontes": ["src/lib.txt"]}],
  "libs": [{"nome": "commander", "uso": "lê os argumentos", "fontes": ["src/lib.txt"]}]
}"#;

#[test]
fn mapping_explores_read_only_and_writes_a_checked_map() {
    let p = Project::new("mapping");
    let out = p.app().map_start().unwrap();
    assert!(out.contains("# Agente: explorador"), "{out}");
    assert!(out.contains("Árvore do projeto") && out.contains("src/"), "{out}");
    assert!(p.root.join(".passo/map/.task.json").exists());
    assert!(
        p.app()
            .start("Algo", None)
            .unwrap_err()
            .to_string()
            .contains("mapeamento em andamento")
    );

    let denied = p.hook(
        HookEvent::PreToolUse,
        tool(&p.root, "Write", serde_json::json!({"file_path": "src/x.txt"})),
    );
    assert!(denied.unwrap().contains("o mapeamento é só leitura"));
    assert!(
        p.hook(HookEvent::Stop, serde_json::json!({ "cwd": p.root }))
            .unwrap()
            .contains("O Passo espera o mapa")
    );

    let wrong = p.draft(
        "map-bad",
        &MAP_JSON.replace(r#""caminhos": ["src/lib.txt"]"#, r#""caminhos": ["lib/**"]"#),
    );
    let err = p
        .app()
        .done(DoneInput {
            text: wrong,
            ..Default::default()
        })
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("`areas[0].caminhos[0]` não casa nenhum arquivo rastreado"),
        "{err}"
    );

    let good = p.draft("map-ok", MAP_JSON);
    let out = p.done(&good);
    assert!(out.contains("Mapa salvo: 1 áreas, 1 libs"), "{out}");
    for file in ["projeto.md", "areas/tarefas.md", "libs/commander.md"] {
        assert!(p.root.join(".passo/map").join(file).exists(), "{file}");
    }
    assert!(!p.root.join(".passo/map/.task.json").exists());
    let map = passo_store::map::load(&p.app().layout).unwrap().unwrap();
    assert_eq!(map.areas.len(), 1);
    assert!(
        p.hook(HookEvent::Stop, serde_json::json!({ "cwd": p.root })).is_none(),
        "the stop hook lets go after"
    );
    let log = std::fs::read_to_string(p.root.join(".passo/map/log.jsonl")).unwrap();
    assert!(log.contains("map.invalid") && log.contains("map.saved") && log.contains("lock.denied"));
}

#[test]
fn a_mapping_that_changes_files_is_refused() {
    let p = Project::new("mapping-breach");
    p.app().map_start().unwrap();
    p.write("src/lib.txt", "mudou por fora do hook\n");
    let good = p.draft("map-breach", MAP_JSON);
    let err = p
        .app()
        .done(DoneInput {
            text: good,
            ..Default::default()
        })
        .unwrap_err()
        .to_string();
    assert!(err.contains("só leitura") && err.contains("src/lib.txt"), "{err}");
}

fn write_map(p: &Project) {
    let area = |id: &str, paths: &str, tags: &str, body: &str| {
        p.write(
            &format!(".passo/map/areas/{id}.md"),
            &format!("---\npaths: [{paths}]\ntags: [{tags}]\n---\n{body}\n"),
        );
    };
    p.write(
        ".passo/map/projeto.md",
        "---\nsources: [src/lib.txt]\n---\nStack: texto puro\nTestar: `true`\n",
    );
    area("tarefas", "src/lib.txt", "tarefas", "As regras das tarefas.");
    area("relatorios", "src/rel/**", "relatório", "Os relatórios mensais.");
    area("infra", "deploy/**", "deploy", "Como o deploy é feito.");
    p.git(&["add", "-A"]);
    p.git(&["commit", "-qm", "mapa"]);
}

#[test]
fn each_stage_reads_only_its_part_of_the_map() {
    let p = Project::new("map-context");
    write_map(&p);
    let out = p.app().start("Adicionar prioridade às tarefas", None).unwrap();
    assert!(
        out.contains("Mapa do projeto:") && out.contains("tarefas (src/lib.txt)"),
        "{out}"
    );
    assert!(!out.contains("relatorios") && !out.contains("infra ("), "{out}");
    assert!(
        !out.contains("Árvore do repositório"),
        "the map takes the tree's place: {out}"
    );
    let brief = p.draft(
        "brief-map",
        r#"{"entendimento":"prioridade","escopo":["a"],"arquivos":["src/lib.txt"]}"#,
    );
    p.done(&brief);
    let plan = p.draft(
        "plan-map",
        r#"{"titulo":"T","criterios":["c"],"passos":[{"titulo":"S","arquivos":["src/lib.txt","src/novo.txt"],"criterios":["C1"]}]}"#,
    );
    p.done(&plan);
    p.app().decide(DevAction::Approve(vec![]), Source::Cli).unwrap();
    let out = p.app().next().unwrap();
    assert!(
        out.contains("tarefas (src/lib.txt)") && !out.contains("relatorios"),
        "{out}"
    );
    assert!(out.contains("Fora do mapa: src/novo.txt"), "{out}");
    let snapshot = p.app().snapshot(None).unwrap();
    assert_eq!(snapshot.map_nodes, vec!["tarefas".to_string()]);
}

#[test]
fn a_broken_map_is_a_warning_and_the_lock_still_works() {
    let p = Project::new("map-broken");
    write_map(&p);
    p.write(".passo/map/areas/infra.md", "---\npaths: [deploy/**\n---\ncorpo\n");
    p.git(&["commit", "-qam", "quebrado"]);
    let out = p.app().start("Algo", None).unwrap();
    assert!(
        out.starts_with("Aviso: mapa do projeto ignorado:") && out.contains("infra.md"),
        "{out}"
    );
    let denied = p.hook(
        HookEvent::PreToolUse,
        tool(&p.root, "Write", serde_json::json!({"file_path": "src/x.txt"})),
    );
    assert!(denied.unwrap().contains("só leitura"));
}

/// The three-area map, every node stamped with the current HEAD, committed.
fn write_dated_map(p: &Project) -> String {
    let head = p.git(&["rev-parse", "HEAD"]);
    let area = |id: &str, paths: &str, tags: &str| {
        p.write(
            &format!(".passo/map/areas/{id}.md"),
            &format!("---\npaths: [{paths}]\ntags: [{tags}]\ncommit: \"{head}\"\n---\nA área {id}.\n"),
        );
    };
    p.write(
        ".passo/map/projeto.md",
        &format!("---\ncommit: \"{head}\"\n---\nStack: texto puro\n"),
    );
    area("tarefas", "src/lib.txt", "tarefas");
    area("relatorios", "src/rel/**", "relatório");
    area("infra", "deploy/**", "deploy");
    p.git(&["add", "-A"]);
    p.git(&["commit", "-qm", "mapa"]);
    head
}

#[test]
fn the_map_ages_by_git() {
    let p = Project::new("map-stale");
    write_dated_map(&p);
    p.write("src/lib.txt", "mudou\n");
    p.git(&["commit", "-qam", "muda tarefas"]);
    let app = p.app();
    let stale = app.map_staleness(app.map.as_ref().unwrap().as_ref().unwrap()).unwrap();
    let changed: Vec<(String, bool)> = stale.areas.clone();
    assert!(
        changed.contains(&("tarefas".into(), true)) && changed.contains(&("relatorios".into(), false)),
        "{changed:?}"
    );
    assert!(!stale.worth_remapping(), "one of three is not past a third");
    let out = p.app().start("Algo", None).unwrap();
    assert!(!out.contains("Mapa do projeto feito em"), "{out}");
    stop_run(&p);
    p.write("package.json", "{}\n");
    p.git(&["add", "-A"]);
    p.git(&["commit", "-qm", "manifesto"]);
    let out = p.app().start("Algo", None).unwrap();
    assert!(
        out.starts_with("Mapa do projeto feito em") && out.contains("os manifestos mudaram"),
        "{out}"
    );
}

fn stop_run(p: &Project) {
    p.app().decide(DevAction::Stop(String::new()), Source::Cli).unwrap();
}

/// Starts a p run on the dated map, builds one step on src/lib.txt and stops at the review.
fn to_map_review(p: &Project) {
    p.app().start("Mudar as tarefas", Some(passo_core::Size::P)).unwrap();
    let brief = p.draft(
        "brief-rev",
        r#"{"entendimento":"x","escopo":["a"],"arquivos":["src/lib.txt"]}"#,
    );
    p.done(&brief);
    let plan = p.draft("plan-rev", r#"{"titulo":"T","criterios":["mudou"],"passos":[{"titulo":"S","arquivos":["src/lib.txt"],"criterios":["C1"]}]}"#);
    p.done(&plan);
    p.app().decide(DevAction::Approve(vec![]), Source::Cli).unwrap();
    p.write("src/lib.txt", "tarefas com prioridade\n");
    let out = p.done("mudou");
    assert!(
        out.contains("Inclua também `mapa`") && out.contains("(tarefas)"),
        "{out}"
    );
}

#[test]
fn the_review_confirms_the_map_and_delivery_dates_it() {
    let p = Project::new("map-confirm");
    let head = write_dated_map(&p);
    to_map_review(&p);
    let missing = p.draft(
        "rev-nomap",
        r#"{"criterios":[{"id":"C1","ok":true,"evidencia":"src/lib.txt:1"}]}"#,
    );
    let err = p
        .app()
        .done(DoneInput {
            text: missing,
            ..Default::default()
        })
        .unwrap_err()
        .to_string();
    assert!(err.contains("falta a área tarefas em `mapa`"), "{err}");
    let ok = p.draft(
        "rev-map",
        r#"{"criterios":[{"id":"C1","ok":true,"evidencia":"src/lib.txt:1"}],"mapa":[{"id":"tarefas","ok":true}]}"#,
    );
    p.done(&ok);
    p.app().decide(DevAction::Approve(vec![]), Source::Cli).unwrap();
    let map = passo_store::map::load(&p.app().layout).unwrap().unwrap();
    let commit = |id: &str| map.areas.iter().find(|a| a.id == id).unwrap().commit.clone().unwrap();
    assert_ne!(commit("tarefas"), head, "a confirmed area gets the run's last commit");
    assert_eq!(commit("relatorios"), head);
    assert!(p.git(&["log", "--oneline"]).contains("log de auditoria e mapa"));
}

#[test]
fn an_area_the_review_flags_keeps_its_commit_and_gets_a_note() {
    let p = Project::new("map-flag");
    let head = write_dated_map(&p);
    to_map_review(&p);
    let flagged = p.draft(
        "rev-flag",
        r#"{"criterios":[{"id":"C1","ok":true,"evidencia":"src/lib.txt:1"}],"mapa":[{"id":"tarefas","ok":false,"nota":"agora as tarefas têm prioridade"}]}"#,
    );
    p.done(&flagged);
    p.app().decide(DevAction::Approve(vec![]), Source::Cli).unwrap();
    let map = passo_store::map::load(&p.app().layout).unwrap().unwrap();
    let tarefas = map.areas.iter().find(|a| a.id == "tarefas").unwrap();
    assert_eq!(tarefas.commit.as_deref(), Some(head.as_str()));
    assert!(
        tarefas.body.contains("## Anotações") && tarefas.body.contains("agora as tarefas têm prioridade"),
        "{}",
        tarefas.body
    );
}

/// A project with `../<attached>` attached as `api`; `mapped` gives the attached project a one-area map.
fn with_attached(name: &str, mapped: bool) -> (Project, PathBuf) {
    let p = Project::new(name);
    let attached = p
        .root
        .parent()
        .unwrap()
        .join(format!("passo-app-api-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&attached).ok();
    std::fs::create_dir_all(attached.join("src")).unwrap();
    std::fs::write(attached.join("src/x.ts"), "export const x = 1;\n").unwrap();
    if mapped {
        std::fs::create_dir_all(attached.join(".passo/map/areas")).unwrap();
        std::fs::write(
            attached.join(".passo/map/areas/x.md"),
            "---\npaths: [src/x.ts]\n---\nO armazenamento da API.\n",
        )
        .unwrap();
    }
    let folder = attached.file_name().unwrap().to_str().unwrap().to_string();
    let config = std::fs::read_to_string(p.root.join(".passo/passo.yaml")).unwrap();
    std::fs::write(
        p.root.join(".passo/passo.yaml"),
        format!("{config}projects:\n  - name: api\n    path: ../{folder}\n"),
    )
    .unwrap();
    p.write(".passo/map/projeto.md", "---\n---\nStack: texto puro\n");
    p.write(
        ".passo/map/areas/tarefas.md",
        "---\npaths: [src/lib.txt]\ndepends_on: [api/x]\ntags: [tarefas]\n---\nAs tarefas.\n",
    );
    p.git(&["add", "-A"]);
    p.git(&["commit", "-qm", "anexado"]);
    (p, attached)
}

#[test]
fn an_attached_projects_map_is_read_with_a_prefix_and_never_written() {
    let (p, attached) = with_attached("attached", true);
    let app = p.app();
    let map = app.map.as_ref().unwrap().as_ref().unwrap();
    assert!(
        map.areas.iter().any(|a| a.id == "api/x"),
        "{:?}",
        map.areas.iter().map(|a| &a.id).collect::<Vec<_>>()
    );
    p.app().start("Adicionar prioridade às tarefas", None).unwrap();
    let brief = p.draft(
        "brief-att",
        r#"{"entendimento":"prioridade","escopo":["a"],"arquivos":["src/lib.txt"]}"#,
    );
    let out = p.done(&brief);
    let folder = attached.file_name().unwrap().to_str().unwrap();
    assert!(
        out.contains(&format!("api/x (../{folder}/src/x.ts)")),
        "planning hops to the attached area: {out}"
    );
    let denied = p.hook(
        HookEvent::PreToolUse,
        tool(
            &p.root,
            "Write",
            serde_json::json!({"file_path": attached.join("src/x.ts")}),
        ),
    );
    assert!(denied.unwrap().contains("projeto anexado"));
    std::fs::remove_dir_all(attached).ok();
}

#[test]
fn an_attached_project_without_a_map_is_a_warning() {
    let (p, attached) = with_attached("attached-nomap", false);
    let out = p.app().start("Algo", None).unwrap();
    assert!(out.contains("projeto anexado api sem mapa"), "{out}");
    std::fs::remove_dir_all(attached).ok();
}

/// A p run with one step that wrote tests/a_test.txt (with `prova_tal` in it), stopped at the review.
fn to_review(p: &Project) {
    p.app().start("Algo pequeno", Some(passo_core::Size::P)).unwrap();
    let brief = p.draft("brief-ev2", r#"{"entendimento":"x","escopo":["a"]}"#);
    p.done(&brief);
    let plan = p.draft(
        "plan-ev2",
        r#"{"titulo":"T","criterios":["tem prova"],"passos":[{"titulo":"S","teste":"prova","criterios":["C1"]}]}"#,
    );
    p.done(&plan);
    p.app().decide(DevAction::Approve(vec![]), Source::Cli).unwrap();
    p.write("tests/a_test.txt", "teste prova_tal\n");
    assert!(p.done("com teste").contains("# Agente: revisor"));
}

fn review_error(p: &Project, name: &str, evidence: &str) -> String {
    let json = format!(r#"{{"criterios":[{{"id":"C1","ok":true,"evidencia":"{evidence}"}}]}}"#);
    let file = p.draft(name, &json);
    match p.app().done(DoneInput {
        text: file,
        ..Default::default()
    }) {
        Ok(out) => format!("ACEITA: {out}"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn evidence_must_be_a_file_inside_the_project() {
    let p = Project::new("evidence-inside");
    to_review(&p);
    let id = p.app().active().unwrap().id;
    let cases = [
        ("/etc/hosts:1", "cita /etc/hosts, que está fora do projeto"),
        ("../../x.rs:1", "cita ../../x.rs, que está fora do projeto"),
        ("src/", "cita src/, que é uma pasta: aponte o arquivo"),
        ("target/debug/x.rs", "cita target/debug/x.rs, que está fora do projeto"),
    ];
    for (i, (evidence, message)) in cases.iter().enumerate() {
        let err = review_error(&p, &format!("rev-in-{i}"), evidence);
        assert!(
            err.contains(&format!("`criterios[0].evidencia` {message}")),
            "{evidence}: {err}"
        );
    }
    let passo = format!(".passo/runs/{id}/run.json");
    assert!(review_error(&p, "rev-in-passo", &passo).contains("que está fora do projeto"));
    let invented = review_error(&p, "rev-in-sym-bad", "`teste_que_ninguem_escreveu`");
    assert!(invented.contains("nada disso existe no projeto"), "{invented}");
    let symbol = review_error(&p, "rev-in-sym", "`prova_tal`");
    assert!(
        symbol.starts_with("ACEITA") && symbol.contains("Aceitar a entrega"),
        "{symbol}"
    );
}

#[test]
fn the_budget_counts_allowed_calls_per_step_and_attempt() {
    let p = Project::new("budget-step");
    let config = std::fs::read_to_string(p.root.join(".passo/passo.yaml")).unwrap();
    std::fs::write(
        p.root.join(".passo/passo.yaml"),
        format!("{config}sizes:\n  m:\n    max_tool_calls: 3\n"),
    )
    .unwrap();
    p.git(&["commit", "-qam", "budget"]);
    p.app().start("Algo médio", None).unwrap();
    let brief = p.draft("brief-budget", r#"{"entendimento":"x","escopo":["a"]}"#);
    p.done(&brief);
    let plan = p.draft(
        "plan-budget",
        r#"{"titulo":"T","criterios":["c"],"passos":[{"titulo":"S","criterios":["C1"]}]}"#,
    );
    p.done(&plan);
    p.app().decide(DevAction::Approve(vec![]), Source::Cli).unwrap();
    let read = || {
        p.hook(
            HookEvent::PreToolUse,
            tool(&p.root, "Read", serde_json::json!({"file_path": "src/lib.txt"})),
        )
    };
    for _ in 0..3 {
        assert_eq!(read(), None);
    }
    let denied = p
        .hook(
            HookEvent::PreToolUse,
            tool(&p.root, "Write", serde_json::json!({"file_path": ".env"})),
        )
        .unwrap();
    assert!(
        denied.contains("deny") && !denied.contains("ações neste passo"),
        "{denied}"
    );
    let warning = read().unwrap();
    assert!(
        warning.contains("já foram 4 ações neste passo"),
        "the denied call did not count: {warning}"
    );
    std::fs::remove_file(p.root.join("src/lib.txt")).unwrap();
    p.done("quebrei a checagem");
    assert_eq!(read(), None, "a new attempt starts a new count");
}

#[test]
fn copilot_gets_the_strong_twin_too() {
    let root = std::env::temp_dir().join(format!("passo-app-copilot-twin-{}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).unwrap();
    setup::init(&root, &["copilot".into()], "passo").unwrap();
    let twin = std::fs::read_to_string(root.join(".github/agents/passo-construtor-forte.agent.md")).unwrap();
    assert!(twin.contains("model: \"Claude Sonnet 5.5\""), "{twin}");
    let cheap = std::fs::read_to_string(root.join(".github/agents/passo-construtor.agent.md")).unwrap();
    assert!(cheap.contains("model: \"Claude Haiku 5.5\""), "{cheap}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn the_tree_leaves_tooling_out_and_a_change_redraws_it() {
    let p = Project::new("tree-redraw");
    p.write(".passo/audit/antigo/events.jsonl", "{}\n");
    p.git(&["add", "-A"]);
    p.git(&["commit", "-qm", "auditoria rastreada"]);
    p.app().start("Algo médio", None).unwrap();
    let id = p.app().active().unwrap().id;
    let tree = || std::fs::read_to_string(p.root.join(".passo/runs").join(&id).join("tree.md")).unwrap();
    assert!(!tree().contains(".passo") && !tree().contains(".git"), "{}", tree());
    p.write("docs/guia.md", "guia\n");
    p.git(&["add", "-A"]);
    p.git(&["commit", "-qm", "docs"]);
    assert!(!tree().contains("docs/"));
    p.app()
        .decide(DevAction::Change("também um guia".into()), Source::Cli)
        .unwrap();
    assert!(tree().contains("docs/ (1)"), "{}", tree());
}

#[test]
fn secrets_stay_out_of_the_log_the_run_and_the_report() {
    let p = Project::new("secrets");
    std::fs::write(
        p.root.join(".passo/passo.yaml"),
        "checks:\n  step: [\"echo API_KEY=0123456789abcdef0123; test -f src/ok.txt\"]\n  final: [\"true\"]\n",
    )
    .unwrap();
    p.git(&["commit", "-qam", "segredo"]);
    p.app().start("Trocar o título", Some(passo_core::Size::Pp)).unwrap();
    p.hook(
        HookEvent::PreToolUse,
        tool(
            &p.root,
            "Bash",
            serde_json::json!({"command": "curl -H \"Authorization: Bearer abc123def456\" https://api.x"}),
        ),
    );
    p.write("src/lib.txt", "novo\n");
    p.done("troquei");
    let app = p.app();
    let run = app.active().unwrap();
    let log = std::fs::read_to_string(p.root.join(".passo/runs").join(&run.id).join("events.jsonl")).unwrap();
    assert!(log.contains("«oculto»") && !log.contains("abc123"), "{log}");
    let saved = std::fs::read_to_string(p.root.join(".passo/runs").join(&run.id).join("run.json")).unwrap();
    assert!(!saved.contains("0123456789abcdef0123"), "{saved}");
    assert!(saved.contains("API_KEY=«oculto»"));
    let snapshot = serde_json::to_string(&app.snapshot(None).unwrap()).unwrap();
    assert!(!snapshot.contains("0123456789abcdef0123") && !snapshot.contains("abc123def456"));

    // The final run.json goes into git with the audit copy: clean there too.
    app.decide(DevAction::Stop(String::new()), Source::Cli).unwrap();
    let branch = p.git(&["branch", "--list", "passo/*", "--format=%(refname:short)"]);
    let archived = p.git(&["show", &format!("{branch}:.passo/audit/{}/run.json", run.id)]);
    assert!(archived.contains("API_KEY=«oculto»") && !archived.contains("0123456789abcdef0123"));
}

#[test]
fn a_failing_hook_is_recorded_and_announced() {
    let p = Project::new("hook-fail");
    p.app().start("Algo médio", None).unwrap();
    let config = p.root.join(".passo/passo.yaml");
    let good = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, format!("{good}chave_errada: 1\n")).unwrap();
    let reply = p.hook(
        HookEvent::PreToolUse,
        tool(&p.root, "Write", serde_json::json!({"file_path": "src/x.txt"})),
    );
    assert_eq!(reply, None, "a broken hook still lets the call through");
    let errors = std::fs::read_to_string(p.root.join(".passo/hook-errors.log")).unwrap();
    assert!(
        errors.contains("chave_errada") && errors.contains("pre-tool-use"),
        "{errors}"
    );
    std::fs::write(&config, good).unwrap();
    let out = p.app().next().unwrap();
    assert!(
        out.starts_with("⚠ 1 erro(s) de hook neste run") && out.contains("passo doctor"),
        "{out}"
    );
    let run = p.app().active().unwrap();
    let log = std::fs::read_to_string(p.root.join(".passo/runs").join(&run.id).join("events.jsonl")).unwrap();
    assert_eq!(log.matches("\"hook.failed\"").count(), 1);
}

/// A finished run with the dev's rejections, as if it had happened.
fn past_run(p: &Project, id: &str, rejections: &[&str]) {
    let now = chrono::Utc::now();
    let mut run = passo_core::Run::new(id, "pedido antigo", passo_core::Size::P, now);
    run.status = Status::Done;
    for text in rejections {
        run.changes.push(passo_core::model::Change {
            at: now,
            by: passo_core::Actor::Dev,
            kind: passo_core::model::ChangeKind::Recusa,
            stage: Stage::Entregar,
            text: text.to_string(),
            before: None,
            after: None,
            step: None,
        });
    }
    p.app().store.save(&run).unwrap();
}

#[test]
fn learning_turns_repeated_rejections_into_a_convention() {
    let p = Project::new("learn");
    past_run(&p, "r1", &["a listagem veio em ordem alfabética"]);
    past_run(&p, "r2", &["listagem deve seguir a ordem de criação, não alfabética"]);
    past_run(&p, "r3", &["faltou testar o erro de rede"]);
    let status = p.app().status_text();
    assert!(status.contains("Há 3 lições novas de 3 runs"), "{status}");

    let task = p.app().learn_start().unwrap();
    assert!(task.contains("se repete em 2 runs"), "{task}");
    assert!(
        task.contains("run r1: recusa: a listagem veio em ordem alfabética"),
        "{task}"
    );
    assert!(task.contains("refinador-forte"), "{task}");

    // Read-only while the agent works, and it may not stop before handing in.
    let write = tool(
        &p.root,
        "Write",
        serde_json::json!({"file_path": p.root.join("src/x.txt")}),
    );
    assert!(p.hook(HookEvent::PreToolUse, write).unwrap().contains("só leitura"));
    assert!(
        p.hook(HookEvent::Stop, serde_json::json!({ "cwd": p.root }))
            .unwrap()
            .contains("block")
    );

    let wrong = p.draft(
        "learn-wrong",
        r#"{"propostas":[{"texto":"Listas em ordem de criação","destino":"mapa:x","fontes":["run r1: recusa: a listagem veio em ordem alfabética"]}]}"#,
    );
    let err = p
        .app()
        .done(DoneInput {
            text: wrong,
            ..Default::default()
        })
        .unwrap_err()
        .to_string();
    assert!(err.contains("`propostas[0].destino`") && err.contains("`x`"), "{err}");

    let good = p.draft(
        "learn-good",
        r#"{"propostas":[{"texto":"Listagens saem em ordem de criação, nunca alfabética","destino":"context","fontes":["run r1: recusa: a listagem veio em ordem alfabética","run r2: recusa: listagem deve seguir a ordem de criação, não alfabética"]}]}"#,
    );
    let out = p.done(&good);
    assert!(
        out.contains("1. Listagens saem em ordem de criação") && out.contains("--apply"),
        "{out}"
    );
    assert!(p.hook(HookEvent::Stop, serde_json::json!({ "cwd": p.root })).is_none());

    let applied = p.app().learn_apply("1").unwrap();
    assert!(applied.contains("Aplicadas"), "{applied}");
    let context = std::fs::read_to_string(p.root.join(".passo/context.md")).unwrap();
    assert!(
        context.contains(
            "## Aprendido com os runs\n- Listagens saem em ordem de criação, nunca alfabética <!-- fontes: run r1"
        ),
        "{context}"
    );
    let seen: Vec<String> =
        serde_json::from_str(&std::fs::read_to_string(p.root.join(".passo/learn/seen.json")).unwrap()).unwrap();
    assert_eq!(seen.len(), 3);
    assert!(p.app().learn_hint().is_none());

    p.git(&["add", "-A"]);
    p.git(&["commit", "-q", "-m", "aprendido"]);
    p.app().start("Listar tarefas", Some(passo_core::Size::P)).unwrap();
    let next = p.app().next().unwrap();
    assert!(
        next.contains("Convenções do projeto") && next.contains("Listagens saem em ordem de criação"),
        "{next}"
    );
}

#[test]
fn a_run_past_its_token_limit_stops_for_the_dev_and_the_transcript_is_read_once() {
    let p = Project::new("tokens");
    p.app().start("Algo grande", Some(passo_core::Size::M)).unwrap();
    let run = p.app().active().unwrap();
    let transcript = p.root.join("../passo-tokens-transcript.jsonl");
    let at = (chrono::Utc::now() + chrono::Duration::seconds(1)).to_rfc3339();
    let line = |input: &str| {
        format!(
            r#"{{"type":"assistant","timestamp":"{at}","message":{{"id":"m1","model":"x","usage":{{"input_tokens":{input},"output_tokens":100000}}}}}}"#
        ) + "\n"
    };
    std::fs::write(&transcript, line("400000")).unwrap();
    p.app()
        .store
        .add_transcript(
            &run.id,
            passo_store::Transcript {
                platform: "claude".into(),
                path: transcript.display().to_string(),
            },
        )
        .unwrap();

    let next = p.app().next().unwrap();
    let run = p.app().active().unwrap();
    assert_eq!(run.tokens, 500_000);
    assert_eq!(run.gate.as_ref().unwrap().kind, GateKind::Tokens);
    assert!(next.contains("Tokens: 500k de 400k (tamanho m)"), "{next}");
    assert!(p.app().status_text().contains("Tokens: 500k de 400k"));
    let stop = p.hook(HookEvent::Stop, serde_json::json!({ "cwd": p.root })).unwrap();
    assert!(stop.contains("systemMessage") && stop.contains("Tokens:"), "{stop}");

    // Same length, same time: the cache answers and the file is not read again.
    let cache: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(p.root.join(".passo/runs").join(&run.id).join("usage.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(cache[0]["len"], std::fs::metadata(&transcript).unwrap().len());
    let mtime = std::fs::metadata(&transcript).unwrap().modified().unwrap();
    std::fs::write(&transcript, line("900000")).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&transcript)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    p.app().next().unwrap();
    assert_eq!(p.app().active().unwrap().tokens, 500_000);

    p.app().decide(DevAction::Approve(vec![]), Source::Cli).unwrap();
    let run = p.app().active().unwrap();
    assert!(run.tokens_accepted && run.gate.is_none());
    assert_eq!(run.stage, Stage::Refinar);
    std::fs::remove_file(&transcript).ok();
}

#[test]
fn the_series_reads_every_run_even_the_archived_ones() {
    let p = Project::new("series");
    for (id, day) in [("s1", 1), ("s2", 5), ("s3", 9)] {
        let at = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 10, day, 12, 0, 0).unwrap();
        let mut run = passo_core::Run::new(id, "pedido", passo_core::Size::M, at);
        run.status = Status::Done;
        run.updated_at = at + chrono::Duration::minutes(10);
        p.app().store.save(&run).unwrap();
        p.app().store.archive(id).unwrap();
    }
    // A fresh clone has only the archived copy.
    std::fs::remove_dir_all(p.root.join(".passo/runs/s1")).unwrap();

    let agg = p.app().stats_all(None).unwrap();
    assert_eq!(agg.runs.len(), 3);
    let text = passo_app::render::series(&agg);
    assert!(text.contains("Runs (3)") && text.contains("m: 3 runs"), "{text}");
    assert_eq!(text.lines().filter(|l| l.contains("concluído")).count(), 3, "{text}");

    let since = passo_app::metrics::day("2026-10-05");
    assert_eq!(p.app().stats_all(since).unwrap().runs.len(), 2);

    let json: serde_json::Value = serde_json::to_value(&agg).unwrap();
    assert!(json["runs"].is_array() && json["by_size"][0]["runs"] == 3, "{json}");
}

#[test]
fn diff_coverage_reaches_the_reviewer() {
    let p = Project::new("coverage");
    p.write(
        ".passo/passo.yaml",
        "checks:\n  step: [test -f src/lib.txt]\n  final: [\"true\"]\n  coverage:\n    command: printf 'SF:src/lib.txt\\nDA:1,1\\nDA:2,1\\nDA:3,0\\nDA:4,0\\nend_of_record\\n' > {out}\n    min_diff: 70\n",
    );
    p.git(&["add", "-A"]);
    p.git(&["commit", "-q", "-m", "cobertura"]);
    p.app().start("Mudar as tarefas", Some(passo_core::Size::P)).unwrap();
    let brief = p.draft(
        "brief-cov",
        r#"{"entendimento":"x","escopo":["a"],"arquivos":["src/lib.txt"]}"#,
    );
    p.done(&brief);
    let plan = p.draft(
        "plan-cov",
        r#"{"titulo":"T","criterios":["mudou"],"passos":[{"titulo":"S","arquivos":["src/lib.txt"],"criterios":["C1"]}]}"#,
    );
    p.done(&plan);
    p.app().decide(DevAction::Approve(vec![]), Source::Cli).unwrap();
    p.write("src/lib.txt", "um\ndois\ntrês\nquatro\n");
    let next = p.done("mudou");

    let run = p.app().active().unwrap();
    let coverage = run.coverage.as_ref().expect("coverage measured");
    assert_eq!((coverage.percent, coverage.lines), (50, 4));
    assert!(run.reviewing);
    let log = std::fs::read_to_string(p.root.join(".passo/runs").join(&run.id).join("events.jsonl")).unwrap();
    assert!(
        log.contains("Cobertura do diff: 50% (2 linha(s) sem teste em 1 arquivo(s))"),
        "{log}"
    );
    assert!(
        next.contains("Linhas mudadas que nenhum teste executa: src/lib.txt 3-4"),
        "{next}"
    );
}
