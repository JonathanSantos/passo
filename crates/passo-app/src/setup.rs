//! `passo init` and `passo sync`: create `.passo/`, pick a preset from the stack, install the platforms.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use passo_adapters::{AgentSpec, InstallContext};
use passo_store::{Layout, Store, assets};

use crate::service::App;

pub struct InitReport {
    pub written: Vec<PathBuf>,
    pub preset: Option<&'static str>,
}

/// Guesses the stack from the files at the root.
pub fn detect_preset(root: &Path) -> Option<&'static str> {
    [
        ("Cargo.toml", "rust"),
        ("package.json", "node"),
        ("pyproject.toml", "python"),
        ("requirements.txt", "python"),
        ("go.mod", "go"),
    ]
    .into_iter()
    .find(|(file, _)| root.join(file).exists())
    .map(|(_, preset)| preset)
}

const CONTEXT_TEMPLATE: &str = "# Convenções do projeto\n\n<!--\nTodo agente do Passo lê este arquivo. O Passo já mostra a árvore de pastas e os testes; escreva aqui o que a árvore não diz, o que um dev novo precisaria saber:\n- onde fica o quê (pastas, camadas)\n- como se escrevem os testes\n- bibliotecas que o time usa (e as que evita)\n- idioma de nomes e comentários\nApague este comentário quando preencher.\n-->\n";

pub fn init(root: &Path, platforms: &[String], bin: &str) -> Result<InitReport> {
    for id in platforms {
        if passo_adapters::get(id).is_none() {
            bail!(
                "plataforma `{id}` desconhecida (há: {})",
                passo_adapters::ids().join(", ")
            );
        }
    }
    let layout = Layout::new(root);
    passo_store::config::check_legacy(&layout)?;
    let store = Store::new(layout.clone());
    store.init()?;
    let mut written = vec![];
    let preset = detect_preset(root);
    if !layout.config().exists() {
        let mut text = assets::CONFIG_TEMPLATE.to_string();
        if let Some(preset) = preset {
            text = text.replace(
                "# extends: [preset:rust, ../time/passo.yaml]",
                &format!("extends: [preset:{preset}]"),
            );
            // The preset brings the checks; empty lists here would replace them.
            text = text
                .replace("  step: []\n", "  # step: []\n")
                .replace("  final: []\n", "  # final: []\n");
            // A preset with a coverage command only needs the minimum to turn it on.
            if passo_store::assets::preset(preset).is_some_and(|p| p.contains("coverage:")) {
                text = text.replace(
                    "  #   command: cargo llvm-cov --lcov --output-path {out} --quiet\n",
                    &format!("  #   (o comando vem do preset:{preset})\n"),
                );
            }
        }
        std::fs::write(layout.config(), text)?;
        written.push(PathBuf::from(".passo/passo.yaml"));
    }
    if !layout.context().exists() {
        std::fs::write(layout.context(), CONTEXT_TEMPLATE)?;
        written.push(PathBuf::from(".passo/context.md"));
    }
    let app = App::at(layout)?;
    written.extend(install(&app, platforms, bin)?);
    Ok(InitReport { written, preset })
}

/// Writes the platform files from the current catalog (re-run after editing `.passo/agents`).
pub fn install(app: &App, platforms: &[String], bin: &str) -> Result<Vec<PathBuf>> {
    let skill = assets::SKILL.replace("{{bin}}", "passo");
    let mut written = vec![];
    for id in platforms {
        let Some(platform) = passo_adapters::get(id) else {
            continue;
        };
        let (cheap, strong) = (passo_core::config::TIERS[0], passo_core::config::TIERS[1]);
        let mut agents: Vec<AgentSpec> = vec![];
        for agent in app.catalog.agents.values() {
            agents.push(AgentSpec {
                name: agent.name.clone(),
                description: agent.description.clone(),
                model: app.config.model(&agent.model, id).map(str::to_string),
                tools: agent.tools.clone(),
                body: agent.body.clone(),
            });
            // A cheap agent gets a strong twin for the attempts after it got stuck.
            if agent.model == cheap {
                agents.push(AgentSpec {
                    name: format!("{}-{strong}", agent.name),
                    description: format!(
                        "{} Nível {strong}: o Passo chama este quando o nível {cheap} travou num passo.",
                        agent.description
                    ),
                    model: app.config.model(strong, id).map(str::to_string),
                    tools: agent.tools.clone(),
                    body: agent.body.clone(),
                });
            }
        }
        let ctx = InstallContext {
            root: &app.layout.root,
            bin,
            skill: &skill,
            agents: &agents,
        };
        written.extend(platform.install(&ctx)?);
    }
    Ok(written)
}

/// How hooks should call Passo: `passo` when it is on the PATH, else the absolute path of this binary.
pub fn hook_bin() -> String {
    let on_path = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join("passo").is_file()))
        .unwrap_or(false);
    if on_path {
        return "passo".into();
    }
    std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .map(|p| if p.contains(' ') { format!("\"{p}\"") } else { p })
        .unwrap_or_else(|| "passo".into())
}
