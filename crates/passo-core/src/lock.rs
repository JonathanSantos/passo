//! The lock: what the agent may do right now. Platform-free; adapters translate hook payloads into `ToolUse`.
//!
//! Rules, in order: dev decisions are never the agent's; with a run active, `.passo/` and secrets are off limits,
//! git history is Passo's, and the project is writable only while building. Anything the lock cannot judge passes:
//! a hook must never block work it does not understand.

use std::path::{Component, Path, PathBuf};

use crate::model::{Run, Stage, Turn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Read,
    Write,
    Exec,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolUse {
    pub name: String,
    pub kind: ToolKind,
    /// Absolute paths the call touches.
    pub paths: Vec<PathBuf>,
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Allow,
    Deny(String),
}

/// Subcommands that belong to the dev: an agent running them would approve its own work.
pub const DEV_COMMANDS: [&str; 5] = ["ok", "no", "change", "size", "stop"];

/// Subcommands that move history or the branch: Passo's job during a run.
const GIT_WRITES: [&str; 15] = [
    "commit",
    "checkout",
    "switch",
    "reset",
    "restore",
    "push",
    "merge",
    "rebase",
    "stash",
    "cherry-pick",
    "worktree",
    "tag",
    "am",
    "apply",
    "revert",
];

/// Subcommands that change files through git instead of the editing tools the lock watches.
const GIT_FILE_OPS: [&str; 3] = ["rm", "mv", "clean"];

/// Finds `passo <dev command>` anywhere in a shell command.
pub fn dev_command_in(command: &str) -> Option<&'static str> {
    let words: Vec<&str> = command
        .split(|c: char| c.is_whitespace() || c == ';' || c == '&' || c == '|')
        .collect();
    for pair in words.windows(2) {
        let program = pair[0].rsplit('/').next().unwrap_or("");
        if program == "passo"
            && let Some(dev) = DEV_COMMANDS.iter().find(|d| **d == pair[1])
        {
            return Some(dev);
        }
    }
    None
}

fn git_write_in(command: &str) -> Option<&'static str> {
    let words: Vec<&str> = command
        .split(|c: char| c.is_whitespace() || c == ';' || c == '&' || c == '|')
        .collect();
    for (i, word) in words.iter().enumerate() {
        if *word != "git" {
            continue;
        }
        // Skip `-C dir` and `-c key=value` options before the subcommand.
        let mut j = i + 1;
        while j < words.len() && (words[j] == "-C" || words[j] == "-c") {
            j += 2;
        }
        if let Some(sub) = words.get(j)
            && let Some(found) = GIT_WRITES.iter().chain(GIT_FILE_OPS.iter()).find(|g| **g == *sub)
        {
            return Some(found);
        }
        if words.get(j) == Some(&"branch") && words.get(j + 1).is_some_and(|w| w.starts_with('-')) {
            return Some("branch");
        }
    }
    None
}

fn is_secret(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    (name == ".env" || name.starts_with(".env."))
        && !(name.ends_with(".example") || name.ends_with(".sample") || name.ends_with(".template"))
}

fn mentions_passo_dir(command: &str) -> bool {
    command
        .split(|c: char| c.is_whitespace() || "'\"=<>;|&()".contains(c))
        .any(|word| {
            word == ".passo" || word.starts_with(".passo/") || word.contains("/.passo/") || word.ends_with("/.passo")
        })
}

fn inside_passo_dir(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).ok().and_then(|rel| rel.components().next()) == Some(Component::Normal(".passo".as_ref()))
}

/// What an agent reads when it tries a dev decision, from a hook or from the CLI in an agent's environment.
pub fn dev_command_refusal(command: &str) -> String {
    format!(
        "`passo {command}` é decisão do dev. Pare e mostre ao dev o que o Passo pediu; ele responde aqui no chat (ex.: \"ok\") ou no terminal."
    )
}

pub fn judge(tool: &ToolUse, root: &Path, run: Option<&Run>) -> Verdict {
    if tool.kind == ToolKind::Exec
        && let Some(command) = &tool.command
        && let Some(dev) = dev_command_in(command)
    {
        return Verdict::Deny(dev_command_refusal(dev));
    }
    let Some(run) = run.filter(|r| r.active()) else {
        return Verdict::Allow;
    };
    if tool.paths.iter().any(|p| inside_passo_dir(root, p)) || tool.command.as_deref().is_some_and(mentions_passo_dir) {
        return Verdict::Deny("a pasta .passo é do Passo: use `passo next`, `passo status` e `passo plan`".into());
    }
    if tool.paths.iter().any(|p| is_secret(p)) {
        return Verdict::Deny("arquivos .env ficam fora do alcance do agente durante um run".into());
    }
    if tool.kind == ToolKind::Exec
        && let Some(command) = &tool.command
        && let Some(git) = git_write_in(command)
    {
        if GIT_FILE_OPS.contains(&git) {
            return Verdict::Deny(format!(
                "`git {git}` muda arquivos por fora da trava: apague ou renomeie com as ferramentas de edição, e deixe o git com o Passo"
            ));
        }
        return Verdict::Deny(format!(
            "`git {git}` é com o Passo durante um run: ele faz os commits e cuida da branch"
        ));
    }
    if tool.kind == ToolKind::Write {
        let in_project: Vec<&PathBuf> = tool.paths.iter().filter(|p| p.starts_with(root)).collect();
        if in_project.is_empty() {
            return Verdict::Allow;
        }
        if run.turn() == Turn::Dev {
            return Verdict::Deny(
                "é a vez do dev; nada se escreve no projeto agora. Mostre ao dev o que o Passo pediu e pare.".into(),
            );
        }
        if !run.stage.writes() {
            return Verdict::Deny(format!(
                "{} é só leitura; rascunhos vão para uma pasta temporária (mktemp -d). Rode `passo next`.",
                run.stage.label()
            ));
        }
        if run.stage == Stage::Construir && run.current_step().is_none() {
            return Verdict::Deny("não há passo em andamento; rode `passo next`".into());
        }
    }
    Verdict::Allow
}

/// Keeps the agent from ending its turn while the run waits on it. Dev turns always let it stop.
pub fn judge_stop(run: Option<&Run>) -> Option<String> {
    let run = run.filter(|r| r.active() && r.turn() == Turn::Agent)?;
    Some(format!(
        "O Passo espera você ({}). Rode `passo next` e siga a instrução; termine com `passo done`. Dúvida que muda o resultado vira `passo ask`.",
        run.headline()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Size;
    use chrono::Utc;

    fn run_at(stage: Stage) -> Run {
        let mut run = Run::new("r1", "x", Size::M, Utc::now());
        run.stage = stage;
        run
    }

    fn exec(cmd: &str) -> ToolUse {
        ToolUse {
            name: "Bash".into(),
            kind: ToolKind::Exec,
            paths: vec![],
            command: Some(cmd.into()),
        }
    }

    fn write(path: &str) -> ToolUse {
        ToolUse {
            name: "Write".into(),
            kind: ToolKind::Write,
            paths: vec![path.into()],
            command: None,
        }
    }

    #[test]
    fn dev_commands_are_never_the_agents() {
        let root = Path::new("/p");
        assert!(matches!(judge(&exec("passo ok 1b"), root, None), Verdict::Deny(_)));
        assert!(matches!(
            judge(&exec("cd x && ~/.local/bin/passo stop"), root, None),
            Verdict::Deny(_)
        ));
        assert_eq!(judge(&exec("passo done \"feito\""), root, None), Verdict::Allow);
        assert_eq!(judge(&exec("echo ok"), root, None), Verdict::Allow);
    }

    #[test]
    fn writes_only_while_building() {
        let root = Path::new("/p");
        let refine = run_at(Stage::Refinar);
        assert!(matches!(
            judge(&write("/p/src/a.rs"), root, Some(&refine)),
            Verdict::Deny(_)
        ));
        assert_eq!(judge(&write("/tmp/draft.json"), root, Some(&refine)), Verdict::Allow);
        let mut build = run_at(Stage::Construir);
        build.steps.push(crate::model::Step {
            id: "S1".into(),
            title: "t".into(),
            status: crate::model::StepStatus::Doing,
            ..Default::default()
        });
        assert_eq!(judge(&write("/p/src/a.rs"), root, Some(&build)), Verdict::Allow);
        assert!(matches!(
            judge(&write("/p/.passo/runs/x.json"), root, Some(&build)),
            Verdict::Deny(_)
        ));
        assert!(matches!(judge(&write("/p/.env"), root, Some(&build)), Verdict::Deny(_)));
        assert_eq!(judge(&write("/p/.env.example"), root, Some(&build)), Verdict::Allow);
    }

    #[test]
    fn git_history_is_passos() {
        let root = Path::new("/p");
        let run = run_at(Stage::Construir);
        assert!(matches!(
            judge(&exec("git -C /p commit -m x"), root, Some(&run)),
            Verdict::Deny(_)
        ));
        assert!(matches!(
            judge(&exec("git branch -D main"), root, Some(&run)),
            Verdict::Deny(_)
        ));
        assert!(matches!(
            judge(&exec("git clean -fd"), root, Some(&run)),
            Verdict::Deny(_)
        ));
        assert!(matches!(
            judge(&exec("git rm src/old.rs"), root, Some(&run)),
            Verdict::Deny(_)
        ));
        assert_eq!(judge(&exec("git diff HEAD"), root, Some(&run)), Verdict::Allow);
        assert_eq!(judge(&exec("git commit -m x"), root, None), Verdict::Allow);
    }

    #[test]
    fn stop_only_blocks_on_agent_turn() {
        let run = run_at(Stage::Construir);
        assert!(judge_stop(Some(&run)).is_some());
        assert!(judge_stop(None).is_none());
    }
}
