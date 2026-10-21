//! What the agent and the dev read. The task text is where Passo carries the structure a small model would
//! otherwise have to hold: one role, one task, only the context it needs, the last failure, and how to hand in.

use std::fmt::Write as _;

use passo_core::spec::{BRIEF_TEMPLATE, PLAN_TEMPLATE, REVIEW_TEMPLATE};
use passo_core::{Config, GateKind, Mode, Run, Stage, Status, StepStatus, Turn};
use passo_store::Catalog;

pub struct View<'a> {
    pub run: &'a Run,
    pub config: &'a Config,
    pub catalog: &'a Catalog,
    /// How to hand a stage to a subagent on the installed platform(s).
    pub delegation: &'a dyn Fn(&str) -> String,
    pub bin: &'a str,
    /// The repository map, shown while refining and planning.
    pub map: Option<&'a str>,
}

/// `Refinar ✓ · Planejar ✓ · Construir ▸ · Verificar · Entregar`, with `(auto)` where the size skips the agent.
pub fn rail(run: &Run, config: &Config) -> String {
    let profile = config.profile(run.size);
    Stage::ALL
        .iter()
        .map(|stage| {
            let auto = profile.mode(*stage) == Mode::Auto && *stage != Stage::Entregar;
            let label = if auto {
                format!("{} (auto)", stage.label())
            } else {
                stage.label().to_string()
            };
            let mark = if run.status == Status::Done || *stage < run.stage {
                " ✓"
            } else if *stage == run.stage && run.status == Status::Active {
                " ▸"
            } else {
                ""
            };
            format!("{label}{mark}")
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The answer to `passo next`: the agent's task, or what to show the dev.
pub fn next(view: &View) -> String {
    let run = view.run;
    let mut out = format!("passo · {}\n{}\n\n", run.headline(), rail(run, view.config));
    match run.turn() {
        Turn::None if run.status == Status::Done => {
            let _ = writeln!(out, "Run concluído. Nada mais a fazer: avise o dev e pare.");
        }
        Turn::None => {
            let _ = writeln!(out, "Run interrompido pelo dev. Nada a fazer: pare.");
        }
        Turn::Dev => {
            out.push_str(
                "# Vez do dev\nMostre ao dev o texto abaixo, do jeito que está, e pare. Não responda por ele: ele responde no chat (\"ok\", \"ok 1b\", \"não: …\") ou no terminal. Depois que ele responder, rode `passo next`.\n\n",
            );
            out.push_str(&gate(view));
        }
        Turn::Agent => out.push_str(&task(view)),
    }
    out
}

fn task(view: &View) -> String {
    let run = view.run;
    let stage = run.stage;
    let mut out = String::new();
    let def = view.catalog.stage(stage);
    if let Some(agent) = view.catalog.agent_for(stage) {
        // A step the cheap model got stuck on asks for the same agent at a stronger tier.
        let escalated = run
            .current_step()
            .filter(|_| stage == Stage::Construir)
            .and_then(|s| s.tier.clone())
            .filter(|tier| *tier != agent.model);
        let (tier, delegate) = match &escalated {
            Some(tier) => (tier.as_str(), format!("{}-{tier}", agent.name)),
            None => (agent.model.as_str(), agent.name.clone()),
        };
        let _ = writeln!(
            out,
            "# Agente: {} (nível {tier})\n{}\n",
            agent.name,
            (view.delegation)(&delegate)
        );
        let _ = writeln!(out, "## Papel\n{}\n", agent.body.trim());
    }
    let instructions = def.map(|d| d.instructions(run.size)).unwrap_or_default();
    let _ = writeln!(out, "## Tarefa: {}\n{}\n", stage.label(), instructions.trim());
    out.push_str("## Contexto\n");
    out.push_str(&context(view));
    if let Some(feedback) = &run.feedback {
        let _ = writeln!(out, "\n## Atenção\n{}", feedback.trim());
    }
    out.push_str("\n## Como entregar\n");
    out.push_str(&hand_in(view));
    out
}

fn context(view: &View) -> String {
    let run = view.run;
    let profile = view.config.profile(run.size);
    let mut out = String::new();
    let _ = writeln!(out, "Pedido do dev: {}", run.request);
    let changes: Vec<&String> = run
        .changes
        .iter()
        .filter(|c| matches!(c.kind, passo_core::ChangeKind::Mudanca | passo_core::ChangeKind::Recusa))
        .map(|c| &c.text)
        .collect();
    if !changes.is_empty() {
        let _ = writeln!(out, "Mudanças do dev no pedido (valem mais que o pedido original):");
        for c in changes {
            let _ = writeln!(out, "- {c}");
        }
    }
    let answered: Vec<String> = run
        .questions
        .iter()
        .filter_map(|q| q.answer_text().map(|a| format!("- {}: {} → {a}", q.id, q.ask)))
        .collect();
    if !answered.is_empty() {
        let _ = writeln!(out, "Decisões do dev:\n{}", answered.join("\n"));
    }
    if let Some(context) = &view.catalog.context {
        let _ = writeln!(out, "\nConvenções do projeto:\n{}", context.trim());
    }

    match run.stage {
        Stage::Refinar => repo_map(view, &mut out),
        Stage::Planejar => {
            repo_map(view, &mut out);
            if let Some(brief) = &run.brief {
                let _ = writeln!(out, "\nPedido refinado: {}", brief.understanding);
                list(&mut out, "Escopo", &brief.scope);
                list(&mut out, "Fora do escopo", &brief.out_of_scope);
                list(&mut out, "Suposições", &brief.assumptions);
                list(&mut out, "Arquivos que o pedido toca", &brief.files);
            }
            let done: Vec<String> = run
                .steps
                .iter()
                .filter(|s| s.status == StepStatus::Done)
                .map(|s| format!("{} {}", s.id, s.title))
                .collect();
            if !done.is_empty() {
                list(&mut out, "Passos já feitos (ficam; planeje só o que falta)", &done);
            }
            checklists(view, &mut out);
        }
        Stage::Construir => {
            // Only what this step needs: the plan's title, the step before, this one, the next one.
            if let Some(plan) = &run.plan {
                let _ = writeln!(out, "\nPlano: {}", plan.title);
            }
            let current = run.steps.iter().position(|s| s.status == StepStatus::Doing);
            if let Some(previous) = run.steps.iter().rev().find(|s| s.status == StepStatus::Done) {
                let summary = if previous.summary.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", previous.summary)
                };
                let _ = writeln!(out, "Passo anterior: ✓ {} {}{summary}", previous.id, previous.title);
            }
            if let Some(step) = run.current_step() {
                if step.criteria.is_empty() && !run.criteria.is_empty() {
                    out.push_str("Critérios de aceite:\n");
                    for c in &run.criteria {
                        let _ = writeln!(out, "  {} {}", c.id, c.text);
                    }
                }
                let _ = writeln!(out, "\nPasso da vez: {} · {}", step.id, step.title);
                if !step.detail.is_empty() {
                    let _ = writeln!(out, "Detalhe: {}", step.detail);
                }
                if !step.files.is_empty() {
                    let _ = writeln!(out, "Arquivos: {}", step.files.join(", "));
                }
                let meets: Vec<String> = run
                    .criteria
                    .iter()
                    .filter(|c| step.criteria.contains(&c.id))
                    .map(|c| format!("{} {}", c.id, c.text))
                    .collect();
                if !meets.is_empty() {
                    let _ = writeln!(out, "Atende: {}", meets.join(" · "));
                }
                if !step.test_command.is_empty() {
                    let test = if step.test.is_empty() {
                        "o deste passo"
                    } else {
                        step.test.as_str()
                    };
                    let _ = writeln!(
                        out,
                        "Teste (escreva primeiro): {test} · o Passo roda: {}",
                        step.test_command
                    );
                } else if step.test.is_empty() {
                    let _ = writeln!(out, "Teste: este passo não pede teste novo.");
                } else {
                    let _ = writeln!(out, "Teste (escreva primeiro): {}", step.test);
                }
                if step.attempts > 0 {
                    let _ = writeln!(
                        out,
                        "Tentativa {} de {}",
                        step.attempts + 1,
                        view.config.limits.attempts
                    );
                }
            }
            let mut checks = view.config.checks.step.clone();
            if let Some(def) = view.catalog.stage(Stage::Construir) {
                checks.extend(def.checks.iter().cloned());
            }
            if !checks.is_empty() {
                let _ = writeln!(out, "\nO Passo roda depois do seu `done`: {}", checks.join(" · "));
            }
            if let Some(next) = current.and_then(|i| run.steps[i + 1..].iter().find(|s| s.status == StepStatus::Todo)) {
                let _ = writeln!(out, "Depois deste: {} {}", next.id, next.title);
            }
            if profile.require_tests {
                let _ = writeln!(
                    out,
                    "Um passo com teste precisa mudar um arquivo de teste, ou volta para você."
                );
            }
            let picked = builder_notes(run);
            if !picked.is_empty() {
                list(&mut out, "Notas do run", &picked);
            }
        }
        Stage::Verificar => {
            plan_overview(run, &mut out);
            let map = criteria_map(run);
            if !map.is_empty() {
                let _ = writeln!(out, "Quem atende cada critério (comece pelos commits deles):");
                for line in map {
                    let _ = writeln!(out, "  {line}");
                }
            }
            if let Some(base) = run.workspace.as_ref().and_then(|w| w.base.as_ref()) {
                let _ = writeln!(
                    out,
                    "\nDiff do run: `git diff {base}` (e `git diff {base} --stat` para o resumo)"
                );
            }
            if !run.checks.is_empty() {
                let _ = writeln!(out, "Checagens finais que o Passo rodou e passaram:");
                for check in &run.checks {
                    let _ = writeln!(out, "- {} ✓", check.command);
                }
            }
            if let Some(plan) = &run.plan
                && !plan.notes.is_empty()
            {
                let _ = writeln!(out, "Notas de design do plano: {}", plan.notes);
            }
            checklists(view, &mut out);
            notes(run, &mut out);
        }
        Stage::Entregar => {}
    }
    out
}

fn hand_in(view: &View) -> String {
    let run = view.run;
    let bin = view.bin;
    let profile = view.config.profile(run.size);
    match run.stage {
        Stage::Refinar => format!(
            "Escreva este JSON num arquivo temporário e rode `{bin} done <arquivo>`. No tamanho {}, no máximo {} pergunta(s); sem perguntas, deixe `perguntas` vazio. Só mude `tamanho` se o pedido for claramente maior.\n\n```json\n{}\n```\n",
            run.size.id(),
            profile.max_questions,
            BRIEF_TEMPLATE.replace("\"tamanho\": \"m\"", &format!("\"tamanho\": \"{}\"", run.size.id()))
        ),
        Stage::Planejar => format!(
            "Escreva este JSON num arquivo temporário e rode `{bin} done <arquivo>`. No tamanho {}, no máximo {} passo(s).\n\n```json\n{PLAN_TEMPLATE}\n```\n",
            run.size.id(),
            profile.max_steps
        ),
        Stage::Construir => format!(
            "`{bin} done \"o que mudou, numa linha\"`\n- Nada a mudar neste passo: `{bin} done --vazio \"por quê\"`\n- Viu algo fora do passo: acrescente `--nota \"…\"`\n- Dúvida que muda o resultado: `{bin} ask \"pergunta\" \"opção recomendada — porquê\" \"outra — porquê\"`\n"
        ),
        Stage::Verificar => {
            let ids: Vec<&str> = run.criteria.iter().map(|c| c.id.as_str()).collect();
            format!(
                "Um item por critério ({}). Escreva o JSON num arquivo temporário e rode `{bin} done <arquivo>`.\n\n```json\n{REVIEW_TEMPLATE}\n```\n",
                ids.join(", ")
            )
        }
        Stage::Entregar => String::new(),
    }
}

/// What the dev reads at a gate (also shown to the agent, to relay).
pub fn gate(view: &View) -> String {
    let run = view.run;
    let Some(gate) = &run.gate else { return String::new() };
    let mut out = String::new();
    match gate.kind {
        GateKind::Aprovar => {
            let _ = writeln!(
                out,
                "## Aprovar o plano · v{} · tamanho {}",
                run.plan_version,
                run.size.label()
            );
            if let Some(brief) = &run.brief {
                let _ = writeln!(out, "{}", brief.understanding);
                list(&mut out, "Escopo", &brief.scope);
                list(&mut out, "Fora do escopo", &brief.out_of_scope);
                list(&mut out, "Suposições", &brief.assumptions);
            }
            plan_overview(run, &mut out);
            if let Some(plan) = &run.plan
                && !plan.notes.is_empty()
            {
                let _ = writeln!(out, "Notas: {}", plan.notes);
            }
            questions(run, &gate.questions, &mut out);
            if run
                .steps
                .iter()
                .any(|s| s.status != StepStatus::Done && !s.test_command.is_empty())
            {
                out.push_str("\nAprovar também autoriza o Passo a rodar os comandos de teste listados nos passos.\n");
            }
            out.push_str(
                "\nResponda:\n  ok                 aprova (perguntas sem resposta ficam com a recomendada ★)\n  ok 1b              escolhe a opção B na pergunta 1\n  ok 1=suas palavras responde com as suas palavras\n  não: o que mudar   recusa e o pedido volta para o refinamento\n",
            );
        }
        GateKind::Aceitar => {
            let _ = writeln!(
                out,
                "## Aceitar a entrega · prova {}",
                run.proof.map(|p| p.label()).unwrap_or("?")
            );
            for step in run.steps.iter().filter(|s| s.status == StepStatus::Done) {
                let commit = step
                    .commit
                    .as_deref()
                    .map(|c| format!(" [{}]", &c[..c.len().min(7)]))
                    .unwrap_or_default();
                let summary = if step.summary.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", step.summary)
                };
                let _ = writeln!(out, "  ✓ {} {}{summary}{commit}", step.id, step.title);
            }
            if !run.criteria.is_empty() {
                out.push_str("Critérios:\n");
                for c in &run.criteria {
                    let mark = match c.ok {
                        Some(true) => "✓",
                        Some(false) => "✗",
                        None => "·",
                    };
                    let evidence = if c.evidence.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", c.evidence)
                    };
                    let _ = writeln!(out, "  {mark} {} {}{evidence}", c.id, c.text);
                }
            }
            if !run.checks.is_empty() {
                let checks: Vec<String> = run
                    .checks
                    .iter()
                    .map(|c| format!("{} {}", c.command, if c.ok { "✓" } else { "✗" }))
                    .collect();
                let _ = writeln!(out, "Checagens: {}", checks.join(" · "));
            }
            if let Some(branch) = run.workspace.as_ref().and_then(|w| w.branch.as_ref()) {
                let what = match view.config.git.delivery {
                    passo_core::config::Delivery::Merge => "aceitar junta o branch",
                    passo_core::config::Delivery::Keep => "aceitar deixa o branch pronto para um pull request",
                };
                let _ = writeln!(out, "Branch: {branch} ({what})");
            }
            if !run.notes.is_empty() {
                list(&mut out, "Notas", &run.notes);
            }
            out.push_str(
                "\nResponda:\n  aceito             aceita a entrega (no chat, um ok solto não basta)\n  não: o que mudar   vira um passo de ajuste\n",
            );
        }
        GateKind::Pergunta => {
            let _ = writeln!(out, "## Pergunta do agente");
            questions(run, &gate.questions, &mut out);
            out.push_str("\nResponda:\n  ok                 fica com a recomendada ★\n  ok b               escolhe a opção B\n  ok suas palavras   responde com as suas palavras (no terminal: passo ok \"…\")\n");
        }
        GateKind::Travado => {
            let _ = writeln!(out, "## {}\n{}", gate.title, gate.detail);
            out.push_str("\nResponda:\n  ok                 mais uma tentativa\n  ok \"dica\"          tenta de novo com uma dica sua (no terminal)\n  passo change \"…\"   muda o pedido · passo stop  encerra\n");
        }
        GateKind::Limite => {
            let _ = writeln!(out, "## {}\n{}", gate.title, gate.detail);
            out.push_str(
                "\nResponda:\n  ok                 segue assim\n  passo size m       sobe o tamanho e segue\n",
            );
        }
    }
    out
}

fn questions(run: &Run, ids: &[String], out: &mut String) {
    let open: Vec<&passo_core::spec::Question> = run.questions.iter().filter(|q| ids.contains(&q.id)).collect();
    if open.is_empty() {
        return;
    }
    out.push_str("Perguntas:\n");
    for (i, q) in open.iter().enumerate() {
        let _ = writeln!(out, "  {}. {}", i + 1, q.ask);
        if !q.why.is_empty() {
            let _ = writeln!(out, "     ({})", q.why);
        }
        for option in &q.options {
            let star = if option.id == q.recommended { " ★" } else { "" };
            let why = if option.why.is_empty() {
                String::new()
            } else {
                format!(" — {}", option.why)
            };
            let _ = writeln!(out, "     {}{star} {}{why}", option.id, option.text);
        }
    }
}

fn plan_overview(run: &Run, out: &mut String) {
    if run.criteria.is_empty() && run.steps.is_empty() {
        return;
    }
    if let Some(plan) = &run.plan {
        let _ = writeln!(out, "\nPlano: {}", plan.title);
    }
    if !run.criteria.is_empty() {
        out.push_str("Critérios de aceite:\n");
        for c in &run.criteria {
            let _ = writeln!(out, "  {} {}", c.id, c.text);
        }
    }
    if !run.steps.is_empty() {
        out.push_str("Passos:\n");
        for s in &run.steps {
            let mark = match s.status {
                StepStatus::Done => "✓",
                StepStatus::Doing => "▸",
                StepStatus::Todo => "·",
            };
            let mut extra = vec![];
            if !s.files.is_empty() {
                extra.push(s.files.join(", "));
            }
            if !s.test.is_empty() {
                extra.push(format!("teste: {}", s.test));
            }
            if !s.criteria.is_empty() {
                extra.push(format!("atende {}", s.criteria.join(", ")));
            }
            if !s.test_command.is_empty() {
                extra.push(format!("o Passo roda: {}", s.test_command));
            }
            let extra = if extra.is_empty() {
                String::new()
            } else {
                format!("  ({})", extra.join(" · "))
            };
            let _ = writeln!(out, "  {mark} {} {}{extra}", s.id, s.title);
        }
    }
}

/// `C1 ← S1 [a1b2c3d], S3 [e4f5a6b]`: the steps that met each criterion, with their commits.
fn criteria_map(run: &Run) -> Vec<String> {
    run.criteria
        .iter()
        .map(|c| {
            let steps: Vec<String> = run
                .steps
                .iter()
                .filter(|s| s.criteria.contains(&c.id))
                .map(|s| match &s.commit {
                    Some(hash) => format!("{} [{}]", s.id, &hash[..hash.len().min(7)]),
                    None => s.id.clone(),
                })
                .collect();
            let who = if steps.is_empty() {
                "nenhum passo".to_string()
            } else {
                steps.join(", ")
            };
            format!("{} ← {who}", c.id)
        })
        .collect()
}

/// The notes a builder needs: what earlier steps said about this step's files, what the review said, and the five
/// most recent. Never more than ten.
fn builder_notes(run: &Run) -> Vec<String> {
    const MAX: usize = 10;
    let Some(step) = run.current_step() else { return vec![] };
    let done: Vec<&str> = run
        .steps
        .iter()
        .filter(|s| s.status == StepStatus::Done)
        .map(|s| s.id.as_str())
        .collect();
    let from_done = |note: &str| {
        done.iter()
            .any(|id| note.starts_with(&format!("{id}:")) || note.starts_with(&format!("{id} ")))
    };
    let mut picked: Vec<String> = vec![];
    let add = |note: &String, picked: &mut Vec<String>| {
        if !picked.contains(note) {
            picked.push(note.clone());
        }
    };
    for note in &run.notes {
        let about_this_step = from_done(note) && step.files.iter().any(|f| note.contains(f.as_str()));
        if about_this_step || note.starts_with("Revisão:") {
            add(note, &mut picked);
        }
    }
    for note in run.notes.iter().rev().take(5) {
        add(note, &mut picked);
    }
    picked.truncate(MAX);
    picked
}

fn repo_map(view: &View, out: &mut String) {
    if let Some(map) = view.map.filter(|m| !m.trim().is_empty()) {
        let _ = writeln!(out, "\nMapa do repositório:\n{}", map.trim_end());
    }
}

fn notes(run: &Run, out: &mut String) {
    if !run.notes.is_empty() {
        list(out, "Notas do run", &run.notes);
    }
}

fn checklists(view: &View, out: &mut String) {
    let mut text = view.run.request.clone();
    if let Some(brief) = &view.run.brief {
        text.push(' ');
        text.push_str(&brief.understanding);
    }
    for checklist in view.catalog.checklists_for(&text) {
        let _ = writeln!(out, "\nChecklist · {}:\n{}", checklist.title, checklist.body.trim());
    }
}

fn list(out: &mut String, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    let _ = writeln!(out, "{title}:");
    for item in items {
        let _ = writeln!(out, "- {item}");
    }
}
