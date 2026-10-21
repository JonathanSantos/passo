//! The flow: a pure state machine. `(run, command, facts) -> (run', events, effects)`.
//!
//! Nothing here touches the disk, git or a clock: the caller measures the world (check results, the diff) and hands
//! the facts in; the engine decides; the caller carries out the effects. Same input, same output, every time.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use thiserror::Error;

use crate::answers::{Answer, Decision};
use crate::config::{Config, Mode, Profile};
use crate::lock::{self, ToolKind, ToolUse, Verdict};
use crate::model::*;
use crate::spec::{Brief, Invalid, Plan, Question, Review};

/// What the engine asks the outside world to do after a transition.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Commit the current changes as this step.
    Commit { step: String, message: String },
    /// Run the final checks and answer with `Command::FinalChecks`.
    RunFinalChecks,
    /// Write the delivery report.
    WriteReport,
    /// The dev accepted: merge or keep the branch.
    Deliver,
}

/// An event before the store stamps it (time, sequence, hash).
#[derive(Debug, Clone, PartialEq)]
pub struct EventDraft {
    pub kind: &'static str,
    pub actor: Actor,
    pub stage: Stage,
    pub text: String,
    pub data: Value,
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub events: Vec<EventDraft>,
    pub effects: Vec<Effect>,
}

/// What Passo measured after the agent said a step was done.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StepFacts {
    pub summary: String,
    pub checks: Vec<CheckResult>,
    /// Files changed by this step.
    pub files: Vec<String>,
    /// Size of the whole run's diff so far.
    pub files_total: usize,
    pub lines_total: usize,
    pub tests_touched: bool,
    /// Sensitive paths in the run's diff.
    pub sensitive: Vec<String>,
    /// The agent said this step needs no file change.
    pub empty_ok: bool,
    /// False when the project has no git: diff facts are unknown and not judged.
    pub measured: bool,
    /// Something the agent saw outside the step, kept for the dev.
    pub note: Option<String>,
    /// Files this step changed that the plan did not list for it (tests excluded).
    pub unplanned: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Command {
    SubmitBrief(Brief),
    SubmitPlan(Plan),
    StepDone(StepFacts),
    FinalChecks(Vec<CheckResult>),
    SubmitReview(Review),
    Ask(Question),
    Approve(Decision),
    Reject(String),
    Change(String),
    Resize(Size),
    Stop(String),
}

impl Command {
    pub fn actor(&self) -> Actor {
        match self {
            Command::SubmitBrief(_)
            | Command::SubmitPlan(_)
            | Command::StepDone(_)
            | Command::SubmitReview(_)
            | Command::Ask(_) => Actor::Agent,
            Command::FinalChecks(_) => Actor::Passo,
            _ => Actor::Dev,
        }
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum FlowError {
    #[error("o run {0} já terminou")]
    Closed(String),
    #[error("agora é a vez do dev ({0}); o agente espera")]
    DevTurn(String),
    #[error("não há nada esperando o dev agora")]
    NoGate,
    #[error("{0}")]
    Wrong(String),
    #[error("a entrega tem problemas:\n{0}")]
    Invalid(Invalid),
}

pub struct Engine<'a> {
    pub config: &'a Config,
}

impl<'a> Engine<'a> {
    pub fn new(config: &'a Config) -> Self {
        Engine { config }
    }

    fn profile(&self, run: &Run) -> Profile {
        self.config.profile(run.size)
    }

    /// Creates a run and walks it to its first turn.
    pub fn start(&self, id: &str, request: &str, size: Size, now: DateTime<Utc>) -> (Run, Outcome) {
        let mut run = Run::new(id, request.trim(), size, now);
        let mut out = Outcome::default();
        out.event(
            &run,
            "run.started",
            Actor::Agent,
            format!("Pedido ({}): {}", size.label(), run.request),
        );
        self.enter(&mut run, Stage::Refinar, &mut out, now);
        (run, out)
    }

    /// Applies a command to a copy of the run. On error the run is untouched.
    pub fn apply(&self, run: &Run, command: Command, now: DateTime<Utc>) -> Result<(Run, Outcome), FlowError> {
        if !run.active() {
            return Err(FlowError::Closed(run.id.clone()));
        }
        let mut next = run.clone();
        let mut out = Outcome::default();
        let actor = command.actor();
        if actor == Actor::Agent
            && let Some(gate) = &run.gate
        {
            return Err(FlowError::DevTurn(gate.kind.label().into()));
        }
        match command {
            Command::SubmitBrief(brief) => self.submit_brief(&mut next, brief, &mut out, now)?,
            Command::SubmitPlan(plan) => self.submit_plan(&mut next, plan, &mut out, now)?,
            Command::StepDone(facts) => self.step_done(&mut next, facts, &mut out, now)?,
            Command::FinalChecks(results) => self.final_checks(&mut next, results, &mut out, now)?,
            Command::SubmitReview(review) => self.submit_review(&mut next, review, &mut out, now)?,
            Command::Ask(question) => self.ask(&mut next, question, &mut out, now)?,
            Command::Approve(decision) => self.approve(&mut next, decision, &mut out, now)?,
            Command::Reject(text) => self.reject(&mut next, text, &mut out, now)?,
            Command::Change(text) => self.change(&mut next, text, &mut out, now)?,
            Command::Resize(size) => self.resize(&mut next, size, &mut out, now)?,
            Command::Stop(reason) => {
                next.status = Status::Aborted;
                next.gate = None;
                let text = if reason.is_empty() {
                    "Run interrompido pelo dev".into()
                } else {
                    reason
                };
                out.event(&next, "run.stopped", Actor::Dev, text);
            }
        }
        next.updated_at = now;
        Ok((next, out))
    }

    // -- stages ----------------------------------------------------------------------------------------------

    /// Moves the run to a stage and records it. Every stage change goes through here, so the log always has a
    /// `stage.entered` for each visit and the metrics can attribute time to the right stage.
    fn announce(&self, run: &mut Run, stage: Stage, out: &mut Outcome) -> Mode {
        run.stage = stage;
        run.reviewing = false;
        let mode = self.profile(run).mode(stage);
        let who = match (stage, mode) {
            (Stage::Entregar, _) => "o Passo prepara a entrega",
            (Stage::Verificar, Mode::Auto) => "só as checagens",
            (_, Mode::Auto) => "automático",
            (_, Mode::Agente) => "agente",
        };
        out.event(run, "stage.entered", Actor::Passo, format!("{} ({who})", stage.label()));
        mode
    }

    fn enter(&self, run: &mut Run, stage: Stage, out: &mut Outcome, now: DateTime<Utc>) {
        let mode = self.announce(run, stage, out);
        match stage {
            Stage::Refinar if mode == Mode::Auto => {
                run.brief = Some(auto_brief(run));
                out.event(
                    run,
                    "brief.auto",
                    Actor::Passo,
                    "Pedido usado como está (tamanho sem refinamento)".into(),
                );
                self.enter(run, Stage::Planejar, out, now);
            }
            Stage::Planejar if mode == Mode::Auto => {
                let plan = auto_plan(run);
                self.accept_plan(run, plan, Actor::Passo, out, now);
            }
            Stage::Construir => self.next_step(run, out, now),
            Stage::Verificar => out.effects.push(Effect::RunFinalChecks),
            Stage::Entregar => {
                let proof = self.proof(run);
                run.proof = Some(proof);
                out.effects.push(Effect::WriteReport);
                let detail = format!(
                    "Prova {}. {} passo(s), {} volta(s) de verificação.",
                    proof.label(),
                    run.steps.iter().filter(|s| s.status == StepStatus::Done).count(),
                    run.rounds
                );
                self.open_gate(
                    run,
                    GateKind::Aceitar,
                    "Aceitar a entrega".into(),
                    detail,
                    vec![],
                    out,
                    now,
                );
            }
            _ => {}
        }
    }

    fn submit_brief(
        &self,
        run: &mut Run,
        brief: Brief,
        out: &mut Outcome,
        now: DateTime<Utc>,
    ) -> Result<(), FlowError> {
        self.expect_agent_stage(run, Stage::Refinar)?;
        let profile = self.profile(run);
        if brief.questions.len() > profile.max_questions {
            return Err(FlowError::Invalid(Invalid(vec![format!(
                "o tamanho {} permite no máximo {} pergunta(s); pergunte só o que muda o resultado e assuma o resto em `suposicoes`",
                run.size.id(),
                profile.max_questions
            )])));
        }
        if let Some(size) = brief.size {
            if size > run.size {
                run.changes.push(Change {
                    at: now,
                    by: Actor::Agent,
                    kind: ChangeKind::Tamanho,
                    stage: run.stage,
                    text: format!("O refinamento subiu o tamanho para {}", size.label()),
                    before: Some(run.size.id().into()),
                    after: Some(size.id().into()),
                });
                out.event(
                    run,
                    "size.changed",
                    Actor::Agent,
                    format!("Tamanho {} → {}", run.size.id(), size.id()),
                );
                run.size = size;
            } else if size < run.size {
                out.event(
                    run,
                    "size.suggested",
                    Actor::Agent,
                    format!("O agente sugere o tamanho {}", size.id()),
                );
            }
        }
        run.questions.extend(brief.questions.iter().cloned());
        out.event(
            run,
            "brief.submitted",
            Actor::Agent,
            format!(
                "Pedido refinado: {} ({} pergunta(s))",
                brief.understanding,
                brief.questions.len()
            ),
        );
        run.brief = Some(brief);
        run.feedback = None;
        self.enter(run, Stage::Planejar, out, now);
        Ok(())
    }

    fn submit_plan(&self, run: &mut Run, plan: Plan, out: &mut Outcome, now: DateTime<Utc>) -> Result<(), FlowError> {
        self.expect_agent_stage(run, Stage::Planejar)?;
        let max = self.profile(run).max_steps;
        if plan.steps.len() > max {
            return Err(FlowError::Invalid(Invalid(vec![format!(
                "o tamanho {} permite no máximo {max} passo(s); junte passos ou peça ao dev para subir o tamanho com `passo ask`",
                run.size.id()
            )])));
        }
        // A test command is the first thing an agent writes that Passo runs itself: it goes through the lock. Only
        // the command matters here, so the project root is a placeholder and core stays free of the filesystem.
        let mut refused = vec![];
        for (i, step) in plan
            .steps
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.test_command.is_empty())
        {
            let tool = ToolUse {
                name: "rodar".into(),
                kind: ToolKind::Exec,
                paths: vec![],
                command: Some(step.test_command.clone()),
            };
            if let Verdict::Deny(reason) = lock::judge(&tool, Path::new("/"), Some(run)) {
                let reason = reason.split(". ").next().unwrap_or(&reason).trim_end_matches('.');
                refused.push(format!(
                    "`passos[{i}].rodar` não pode ser \"{}\": {reason}",
                    step.test_command
                ));
            }
        }
        if !refused.is_empty() {
            return Err(FlowError::Invalid(Invalid(refused)));
        }
        // On a re-plan the steps already done keep meeting criteria under the old numbering, so only a first plan
        // is held to "every criterion has a step".
        if !run.steps.iter().any(|s| s.status == StepStatus::Done) {
            let orphans: Vec<String> = (1..=plan.criteria.len())
                .map(|n| format!("C{n}"))
                .filter(|id| !plan.steps.iter().any(|s| s.criteria.contains(id)))
                .map(|id| {
                    format!(
                        "o critério {id} não tem passo que o atenda; acrescente um passo ou junte {id} a outro critério"
                    )
                })
                .collect();
            if !orphans.is_empty() {
                return Err(FlowError::Invalid(Invalid(orphans)));
            }
        }
        run.feedback = None;
        self.accept_plan(run, plan, Actor::Agent, out, now);
        Ok(())
    }

    fn accept_plan(&self, run: &mut Run, plan: Plan, by: Actor, out: &mut Outcome, now: DateTime<Utc>) {
        let print = fingerprint(&plan);
        run.plan_version += 1;
        if run.plan_version > 1 {
            run.changes.push(Change {
                at: now,
                by,
                kind: ChangeKind::Plano,
                stage: run.stage,
                text: format!("Plano v{}: {}", run.plan_version, plan.title),
                before: run.plan.as_ref().map(fingerprint),
                after: Some(print.clone()),
            });
        }
        let done: Vec<Step> = run.steps.drain(..).filter(|s| s.status == StepStatus::Done).collect();
        let offset = done.len();
        run.steps = done;
        run.steps.extend(plan.steps.iter().enumerate().map(|(i, s)| Step {
            id: format!("S{}", offset + i + 1),
            title: s.title.clone(),
            detail: s.detail.clone(),
            files: s.files.clone(),
            test: s.test.clone(),
            criteria: s.criteria.clone(),
            test_command: s.test_command.clone(),
            tier: None,
            status: StepStatus::Todo,
            attempts: 0,
            fix: false,
            summary: String::new(),
            commit: None,
        }));
        run.criteria = plan
            .criteria
            .iter()
            .enumerate()
            .map(|(i, text)| Criterion {
                id: format!("C{}", i + 1),
                text: text.clone(),
                ok: None,
                evidence: String::new(),
            })
            .collect();
        out.event(
            run,
            "plan.submitted",
            by,
            format!(
                "Plano v{}: {} ({} passo(s), {} critério(s))",
                run.plan_version,
                plan.title,
                plan.steps.len(),
                plan.criteria.len()
            ),
        );
        run.plan = Some(plan);
        let open: Vec<String> = run.open_questions().iter().map(|q| q.id.clone()).collect();
        // Commands Passo will run need the dev's yes, whatever the size says.
        let runs_commands = run
            .steps
            .iter()
            .any(|s| s.status != StepStatus::Done && !s.test_command.is_empty());
        if self.profile(run).approve_plan || !open.is_empty() || runs_commands {
            let mut detail = if open.is_empty() {
                "Aprove o plano ou recuse dizendo o que mudar.".to_string()
            } else {
                format!(
                    "{} pergunta(s) em aberto; sem resposta, vale a recomendada.",
                    open.len()
                )
            };
            if runs_commands {
                detail.push_str(" Aprovar também autoriza o Passo a rodar os comandos de teste listados nos passos.");
            }
            self.open_gate(run, GateKind::Aprovar, "Aprovar o plano".into(), detail, open, out, now);
        } else {
            run.approved_plan = Some(print);
            self.enter(run, Stage::Construir, out, now);
        }
    }

    fn next_step(&self, run: &mut Run, out: &mut Outcome, now: DateTime<Utc>) {
        if run.current_step().is_some() {
            return;
        }
        let Some(step) = run.steps.iter_mut().find(|s| s.status == StepStatus::Todo) else {
            self.enter(run, Stage::Verificar, out, now);
            return;
        };
        step.status = StepStatus::Doing;
        let text = format!("{}: {}", step.id, step.title);
        out.event(run, "step.started", Actor::Passo, text);
    }

    fn step_done(
        &self,
        run: &mut Run,
        facts: StepFacts,
        out: &mut Outcome,
        now: DateTime<Utc>,
    ) -> Result<(), FlowError> {
        self.expect_agent_stage(run, Stage::Construir)?;
        let profile = self.profile(run);
        let max_attempts = self.config.limits.attempts.max(1);
        let step = run
            .current_step()
            .cloned()
            .ok_or_else(|| FlowError::Wrong("não há passo em andamento".into()))?;
        run.checks = facts.checks.clone();

        let mut problems = vec![];
        for check in facts.checks.iter().filter(|c| !c.ok) {
            problems.push(format!("a checagem `{}` falhou:\n{}", check.command, check.output));
        }
        if facts.measured && facts.files.is_empty() && !facts.empty_ok {
            problems.push(
                "nenhum arquivo mudou neste passo. Se ele não precisa mudar nada, use `passo done --vazio \"motivo\"`"
                    .into(),
            );
        }
        if facts.measured && profile.require_tests && !step.test.is_empty() && !facts.tests_touched && !facts.empty_ok {
            problems.push(format!(
                "o passo pede um teste ({}), mas nenhum arquivo de teste mudou. Escreva o teste junto com o código",
                step.test
            ));
        }

        if !problems.is_empty() {
            let strong = crate::config::TIERS[1];
            let current = run.current_step_mut().expect("step in progress");
            current.attempts += 1;
            let attempts = current.attempts;
            // Spend the strong model only where the cheap one got stuck, before calling the dev.
            let escalated = profile.escalate_after > 0 && attempts >= profile.escalate_after && current.tier.is_none();
            if escalated {
                current.tier = Some(strong.to_string());
            }
            let mut feedback = problems.join("\n\n");
            if let Some(tier) = &current.tier {
                feedback.push_str(&format!("\n\nEsta tentativa usa o nível {tier}."));
            }
            run.feedback = Some(feedback);
            out.event(
                run,
                "step.failed",
                Actor::Passo,
                format!("{} não passou (tentativa {attempts}/{max_attempts})", step.id),
            );
            if escalated {
                out.event(
                    run,
                    "step.escalated",
                    Actor::Passo,
                    format!("{} sobe para o nível {strong}", step.id),
                );
            }
            if attempts >= max_attempts {
                let detail = format!(
                    "{} falhou {attempts} vezes. Último problema:\n{}",
                    step.id,
                    problems.join("\n")
                );
                self.open_gate(
                    run,
                    GateKind::Travado,
                    format!("{} travou", step.id),
                    detail,
                    vec![],
                    out,
                    now,
                );
            }
            return Ok(());
        }

        if let Some(current) = run.current_step_mut() {
            current.summary = facts.summary.clone();
        }
        if let Some(note) = facts.note.as_ref().filter(|n| !n.trim().is_empty()) {
            run.notes.push(format!("{}: {}", step.id, note.trim()));
        }
        if !facts.unplanned.is_empty() {
            let text = format!(
                "{} mudou arquivos fora do plano: {}",
                step.id,
                facts.unplanned.join(", ")
            );
            out.event(run, "step.unplanned", Actor::Passo, text.clone());
            run.notes.push(format!("{text}. Confira se deviam entrar."));
        }
        let over_files = profile.max_files > 0 && facts.files_total > profile.max_files;
        let over_lines = profile.max_lines > 0 && facts.lines_total > profile.max_lines;
        let sensitive = profile.block_sensitive && !facts.sensitive.is_empty();
        if facts.measured && !run.limit_accepted && (over_files || over_lines || sensitive) {
            let mut why = vec![];
            if over_files {
                why.push(format!("{} arquivos (limite {})", facts.files_total, profile.max_files));
            }
            if over_lines {
                why.push(format!("{} linhas (limite {})", facts.lines_total, profile.max_lines));
            }
            if sensitive {
                why.push(format!("área sensível: {}", facts.sensitive.join(", ")));
            }
            let detail = format!(
                "O tamanho {} passou do limite: {}. Siga assim (`passo ok`) ou suba o tamanho (`passo size m`).",
                run.size.id(),
                why.join("; ")
            );
            self.open_gate(
                run,
                GateKind::Limite,
                "Passou do tamanho".into(),
                detail,
                vec![],
                out,
                now,
            );
            return Ok(());
        }
        self.complete_step(run, out, now);
        Ok(())
    }

    fn complete_step(&self, run: &mut Run, out: &mut Outcome, now: DateTime<Utc>) {
        let Some(step) = run.current_step_mut() else { return };
        step.status = StepStatus::Done;
        let message = format!("{}: {}", step.id, step.title);
        let text = if step.summary.is_empty() {
            message.clone()
        } else {
            format!("{message} — {}", step.summary)
        };
        let id = step.id.clone();
        run.feedback = None;
        out.effects.push(Effect::Commit { step: id, message });
        out.event(run, "step.done", Actor::Agent, text);
        self.next_step(run, out, now);
    }

    fn final_checks(
        &self,
        run: &mut Run,
        results: Vec<CheckResult>,
        out: &mut Outcome,
        now: DateTime<Utc>,
    ) -> Result<(), FlowError> {
        if run.stage != Stage::Verificar || run.reviewing {
            return Err(FlowError::Wrong(
                "as checagens finais só rodam ao entrar em Verificar".into(),
            ));
        }
        let failed: Vec<&CheckResult> = results.iter().filter(|c| !c.ok).collect();
        let text = if results.is_empty() {
            "Nenhuma checagem final configurada".to_string()
        } else if failed.is_empty() {
            format!("{} checagem(ns) final(is) passaram", results.len())
        } else {
            format!("{} de {} checagem(ns) final(is) falharam", failed.len(), results.len())
        };
        out.event(run, "checks.final", Actor::Passo, text);
        if !failed.is_empty() {
            let detail = failed
                .iter()
                .map(|c| format!("`{}`:\n{}", c.command, c.output))
                .collect::<Vec<_>>()
                .join("\n\n");
            run.checks = results;
            self.back_to_build(
                run,
                vec![Fix::plain("Corrigir as checagens finais".into(), detail)],
                out,
                now,
            );
            return Ok(());
        }
        run.checks = results;
        if self.profile(run).verificar == Mode::Agente {
            run.reviewing = true;
            out.event(run, "review.requested", Actor::Passo, "Revisão dos critérios".into());
        } else {
            self.enter(run, Stage::Entregar, out, now);
        }
        Ok(())
    }

    fn submit_review(
        &self,
        run: &mut Run,
        review: Review,
        out: &mut Outcome,
        now: DateTime<Utc>,
    ) -> Result<(), FlowError> {
        if run.stage != Stage::Verificar || !run.reviewing {
            return Err(FlowError::Wrong(
                "a revisão só é entregue em Verificar, depois das checagens finais".into(),
            ));
        }
        for item in &review.criteria {
            if let Some(criterion) = run.criteria.iter_mut().find(|c| c.id == item.id) {
                criterion.ok = Some(item.ok);
                criterion.evidence = item.evidence.clone();
            }
        }
        // A criterion the review could not confirm is fixed with a test that proves it.
        let failing: Vec<Fix> = run
            .criteria
            .iter()
            .filter(|c| c.ok == Some(false))
            .map(|c| Fix {
                title: format!("Atender {}: {}", c.id, c.text),
                detail: c.evidence.clone(),
                test: format!("um teste que prove {}: {}", c.id, c.text),
                criteria: vec![c.id.clone()],
                // The last step that met the criterion knows how to run its test.
                test_command: run
                    .steps
                    .iter()
                    .rev()
                    .find(|s| s.criteria.contains(&c.id) && !s.test_command.is_empty())
                    .map(|s| s.test_command.clone())
                    .unwrap_or_default(),
            })
            .collect();
        let ok = run.criteria.len() - failing.len();
        out.event(
            run,
            "review.submitted",
            Actor::Agent,
            format!("Revisão: {ok}/{} critério(s) ok", run.criteria.len()),
        );
        for note in &review.notes {
            let note = format!("Revisão: {note}");
            if !run.notes.contains(&note) {
                run.notes.push(note);
            }
        }
        if failing.is_empty() {
            self.enter(run, Stage::Entregar, out, now);
        } else {
            self.back_to_build(run, failing, out, now);
        }
        Ok(())
    }

    /// Verify found problems: each becomes a fix step. Past the round limit, the dev decides.
    fn back_to_build(&self, run: &mut Run, fixes: Vec<Fix>, out: &mut Outcome, now: DateTime<Utc>) {
        run.rounds += 1;
        add_fix_steps(run, fixes);
        run.feedback = Some("A verificação achou problemas; cada um virou um passo de correção.".into());
        let max = self.profile(run).max_rounds;
        if run.rounds > max {
            // Back in Construir, but no step starts until the dev answers.
            self.announce(run, Stage::Construir, out);
            let detail = format!(
                "A verificação já voltou {} vez(es) (limite {max}). Os problemas viraram passos de correção. `passo ok` tenta de novo; `passo ok \"dica\"` tenta com uma dica sua.",
                run.rounds
            );
            self.open_gate(
                run,
                GateKind::Travado,
                "Verificação travou".into(),
                detail,
                vec![],
                out,
                now,
            );
        } else {
            self.enter(run, Stage::Construir, out, now);
        }
    }

    fn ask(
        &self,
        run: &mut Run,
        mut question: Question,
        out: &mut Outcome,
        now: DateTime<Utc>,
    ) -> Result<(), FlowError> {
        question.id = format!("Q{}", run.questions.len() + 1);
        let id = question.id.clone();
        out.event(run, "question.asked", Actor::Agent, format!("{id}: {}", question.ask));
        let title = question.ask.clone();
        run.questions.push(question);
        self.open_gate(run, GateKind::Pergunta, title, String::new(), vec![id], out, now);
        Ok(())
    }

    // -- dev decisions ---------------------------------------------------------------------------------------

    fn approve(
        &self,
        run: &mut Run,
        decision: Decision,
        out: &mut Outcome,
        now: DateTime<Utc>,
    ) -> Result<(), FlowError> {
        let gate = run.gate.clone().ok_or(FlowError::NoGate)?;
        match gate.kind {
            GateKind::Aprovar => {
                let deviated = self.answer_questions(run, &gate, &decision, out, now)?;
                run.gate = None;
                if deviated && self.profile(run).planejar == Mode::Agente {
                    run.feedback = Some(format!(
                        "O dev respondeu diferente da recomendação. Ajuste o plano a estas respostas:\n{}",
                        answered_lines(run, &gate.questions)
                    ));
                    out.event(
                        run,
                        "plan.revise",
                        Actor::Dev,
                        "Respostas mudam o plano: volta ao planejamento".into(),
                    );
                    self.enter(run, Stage::Planejar, out, now);
                } else {
                    run.approved_plan = run.plan.as_ref().map(fingerprint);
                    out.event(
                        run,
                        "plan.approved",
                        Actor::Dev,
                        format!("Plano v{} aprovado", run.plan_version),
                    );
                    self.enter(run, Stage::Construir, out, now);
                }
            }
            GateKind::Pergunta => {
                self.answer_questions(run, &gate, &decision, out, now)?;
                run.gate = None;
                let lines = answered_lines(run, &gate.questions);
                run.notes.push(lines.clone());
                run.feedback = Some(format!("Resposta do dev:\n{lines}"));
            }
            GateKind::Travado => {
                let hint = decision.note();
                if let Some(hint) = &hint {
                    self.record(run, Actor::Dev, ChangeKind::Dica, hint.clone(), None, None, now);
                }
                if let Some(step) = run.current_step_mut() {
                    step.attempts = 0;
                }
                run.gate = None;
                run.feedback = Some(match hint {
                    Some(hint) => format!("Dica do dev: {hint}"),
                    None => {
                        "O dev pediu mais uma tentativa. Releia o problema com calma antes de mudar o código.".into()
                    }
                });
                out.event(run, "gate.retry", Actor::Dev, "Mais uma tentativa".into());
                self.next_step(run, out, now);
            }
            GateKind::Limite => {
                run.limit_accepted = true;
                run.gate = None;
                out.event(
                    run,
                    "limit.accepted",
                    Actor::Dev,
                    "Dev aceitou passar do limite do tamanho".into(),
                );
                self.complete_step(run, out, now);
            }
            GateKind::Aceitar => {
                run.gate = None;
                run.status = Status::Done;
                out.effects.push(Effect::Deliver);
                out.event(run, "delivery.accepted", Actor::Dev, "Entrega aceita".into());
            }
        }
        Ok(())
    }

    fn reject(&self, run: &mut Run, text: String, out: &mut Outcome, now: DateTime<Utc>) -> Result<(), FlowError> {
        let gate = run.gate.clone().ok_or(FlowError::NoGate)?;
        if text.trim().is_empty() {
            return Err(FlowError::Wrong("diga o que mudar: passo no \"o que mudar\"".into()));
        }
        match gate.kind {
            GateKind::Aprovar => {
                self.record(run, Actor::Dev, ChangeKind::Recusa, text.clone(), None, None, now);
                run.gate = None;
                run.feedback = Some(format!("O dev recusou o plano: {text}"));
                out.event(run, "plan.rejected", Actor::Dev, text);
                self.enter(run, Stage::Refinar, out, now);
            }
            GateKind::Aceitar => {
                self.record(run, Actor::Dev, ChangeKind::Recusa, text.clone(), None, None, now);
                run.gate = None;
                add_fix_steps(
                    run,
                    vec![Fix::plain(format!("Ajustar a entrega: {text}"), String::new())],
                );
                run.feedback = Some(format!("O dev recusou a entrega: {text}"));
                out.event(run, "delivery.rejected", Actor::Dev, text);
                self.enter(run, Stage::Construir, out, now);
            }
            GateKind::Pergunta => {
                let decision = Decision {
                    answers: vec![Answer {
                        index: None,
                        value: text,
                    }],
                };
                return self.approve(run, decision, out, now);
            }
            GateKind::Travado | GateKind::Limite => {
                return Err(FlowError::Wrong(
                    "aqui não há o que recusar: use passo ok, passo size, passo change ou passo stop".into(),
                ));
            }
        }
        Ok(())
    }

    fn change(&self, run: &mut Run, text: String, out: &mut Outcome, now: DateTime<Utc>) -> Result<(), FlowError> {
        if text.trim().is_empty() {
            return Err(FlowError::Wrong(
                "diga a mudança: passo change \"o que muda no pedido\"".into(),
            ));
        }
        self.record(run, Actor::Dev, ChangeKind::Mudanca, text.clone(), None, None, now);
        run.gate = None;
        run.limit_accepted = false;
        run.feedback = Some(format!(
            "O dev mudou o pedido: {text}\nPassos já feitos ficam; refine e planeje só o que falta."
        ));
        out.event(run, "request.changed", Actor::Dev, text);
        if let Some(step) = run.current_step_mut() {
            step.status = StepStatus::Todo;
        }
        self.enter(run, Stage::Refinar, out, now);
        Ok(())
    }

    fn resize(&self, run: &mut Run, size: Size, out: &mut Outcome, now: DateTime<Utc>) -> Result<(), FlowError> {
        if size == run.size {
            return Err(FlowError::Wrong(format!("o run já é {}", size.label())));
        }
        let before = run.size;
        self.record(
            run,
            Actor::Dev,
            ChangeKind::Tamanho,
            format!("Tamanho {} → {}", before.id(), size.id()),
            Some(before.id().into()),
            Some(size.id().into()),
            now,
        );
        run.size = size;
        out.event(
            run,
            "size.changed",
            Actor::Dev,
            format!("Tamanho {} → {}", before.id(), size.id()),
        );
        if run.gate.as_ref().map(|g| g.kind) == Some(GateKind::Limite) {
            run.gate = None;
            run.limit_accepted = true;
            self.complete_step(run, out, now);
        }
        Ok(())
    }

    // -- helpers ---------------------------------------------------------------------------------------------

    /// Applies the dev's answers to the gate's questions; unanswered ones take the recommendation.
    /// Returns whether any answer differs from the recommendation.
    fn answer_questions(
        &self,
        run: &mut Run,
        gate: &Gate,
        decision: &Decision,
        out: &mut Outcome,
        now: DateTime<Utc>,
    ) -> Result<bool, FlowError> {
        let ids = &gate.questions;
        let mut chosen: Vec<Option<String>> = vec![None; ids.len()];
        for answer in &decision.answers {
            let slot = match answer.index {
                Some(n) if n >= 1 && n <= ids.len() => n - 1,
                Some(n) => {
                    return Err(FlowError::Wrong(format!(
                        "não há a pergunta {n}; há {} em aberto",
                        ids.len()
                    )));
                }
                None if ids.len() == 1 => 0,
                None if ids.is_empty() => continue,
                None => {
                    return Err(FlowError::Wrong(
                        "há mais de uma pergunta: numere as respostas, como `1b` ou `2=suas palavras`".into(),
                    ));
                }
            };
            chosen[slot] = Some(answer.value.clone());
        }
        let mut deviated = false;
        for (id, value) in ids.iter().zip(chosen) {
            let Some(question) = run.questions.iter_mut().find(|q| &q.id == id) else {
                continue;
            };
            let value = value.map(|v| {
                let letter = v.trim().to_uppercase();
                if question.options.iter().any(|o| o.id == letter) {
                    letter
                } else {
                    v.trim().to_string()
                }
            });
            let answer = value.unwrap_or_else(|| question.recommended.clone());
            if answer != question.recommended {
                deviated = true;
            }
            question.answer = Some(answer);
            let text = question.answer_text().unwrap_or_default();
            let recommended = question.recommended_text().to_string();
            let line = format!("{}: {} → {}", question.id, question.ask, text);
            self.record(
                run,
                Actor::Dev,
                ChangeKind::Resposta,
                line.clone(),
                Some(recommended),
                Some(text),
                now,
            );
            out.event(run, "question.answered", Actor::Dev, line);
        }
        Ok(deviated)
    }

    #[allow(clippy::too_many_arguments)]
    fn open_gate(
        &self,
        run: &mut Run,
        kind: GateKind,
        title: String,
        detail: String,
        questions: Vec<String>,
        out: &mut Outcome,
        now: DateTime<Utc>,
    ) {
        out.event(
            run,
            "gate.opened",
            Actor::Passo,
            format!("Vez do dev: {}", kind.label()),
        );
        run.gate = Some(Gate {
            kind,
            title,
            detail,
            questions,
            at: now,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        &self,
        run: &mut Run,
        by: Actor,
        kind: ChangeKind,
        text: String,
        before: Option<String>,
        after: Option<String>,
        now: DateTime<Utc>,
    ) {
        run.changes.push(Change {
            at: now,
            by,
            kind,
            stage: run.stage,
            text,
            before,
            after,
        });
    }

    fn expect_agent_stage(&self, run: &Run, stage: Stage) -> Result<(), FlowError> {
        if run.stage != stage {
            return Err(FlowError::Wrong(format!(
                "o run está em {}, não em {}; rode `passo next`",
                run.stage.label(),
                stage.label()
            )));
        }
        if self.profile(run).mode(stage) == Mode::Auto {
            return Err(FlowError::Wrong(format!(
                "{} é automática no tamanho {}",
                stage.label(),
                run.size.id()
            )));
        }
        Ok(())
    }

    fn proof(&self, run: &Run) -> Proof {
        // `run.checks` holds the final checks here, wherever they were configured (passo.yaml or the stage file).
        let final_ok = !run.checks.is_empty() && run.checks.iter().all(|c| c.ok);
        let reviewed = !run.criteria.is_empty() && run.criteria.iter().all(|c| c.ok == Some(true));
        match (final_ok, reviewed) {
            (true, true) => Proof::Forte,
            (true, false) | (false, true) => Proof::Media,
            _ if !self.config.checks.step.is_empty() => Proof::Fraca,
            _ => Proof::Nenhuma,
        }
    }
}

impl Outcome {
    fn event(&mut self, run: &Run, kind: &'static str, actor: Actor, text: String) {
        self.events.push(EventDraft {
            kind,
            actor,
            stage: run.stage,
            text,
            data: json!({}),
        });
    }
}

/// A correction step that verify or a rejected delivery asks for.
struct Fix {
    title: String,
    detail: String,
    test: String,
    criteria: Vec<String>,
    test_command: String,
}

impl Fix {
    fn plain(title: String, detail: String) -> Fix {
        Fix {
            title,
            detail,
            test: String::new(),
            criteria: vec![],
            test_command: String::new(),
        }
    }
}

fn add_fix_steps(run: &mut Run, fixes: Vec<Fix>) {
    if let Some(step) = run.current_step_mut() {
        step.status = StepStatus::Todo;
    }
    for fix in fixes {
        let id = format!("S{}", run.steps.len() + 1);
        run.steps.push(Step {
            id,
            title: fix.title,
            detail: fix.detail,
            files: vec![],
            test: fix.test,
            criteria: fix.criteria,
            test_command: fix.test_command,
            tier: None,
            status: StepStatus::Todo,
            attempts: 0,
            fix: true,
            summary: String::new(),
            commit: None,
        });
    }
}

fn answered_lines(run: &Run, ids: &[String]) -> String {
    run.questions
        .iter()
        .filter(|q| ids.contains(&q.id))
        .map(|q| format!("- {}: {} → {}", q.id, q.ask, q.answer_text().unwrap_or_default()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn change_lines(run: &Run) -> Vec<String> {
    run.changes
        .iter()
        .filter(|c| c.kind == ChangeKind::Mudanca)
        .map(|c| c.text.clone())
        .collect()
}

/// The brief for sizes that skip refinement: the request as it is.
fn auto_brief(run: &Run) -> Brief {
    let mut scope = vec![run.request.clone()];
    scope.extend(change_lines(run));
    Brief {
        understanding: run.request.clone(),
        size: None,
        scope,
        out_of_scope: vec!["qualquer coisa além do pedido".into()],
        assumptions: vec![],
        files: vec![],
        questions: vec![],
    }
}

/// The plan for sizes that skip planning: one step that does the request (or the latest change).
fn auto_plan(run: &Run) -> Plan {
    let changes = change_lines(run);
    let has_done = run.steps.iter().any(|s| s.status == StepStatus::Done);
    let title = match (has_done, changes.last()) {
        (true, Some(change)) => format!("Aplicar a mudança: {change}"),
        _ => run.request.clone(),
    };
    let mut criteria = vec![run.request.clone()];
    criteria.extend(changes);
    let count = criteria.len();
    Plan {
        title: run.request.chars().take(80).collect(),
        criteria,
        steps: vec![crate::spec::PlanStep {
            title,
            detail: String::new(),
            files: vec![],
            test: String::new(),
            // The single step of an automatic plan meets every criterion, and never runs a command of its own.
            criteria: (1..=count).map(|n| format!("C{n}")).collect(),
            test_command: String::new(),
        }],
        notes: String::new(),
    }
}

/// A stable fingerprint of a plan (FNV-1a over its JSON). The audit chain uses SHA-256; this only names versions.
pub fn fingerprint(plan: &Plan) -> String {
    let bytes = serde_json::to_vec(plan).unwrap_or_default();
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}
