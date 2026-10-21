//! `passo`: the command line. A thin shell over `passo-app`.

mod paint;

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use passo_adapters::HookEvent;
use passo_app::metrics::{count, duration};
use passo_app::{App, DevAction, DoneInput, Rejected, Source, setup};
use passo_core::Size;

#[derive(Parser)]
#[command(
    name = "passo",
    version,
    about = "Passo a passo, com você aprovando: conduz agentes de IA do pedido à entrega.",
    after_help = "Etapas: Refinar → Planejar → Construir → Verificar → Entregar\nTamanhos: pp (ajuste) · p (pequeno) · m (feature) · g (grande)\n\nDo agente: start, next, done, ask · Do dev: ok, no, change, size, stop"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Prepara o projeto: cria .passo/ e instala a skill, os agentes e os hooks
    Init {
        /// Plataforma: claude, copilot ou ambos (repita para mais de uma)
        #[arg(long = "agent", short, default_value = "claude")]
        agents: Vec<String>,
    },
    /// Reinstala os arquivos das plataformas (depois de editar .passo/agents)
    Sync,
    /// Começa um run com o pedido do dev
    Start {
        pedido: String,
        /// pp (ajuste) · p (pequeno) · m (feature) · g (grande)
        #[arg(long, short)]
        size: Option<String>,
    },
    /// O que o run precisa agora
    Next,
    /// Entrega a etapa: um resumo (construir), um arquivo JSON ou - (entrada padrão)
    Done {
        entrega: Option<String>,
        /// O passo não precisa mudar nenhum arquivo
        #[arg(long)]
        vazio: bool,
        /// Algo que você viu fora do passo
        #[arg(long)]
        nota: Option<String>,
    },
    /// Pergunta ao dev: a pergunta e de 2 a 6 opções, a recomendada primeiro ("texto — porquê")
    Ask {
        pergunta: String,
        #[arg(required = true, num_args = 2..=6)]
        opcoes: Vec<String>,
    },
    /// [dev] Aprova, responde ou segue: passo ok · passo ok 1b · passo ok "2=suas palavras"
    Ok { respostas: Vec<String> },
    /// [dev] Recusa o plano ou a entrega, dizendo o que mudar
    No { motivo: Vec<String> },
    /// [dev] Muda o pedido no meio do run
    Change { mudanca: Vec<String> },
    /// [dev] Troca o tamanho do run
    Size { tamanho: String },
    /// [dev] Encerra o run (o trabalho fica no branch)
    Stop { motivo: Vec<String> },
    /// Onde o run está e o que ele espera
    Status,
    /// O pedido refinado, o plano e as decisões
    Plan,
    /// Os runs do projeto
    Runs,
    /// O log de auditoria do run
    Log {
        #[arg(long)]
        run: Option<String>,
        /// Confere a cadeia de hashes
        #[arg(long)]
        verify: bool,
        /// Últimos N eventos
        #[arg(short = 'n', default_value_t = 40)]
        n: usize,
    },
    /// Métricas: tempo, vezes do dev, tokens por etapa e modelo
    Stats {
        #[arg(long)]
        run: Option<String>,
        /// Saída em JSON
        #[arg(long)]
        json: bool,
    },
    /// Painel ao vivo no navegador (localhost)
    Ui {
        #[arg(long, default_value_t = 4477)]
        port: u16,
        /// Não abre o navegador
        #[arg(long)]
        no_open: bool,
    },
    /// Gera o relatório HTML de um run (um arquivo, abre offline)
    Report {
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Confere a configuração, as camadas, os agentes e as plataformas
    Doctor,
    /// Usado pelos hooks das plataformas
    #[command(hide = true)]
    Hook {
        event: String,
        #[arg(long)]
        platform: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(text) => {
            if !text.is_empty() {
                println!("{}", paint::paint(&text));
            }
            ExitCode::SUCCESS
        }
        Err(e) if e.downcast_ref::<Rejected>().is_some() => {
            println!("{}", paint::paint(&e.to_string()));
            ExitCode::from(2)
        }
        Err(e) => {
            eprintln!("{}", paint::error(&format!("{e:#}")));
            ExitCode::FAILURE
        }
    }
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn app() -> Result<App> {
    Ok(App::open(&cwd())?.with_reporter(Arc::new(passo_web::HtmlReporter)))
}

fn size(text: &str) -> Result<Size> {
    Size::parse(text).ok_or_else(|| anyhow!("tamanho `{text}` não existe: use pp, p, m ou g"))
}

fn joined(words: Vec<String>) -> String {
    words.join(" ").trim().to_string()
}

/// The dev's subcommands, by the name the lock uses.
fn dev_subcommand(command: &Cmd) -> Option<&'static str> {
    match command {
        Cmd::Ok { .. } => Some("ok"),
        Cmd::No { .. } => Some("no"),
        Cmd::Change { .. } => Some("change"),
        Cmd::Size { .. } => Some("size"),
        Cmd::Stop { .. } => Some("stop"),
        _ => None,
    }
}

/// The platform whose agent environment this process runs in, if any.
fn agent_platform() -> Option<Box<dyn passo_adapters::Platform>> {
    let env = |key: &str| std::env::var(key).ok();
    passo_adapters::registry().into_iter().find(|p| p.agent_env(&env))
}

/// Dev decisions are refused inside an agent's environment, hooks or not: subagents run outside the hooks.
/// `PASSO_DEV=1` is for the dev who runs the CLI in the agent's terminal on purpose.
fn refuse_in_agent_env(command: &str) -> Result<()> {
    if std::env::var("PASSO_DEV").is_ok_and(|v| v == "1") || agent_platform().is_none() {
        return Ok(());
    }
    let reason = passo_core::lock::dev_command_refusal(command);
    if let Ok(app) = App::open(&cwd()) {
        app.record_denial(&reason, "cli");
    }
    Err(Rejected(reason).into())
}

fn run(command: Cmd) -> Result<String> {
    if let Some(dev) = dev_subcommand(&command) {
        refuse_in_agent_env(dev)?;
    }
    match command {
        Cmd::Init { agents } => init(agents),
        Cmd::Sync => {
            let app = app()?;
            let ids: Vec<String> = app.platforms().iter().map(|p| p.id().to_string()).collect();
            if ids.is_empty() {
                return Ok("Nenhuma plataforma instalada. Rode: passo init --agent claude".into());
            }
            let written = setup::install(&app, &ids, &setup::hook_bin())?;
            Ok(format!("Atualizado:\n{}", list_files(&written)))
        }
        Cmd::Start { pedido, size: s } => app()?.start(&pedido, s.as_deref().map(size).transpose()?),
        Cmd::Next => app()?.next(),
        Cmd::Done { entrega, vazio, nota } => {
            let text = entrega.unwrap_or_default();
            app()?.done(DoneInput {
                text,
                empty_ok: vazio,
                note: nota,
            })
        }
        Cmd::Ask { pergunta, opcoes } => app()?.ask(&pergunta, &opcoes),
        Cmd::Ok { respostas } => decide(DevAction::Approve(respostas)),
        Cmd::No { motivo } => decide(DevAction::Reject(joined(motivo))),
        Cmd::Change { mudanca } => decide(DevAction::Change(joined(mudanca))),
        Cmd::Size { tamanho } => decide(DevAction::Resize(size(&tamanho)?)),
        Cmd::Stop { motivo } => decide(DevAction::Stop(joined(motivo))),
        Cmd::Status => Ok(app()?.status_text()),
        Cmd::Plan => plan(),
        Cmd::Runs => runs(),
        Cmd::Log { run, verify, n } => log(run, verify, n),
        Cmd::Stats { run, json } => stats(run, json),
        Cmd::Ui { port, no_open } => {
            let app = app()?;
            passo_web::serve(app.layout.root.clone(), port, !no_open)?;
            Ok(String::new())
        }
        Cmd::Report { run, out } => {
            let app = app()?;
            let snapshot = app.snapshot(run.as_deref())?;
            let id = snapshot
                .run
                .as_ref()
                .map(|r| r.id.clone())
                .ok_or_else(|| anyhow!("nenhum run ainda"))?;
            let path = out.unwrap_or_else(|| app.layout.report(&id));
            passo_app::Reporter::write(&passo_web::HtmlReporter, &snapshot, &path)?;
            Ok(format!("Relatório: {}", path.display()))
        }
        Cmd::Doctor => doctor(),
        Cmd::Hook { event, platform } => {
            let Some(event) = HookEvent::parse(&event) else {
                return Ok(String::new());
            };
            let mut stdin = String::new();
            std::io::stdin().read_to_string(&mut stdin).ok();
            let reporter: Arc<dyn passo_app::Reporter> = Arc::new(passo_web::HtmlReporter);
            if let Some(reply) = passo_app::hooks::handle(&platform, event, &stdin, &cwd(), Some(reporter)) {
                print!("{reply}");
            }
            Ok(String::new())
        }
    }
}

fn decide(action: DevAction) -> Result<String> {
    let app = app()?;
    app.decide(action, Source::Cli)?;
    Ok(app.status_text())
}

fn init(agents: Vec<String>) -> Result<String> {
    let mut ids: Vec<String> = vec![];
    for agent in agents {
        match agent.as_str() {
            "ambos" | "both" | "todos" => ids.extend(passo_adapters::ids().iter().map(|s| s.to_string())),
            "none" | "nenhum" => {}
            other => ids.push(other.to_string()),
        }
    }
    ids.dedup();
    let root = cwd();
    let report = setup::init(&root, &ids, &setup::hook_bin())?;
    let app = App::open(&root)?;
    let mut out = String::from("Passo pronto neste projeto.\n");
    out.push_str(&list_files(&report.written));
    if let Some(preset) = report.preset {
        out.push_str(&format!(
            "\nStack detectada: {preset} (extends = [\"preset:{preset}\"])\n"
        ));
    }
    let show = |v: &[String]| {
        if v.is_empty() {
            "nenhuma".to_string()
        } else {
            v.join(" · ")
        }
    };
    out.push_str(&format!(
        "Checagens por passo: {}\nChecagens finais:    {}\nAjuste em .passo/passo.yaml e conte as convenções do projeto em .passo/context.md.\n",
        show(&app.config.checks.step),
        show(&app.config.checks.final_)
    ));
    if !ids.is_empty() {
        out.push_str(&format!(
            "\nCommite esses arquivos, abra uma sessão nova do agente ({}) e peça \"com o passo, …\".\nAcompanhe com: passo ui\n",
            ids.join(", ")
        ));
    }
    Ok(out)
}

fn list_files(files: &[PathBuf]) -> String {
    files.iter().map(|f| format!("  + {}\n", f.display())).collect()
}

fn plan() -> Result<String> {
    let app = app()?;
    let run = app.store.latest().ok_or_else(|| anyhow!("nenhum run ainda"))?;
    let mut out = format!("{}\n\nPedido: {}\n", run.headline(), run.request);
    if let Some(brief) = &run.brief {
        out.push_str(&format!("Entendimento: {}\n", brief.understanding));
        for (title, items) in [("Escopo", &brief.scope), ("Fora do escopo", &brief.out_of_scope)] {
            if !items.is_empty() {
                out.push_str(&format!(
                    "{title}:\n{}",
                    items.iter().map(|i| format!("  - {i}\n")).collect::<String>()
                ));
            }
        }
    }
    let decisions: Vec<String> = run
        .questions
        .iter()
        .filter_map(|q| q.answer_text().map(|a| format!("  {}: {} → {a}\n", q.id, q.ask)))
        .collect();
    if !decisions.is_empty() {
        out.push_str(&format!("Decisões:\n{}", decisions.concat()));
    }
    if let Some(plan) = &run.plan {
        out.push_str(&format!("\nPlano v{}: {}\n", run.plan_version, plan.title));
        for c in &run.criteria {
            out.push_str(&format!("  {} {}\n", c.id, c.text));
        }
        for s in &run.steps {
            let mark = match s.status {
                passo_core::StepStatus::Done => "✓",
                passo_core::StepStatus::Doing => "▸",
                passo_core::StepStatus::Todo => "·",
            };
            out.push_str(&format!("  {mark} {} {}\n", s.id, s.title));
        }
    }
    if !run.changes.is_empty() {
        out.push_str("\nMudanças no pedido:\n");
        for c in &run.changes {
            out.push_str(&format!(
                "  {} · {} · {}\n",
                c.at.format("%d/%m %H:%M"),
                c.kind.label(),
                c.text
            ));
        }
    }
    Ok(out)
}

fn runs() -> Result<String> {
    let app = app()?;
    let runs = app.store.list();
    if runs.is_empty() {
        return Ok("Nenhum run ainda.".into());
    }
    Ok(runs
        .iter()
        .map(|r| {
            let state = match r.status {
                passo_core::Status::Active => r.stage.label(),
                passo_core::Status::Done => "concluído",
                passo_core::Status::Aborted => "interrompido",
            };
            format!(
                "{}  {:<3} {:<12} {}  {}\n",
                r.created_at.format("%d/%m %H:%M"),
                r.size.id(),
                state,
                r.id,
                r.request
            )
        })
        .collect())
}

fn log(run: Option<String>, verify: bool, n: usize) -> Result<String> {
    let app = app()?;
    let run = match run {
        Some(id) => app.store.load(&id)?,
        None => app.store.latest().ok_or_else(|| anyhow!("nenhum run ainda"))?,
    };
    let log = app.store.log(&run.id);
    if verify {
        let anchored = if app.uses_git(&run) {
            " e de acordo com os commits"
        } else {
            ""
        };
        return Ok(match app.verify_audit(&run)? {
            Ok(count) => format!(
                "✓ Cadeia íntegra{anchored}: {count} eventos em {}",
                log.path().display()
            ),
            Err(why) => format!("✗ Auditoria quebrada: {why}"),
        });
    }
    let events = log.read()?;
    let skip = events.len().saturating_sub(n);
    Ok(events[skip..]
        .iter()
        .map(|e| {
            format!(
                "{}  {:<9} {:<7} {}\n",
                e.t.format("%H:%M:%S"),
                e.stage.label(),
                e.actor.label(),
                e.text
            )
        })
        .collect())
}

fn stats(run: Option<String>, json: bool) -> Result<String> {
    let app = app()?;
    let run = match run {
        Some(id) => app.store.load(&id)?,
        None => app.store.latest().ok_or_else(|| anyhow!("nenhum run ainda"))?,
    };
    let m = app.metrics(&run)?;
    if json {
        return Ok(serde_json::to_string_pretty(&m)?);
    }
    let mut out = format!("{}\n\n", run.headline());
    out.push_str(&format!(
        "Tempo total      {}\nVez do dev       {} vez(es), {} esperando\nPassos           {} feitos, {} tentativas que falharam\nVoltas           {} · plano v{} · {} mudança(s) no pedido\nAgente           {} ações · {} negadas pela trava · {} entregas inválidas\nEscaladas        {}\n",
        duration(m.total_ms),
        m.dev_turns,
        duration(m.dev_wait_ms),
        m.steps_done,
        m.steps_failed,
        m.rounds,
        m.plan_versions,
        m.changes,
        m.tool_calls,
        m.lock_denied,
        m.invalid_outputs,
        m.escalations
    ));
    if let Some((step, max)) = m.tools_per_step.iter().max_by_key(|(_, n)| **n) {
        let avg = m.tools_per_step.values().sum::<u32>() as f64 / m.tools_per_step.len() as f64;
        out.push_str(&format!("Ações por passo  média {avg:.0} · máximo {max} ({step})\n"));
    }
    out.push_str("\nEtapa        tempo      tokens\n");
    for s in &m.stages {
        let Some(stage) = s.stage else { continue };
        let tokens = if s.tokens.calls == 0 {
            "—".to_string()
        } else {
            count(s.tokens.billable())
        };
        out.push_str(&format!(
            "{:<12} {:<10} {}\n",
            stage.label(),
            if s.visits == 0 { "—".into() } else { duration(s.ms) },
            tokens
        ));
    }
    if m.tokens.calls > 0 {
        out.push_str(&format!(
            "\nTokens: {} (entrada {} · saída {} · cache lido {})\n",
            count(m.tokens.billable()),
            count(m.tokens.input),
            count(m.tokens.output),
            count(m.tokens.cache_read)
        ));
        for (model, t) in &m.by_model {
            out.push_str(&format!("  {model}: {} em {} chamadas\n", count(t.billable()), t.calls));
        }
    } else {
        out.push_str("\nTokens: sem transcrições (o Claude Code registra pelos hooks; o Copilot ainda não expõe)\n");
    }
    Ok(out)
}

fn doctor() -> Result<String> {
    let app = app()?;
    let mut out = format!(
        "Projeto: {}\n\nConfiguração (da mais fraca para a mais forte):\n",
        app.layout.root.display()
    );
    for layer in &app.layers {
        out.push_str(&format!("  · {layer}\n"));
    }
    let show = |v: &[String]| {
        if v.is_empty() {
            "nenhuma".to_string()
        } else {
            v.join(" · ")
        }
    };
    out.push_str(&format!(
        "\nChecagens por passo: {}\nChecagens finais:    {}\n",
        show(&app.config.checks.step),
        show(&app.config.checks.final_)
    ));
    out.push_str("\nTamanho  refinar  planejar  verificar  aprovar plano  perguntas  passos  limite\n");
    for s in Size::ALL {
        let p = app.config.profile(s);
        let mode = |m: passo_core::Mode| if m == passo_core::Mode::Auto { "auto" } else { "agente" };
        let limit = if p.max_files == 0 {
            "—".to_string()
        } else {
            format!("{} arq · {} lin", p.max_files, p.max_lines)
        };
        out.push_str(&format!(
            "{:<8} {:<8} {:<9} {:<10} {:<14} {:<10} {:<7} {}\n",
            s.id(),
            mode(p.refinar),
            mode(p.planejar),
            mode(p.verificar),
            if p.approve_plan { "sim" } else { "não" },
            p.max_questions,
            p.max_steps,
            limit
        ));
    }
    out.push_str("\nEtapas e agentes:\n");
    for stage in passo_core::Stage::ALL {
        let agent = app.catalog.agent_for(stage);
        let def = app.catalog.stage(stage);
        let who = match agent {
            Some(a) => {
                let models: Vec<String> = passo_adapters::ids()
                    .into_iter()
                    .filter_map(|p| app.config.model(&a.model, p).map(|m| format!("{p}: {m}")))
                    .collect();
                format!("{} (nível {}; {})", a.name, a.model, models.join(", "))
            }
            None => "o Passo".into(),
        };
        let sources = def.map(|d| d.sources.join(" + ")).unwrap_or_else(|| "embutido".into());
        out.push_str(&format!(
            "  {:<10} {who}\n             fonte: {sources}\n",
            stage.label()
        ));
    }
    let custom: Vec<&String> = app
        .catalog
        .agents
        .keys()
        .filter(|k| !["refinador", "planejador", "construtor", "revisor"].contains(&k.as_str()))
        .collect();
    if !custom.is_empty() {
        out.push_str(&format!(
            "Agentes do projeto: {}\n",
            custom.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    if let Some(platform) = agent_platform() {
        out.push_str(&format!(
            "\nAmbiente de agente: sim ({}). passo ok/no/change/size/stop são recusados aqui; PASSO_DEV=1 libera.\n",
            platform.label()
        ));
    }
    let platforms: Vec<String> = app.platforms().iter().map(|p| p.label().to_string()).collect();
    out.push_str(&format!(
        "\nPlataformas: {}\n",
        if platforms.is_empty() {
            "nenhuma (passo init --agent claude)".into()
        } else {
            platforms.join(" · ")
        }
    ));
    out.push_str(&format!(
        "Contexto do projeto: {}\n",
        if app.catalog.context.is_some() {
            ".passo/context.md ✓"
        } else {
            "vazio (preencha .passo/context.md)"
        }
    ));
    out.push_str(&format!("Mapa do projeto: {}\n", app.map_summary()));
    if let Some(run) = app.store.latest() {
        let verdict = match app.verify_audit(&run)? {
            Ok(n) => format!("✓ {n} eventos íntegros"),
            Err(why) => format!("✗ {why}"),
        };
        out.push_str(&format!("Auditoria do último run ({}): {verdict}\n", run.id));
    }
    Ok(out)
}
