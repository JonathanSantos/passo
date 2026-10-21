//! The flow, end to end, without IO: every path a run can take.

use chrono::{DateTime, TimeZone, Utc};
use passo_core::answers::{Decision, parse_answers};
use passo_core::spec::{parse_brief, parse_plan, parse_review, question_from_cli};
use passo_core::*;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 20, 12, 0, 0).unwrap()
}

fn config() -> Config {
    let mut config = Config::default();
    config.checks.step = vec!["cargo check".into()];
    config.checks.final_ = vec!["cargo test".into()];
    config
}

fn ok_check(command: &str) -> CheckResult {
    CheckResult {
        command: command.into(),
        ok: true,
        output: String::new(),
        ms: 1,
    }
}

fn bad_check(command: &str) -> CheckResult {
    CheckResult {
        command: command.into(),
        ok: false,
        output: "erro: x".into(),
        ms: 1,
    }
}

fn facts(files: &[&str], tests: bool) -> StepFacts {
    StepFacts {
        summary: "feito".into(),
        checks: vec![ok_check("cargo check")],
        files: files.iter().map(|f| f.to_string()).collect(),
        files_total: files.len(),
        lines_total: 10,
        tests_touched: tests,
        sensitive: vec![],
        empty_ok: false,
        measured: true,
        note: None,
        unplanned: vec![],
    }
}

struct Sim<'a> {
    engine: Engine<'a>,
    run: Run,
    events: Vec<EventDraft>,
    effects: Vec<Effect>,
}

impl<'a> Sim<'a> {
    fn start(config: &'a Config, size: Size) -> Self {
        let engine = Engine::new(config);
        let (run, out) = engine.start("r1", "Adicionar prioridade às tarefas", size, now());
        let mut sim = Sim {
            engine,
            run,
            events: out.events,
            effects: out.effects,
        };
        sim.settle();
        sim
    }

    fn apply(&mut self, command: Command) -> Result<(), FlowError> {
        let (run, out) = self.engine.apply(&self.run, command, now())?;
        self.run = run;
        self.events.extend(out.events);
        self.effects.extend(out.effects);
        self.settle();
        Ok(())
    }

    /// Plays the outside world: final checks pass.
    fn settle(&mut self) {
        while let Some(pos) = self.effects.iter().position(|e| *e == Effect::RunFinalChecks) {
            self.effects.remove(pos);
            let (run, out) = self
                .engine
                .apply(&self.run, Command::FinalChecks(vec![ok_check("cargo test")]), now())
                .unwrap();
            self.run = run;
            self.events.extend(out.events);
            self.effects.extend(out.effects);
        }
    }

    fn stages_entered(&self) -> Vec<String> {
        self.events
            .iter()
            .filter(|e| e.kind == "stage.entered")
            .map(|e| e.stage.id().to_string())
            .collect()
    }

    fn gate(&self) -> GateKind {
        self.run.gate.as_ref().expect("a gate").kind
    }
}

const BRIEF: &str = r#"{"entendimento":"Tarefas ganham prioridade","escopo":["campo prioridade"],
  "perguntas":[{"pergunta":"Qual o padrão?","opcoes":[{"texto":"média","porque":"neutro"},{"texto":"baixa"}]}]}"#;
const PLAN: &str = r#"{"titulo":"Prioridade","criterios":["toda tarefa tem prioridade","filtro funciona"],
  "passos":[{"titulo":"Modelo","arquivos":["src/task.rs"],"teste":"task tem prioridade"},{"titulo":"Filtro","teste":"filtra"}]}"#;

fn to_build(sim: &mut Sim) {
    sim.apply(Command::SubmitBrief(parse_brief(BRIEF, 1).unwrap())).unwrap();
    sim.apply(Command::SubmitPlan(parse_plan(PLAN).unwrap())).unwrap();
    assert_eq!(sim.gate(), GateKind::Aprovar);
    sim.apply(Command::Approve(Decision::default())).unwrap();
}

#[test]
fn pp_fast_travel_runs_every_stage_and_asks_only_for_acceptance() {
    let config = config();
    let mut sim = Sim::start(&config, Size::Pp);
    assert_eq!(sim.run.stage, Stage::Construir, "refine and plan are automatic in pp");
    assert_eq!(sim.run.turn(), Turn::Agent);
    sim.apply(Command::StepDone(facts(&["README.md"], false))).unwrap();
    assert_eq!(sim.gate(), GateKind::Aceitar);
    assert_eq!(
        sim.stages_entered(),
        vec!["refinar", "planejar", "construir", "verificar", "entregar"]
    );
    sim.apply(Command::Approve(Decision::default())).unwrap();
    assert_eq!(sim.run.status, Status::Done);
    assert!(sim.effects.contains(&Effect::Deliver));
    assert!(matches!(sim.effects[0], Effect::Commit { .. }));
}

#[test]
fn standard_flow_with_recommended_answers() {
    let config = config();
    let mut sim = Sim::start(&config, Size::M);
    assert_eq!(sim.run.stage, Stage::Refinar);
    to_build(&mut sim);
    assert_eq!(sim.run.questions[0].answer.as_deref(), Some("A"));
    assert_eq!(sim.run.current_step().unwrap().id, "S1");
    sim.apply(Command::StepDone(facts(&["src/task.rs", "tests/task_test.rs"], true)))
        .unwrap();
    sim.apply(Command::StepDone(facts(
        &["src/filter.rs", "tests/filter_test.rs"],
        true,
    )))
    .unwrap();
    assert_eq!(sim.run.stage, Stage::Verificar);
    assert!(sim.run.reviewing);
    let review = parse_review(
        r#"{"criterios":[{"id":"C1","ok":true,"evidencia":"task_test"},{"id":"C2","ok":true,"evidencia":"filter_test"}]}"#,
        &["C1".into(), "C2".into()],
    )
    .unwrap();
    sim.apply(Command::SubmitReview(review)).unwrap();
    assert_eq!(sim.gate(), GateKind::Aceitar);
    assert_eq!(sim.run.proof, Some(Proof::Forte));
}

#[test]
fn a_different_answer_sends_the_plan_back_and_is_recorded() {
    let config = config();
    let mut sim = Sim::start(&config, Size::M);
    sim.apply(Command::SubmitBrief(parse_brief(BRIEF, 1).unwrap())).unwrap();
    sim.apply(Command::SubmitPlan(parse_plan(PLAN).unwrap())).unwrap();
    sim.apply(Command::Approve(parse_answers(&["1b".into()]))).unwrap();
    assert_eq!(sim.run.stage, Stage::Planejar);
    assert!(sim.run.feedback.as_deref().unwrap().contains("baixa"));
    let answer = sim.run.changes.iter().find(|c| c.kind == ChangeKind::Resposta).unwrap();
    assert_eq!(answer.before.as_deref(), Some("média"));
    assert_eq!(answer.after.as_deref(), Some("baixa"));
    sim.apply(Command::SubmitPlan(parse_plan(PLAN).unwrap())).unwrap();
    assert_eq!(sim.gate(), GateKind::Aprovar, "the revised plan is shown again");
    assert_eq!(sim.run.plan_version, 2);
    sim.apply(Command::Approve(Decision::default())).unwrap();
    assert_eq!(sim.run.stage, Stage::Construir);
}

#[test]
fn the_agent_cannot_move_on_the_devs_turn() {
    let config = config();
    let mut sim = Sim::start(&config, Size::M);
    sim.apply(Command::SubmitBrief(parse_brief(BRIEF, 1).unwrap())).unwrap();
    sim.apply(Command::SubmitPlan(parse_plan(PLAN).unwrap())).unwrap();
    let err = sim.apply(Command::StepDone(facts(&["a"], true))).unwrap_err();
    assert!(matches!(err, FlowError::DevTurn(_)));
}

#[test]
fn failing_checks_retry_then_call_the_dev() {
    let config = config();
    let mut sim = Sim::start(&config, Size::M);
    to_build(&mut sim);
    let mut bad = facts(&["src/task.rs", "tests/t.rs"], true);
    bad.checks = vec![bad_check("cargo check")];
    for _ in 0..2 {
        sim.apply(Command::StepDone(bad.clone())).unwrap();
        assert_eq!(sim.run.turn(), Turn::Agent);
        assert!(sim.run.feedback.as_deref().unwrap().contains("erro: x"));
    }
    sim.apply(Command::StepDone(bad)).unwrap();
    assert_eq!(sim.gate(), GateKind::Travado);
    sim.apply(Command::Approve(parse_answers(&["use o enum".into()])))
        .unwrap();
    assert_eq!(sim.run.current_step().unwrap().attempts, 0);
    assert_eq!(sim.run.feedback.as_deref(), Some("Dica do dev: use o enum"));
    assert!(sim.run.changes.iter().any(|c| c.kind == ChangeKind::Dica));
}

#[test]
fn files_outside_the_plan_are_noted_for_review() {
    let config = config();
    let mut sim = Sim::start(&config, Size::M);
    to_build(&mut sim);
    let mut f = facts(&["src/task.rs", "tests/a.rs", "tarefas.json"], true);
    f.unplanned = vec!["tarefas.json".into()];
    sim.apply(Command::StepDone(f)).unwrap();
    assert!(
        sim.run
            .notes
            .iter()
            .any(|n| n.contains("S1 mudou arquivos fora do plano: tarefas.json"))
    );
    assert!(sim.events.iter().any(|e| e.kind == "step.unplanned"));
}

#[test]
fn a_step_with_a_test_must_touch_a_test_file() {
    let config = config();
    let mut sim = Sim::start(&config, Size::M);
    to_build(&mut sim);
    sim.apply(Command::StepDone(facts(&["src/task.rs"], false))).unwrap();
    assert!(
        sim.run
            .feedback
            .as_deref()
            .unwrap()
            .contains("nenhum arquivo de teste mudou")
    );
    assert_eq!(sim.run.current_step().unwrap().id, "S1");
}

#[test]
fn pp_past_its_limit_stops_for_the_dev() {
    let config = config();
    let mut sim = Sim::start(&config, Size::Pp);
    let mut big = facts(&["a", "b", "c", "d"], false);
    big.files_total = 4;
    sim.apply(Command::StepDone(big)).unwrap();
    assert_eq!(sim.gate(), GateKind::Limite);
    sim.apply(Command::Resize(Size::P)).unwrap();
    assert_eq!(sim.run.size, Size::P);
    assert!(sim.effects.iter().any(|e| matches!(e, Effect::Commit { .. })));
}

#[test]
fn review_failures_become_fix_steps_until_the_round_limit() {
    let config = config();
    let mut sim = Sim::start(&config, Size::P);
    to_build(&mut sim);
    sim.apply(Command::StepDone(facts(&["src/task.rs", "tests/a.rs"], true)))
        .unwrap();
    sim.apply(Command::StepDone(facts(&["src/f.rs", "tests/b.rs"], true)))
        .unwrap();
    let failing = r#"{"criterios":[{"id":"C1","ok":true,"evidencia":"ok"},{"id":"C2","ok":false,"evidencia":"sem filtro vazio"}]}"#;
    let criteria = vec!["C1".to_string(), "C2".to_string()];
    for round in 1..=2 {
        sim.apply(Command::SubmitReview(parse_review(failing, &criteria).unwrap()))
            .unwrap();
        assert_eq!(sim.run.stage, Stage::Construir);
        assert!(sim.run.current_step().unwrap().fix);
        assert_eq!(sim.run.rounds, round);
        assert!(
            sim.run.current_step().unwrap().test.contains("C2"),
            "a fix for a criterion asks for a test"
        );
        sim.apply(Command::StepDone(facts(&["src/f.rs", "tests/c.rs"], true)))
            .unwrap();
    }
    sim.apply(Command::SubmitReview(parse_review(failing, &criteria).unwrap()))
        .unwrap();
    assert_eq!(sim.gate(), GateKind::Travado);
    assert_eq!(sim.run.stage, Stage::Construir);
    assert_eq!(
        sim.stages_entered().last().map(String::as_str),
        Some("construir"),
        "the stuck round still records the stage change, so the metrics attribute the time right"
    );
}

#[test]
fn proof_counts_final_checks_wherever_they_were_configured() {
    // A project may define its final checks only in `.passo/stages/verificar.md`; the engine only sees results.
    let mut config = Config::default();
    config.checks.step = vec!["cargo check".into()];
    let mut sim = Sim::start(&config, Size::M);
    to_build(&mut sim);
    sim.apply(Command::StepDone(facts(&["src/task.rs", "tests/a.rs"], true)))
        .unwrap();
    sim.apply(Command::StepDone(facts(&["src/f.rs", "tests/b.rs"], true)))
        .unwrap();
    let review = parse_review(
        r#"{"criterios":[{"id":"C1","ok":true,"evidencia":"a"},{"id":"C2","ok":true,"evidencia":"b"}]}"#,
        &["C1".into(), "C2".into()],
    )
    .unwrap();
    sim.apply(Command::SubmitReview(review)).unwrap();
    assert_eq!(sim.run.proof, Some(Proof::Forte));
}

#[test]
fn a_change_mid_build_keeps_done_steps_and_goes_through_refine() {
    let config = config();
    let mut sim = Sim::start(&config, Size::M);
    to_build(&mut sim);
    sim.apply(Command::StepDone(facts(&["src/task.rs", "tests/a.rs"], true)))
        .unwrap();
    sim.apply(Command::Change("a prioridade também ordena a lista".into()))
        .unwrap();
    assert_eq!(sim.run.stage, Stage::Refinar);
    sim.apply(Command::SubmitBrief(parse_brief(BRIEF, 2).unwrap())).unwrap();
    sim.apply(Command::SubmitPlan(parse_plan(PLAN).unwrap())).unwrap();
    sim.apply(Command::Approve(Decision::default())).unwrap();
    let ids: Vec<&str> = sim.run.steps.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, vec!["S1", "S2", "S3"]);
    assert_eq!(sim.run.steps[0].status, StepStatus::Done);
    assert!(sim.run.changes.iter().any(|c| c.kind == ChangeKind::Mudanca));
    assert!(sim.run.changes.iter().any(|c| c.kind == ChangeKind::Plano));
}

#[test]
fn rejecting_the_delivery_adds_a_fix_step() {
    let config = config();
    let mut sim = Sim::start(&config, Size::Pp);
    sim.apply(Command::StepDone(facts(&["README.md"], false))).unwrap();
    sim.apply(Command::Reject("o título ficou em inglês".into())).unwrap();
    assert_eq!(sim.run.stage, Stage::Construir);
    assert!(sim.run.current_step().unwrap().title.contains("título ficou em inglês"));
}

#[test]
fn a_question_mid_build_waits_for_the_dev() {
    let config = config();
    let mut sim = Sim::start(&config, Size::Pp);
    let q = question_from_cli(
        String::new(),
        "Manter o texto antigo?",
        &["Não — pedido".into(), "Sim".into()],
    )
    .unwrap();
    sim.apply(Command::Ask(q)).unwrap();
    assert_eq!(sim.gate(), GateKind::Pergunta);
    sim.apply(Command::Reject("só no rodapé".into())).unwrap();
    assert_eq!(sim.run.turn(), Turn::Agent);
    assert!(sim.run.notes[0].contains("só no rodapé"));
}

#[test]
fn too_many_questions_for_the_size_is_rejected_with_a_reason() {
    let config = config();
    let mut sim = Sim::start(&config, Size::P);
    let brief = r#"{"entendimento":"x","escopo":["a"],"perguntas":[
      {"pergunta":"1","opcoes":[{"texto":"a"},{"texto":"b"}]},{"pergunta":"2","opcoes":[{"texto":"a"},{"texto":"b"}]},
      {"pergunta":"3","opcoes":[{"texto":"a"},{"texto":"b"}]},{"pergunta":"4","opcoes":[{"texto":"a"},{"texto":"b"}]}]}"#;
    let err = sim
        .apply(Command::SubmitBrief(parse_brief(brief, 1).unwrap()))
        .unwrap_err();
    assert!(err.to_string().contains("no máximo 3"));
    assert_eq!(sim.run.stage, Stage::Refinar, "a rejected submission changes nothing");
}

#[test]
fn the_same_inputs_give_the_same_run() {
    let config = config();
    let play = || {
        let mut sim = Sim::start(&config, Size::M);
        to_build(&mut sim);
        sim.apply(Command::StepDone(facts(&["src/task.rs", "tests/a.rs"], true)))
            .unwrap();
        serde_json::to_string(&sim.run).unwrap()
    };
    assert_eq!(play(), play());
}
