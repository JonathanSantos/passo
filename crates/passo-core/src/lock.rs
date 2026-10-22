//! The lock: what the agent may do right now. Platform-free; adapters translate hook payloads into `ToolUse`.
//!
//! Rules, in order: dev decisions are never the agent's; with a run active, `.passo/` and secrets are off limits,
//! git history is Passo's, and the project is writable only while building. Shell commands are read with
//! [`crate::shell::lex`], and the lock looks for pairs in **every** word of every command (`passo ok`, `git commit`),
//! wherever they sit: after `sudo`, inside `{ … }`, after `then`. Every file a line redirects to or names is judged;
//! a path with a variable or a wildcard that could reach `.env` or `.passo` is refused. A command the lexer cannot
//! read is refused when it mentions `passo` or `git`, and in read-only stages; while building it passes, and the git
//! check after each step covers what it wrote.

use std::path::{Component, Path, PathBuf};

use crate::model::{Run, Stage, Turn};
use crate::shell::{self, Script};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolKind {
    Read,
    Write,
    Exec,
    #[default]
    Other,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolUse {
    pub name: String,
    pub kind: ToolKind,
    /// Absolute paths the call touches.
    pub paths: Vec<PathBuf>,
    pub command: Option<String>,
    /// Where a shell command runs (relative paths in it resolve from here); the project root when unknown.
    pub cwd: Option<PathBuf>,
}

/// Why a command the lexer cannot read is refused.
pub fn unreadable(why: &str) -> String {
    format!("não consigo ler este comando ({why}); escreva o comando de forma direta")
}

pub const WHY_DYNAMIC_PROGRAM: &str = "expansão no nome do programa";
pub const NO_SUBCOMMAND: &str = "diga o subcomando do passo por extenso";
pub const PROTECTED_PATTERN: &str = "curinga ou variável num caminho protegido; escreva o caminho por extenso";
pub const DYNAMIC_TARGET: &str = "alvo do redirecionamento com variável; use um caminho fixo em mktemp -d";
pub const SWAPPED_ENV: &str = "o Passo e o git rodam com o ambiente do projeto, sem PATH ou bibliotecas trocadas";

/// Programs that run the command after them: a `passo` after one of these is in a program's place.
const WRAPPERS: [&str; 9] = [
    "sudo", "env", "xargs", "nice", "nohup", "time", "timeout", "command", "exec",
];
/// Variables that change which program or library runs.
const SWAPS: [&str; 6] = [
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "PATH",
    "BASH_ENV",
];
/// Git options whose value is the next word when it has no `=`.
const GIT_VALUE_OPTIONS: [&str; 5] = ["-C", "-c", "--git-dir", "--work-tree", "--namespace"];

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Allow,
    Deny(String),
}

/// Subcommands that belong to the dev: an agent running them would approve its own work.
pub const DEV_COMMANDS: [&str; 5] = ["ok", "no", "change", "size", "stop"];

/// Subcommands that move history or the branch: Passo's job during a run.
const GIT_WRITES: [&str; 22] = [
    "update-ref",
    "symbolic-ref",
    "reflog",
    "filter-branch",
    "gc",
    "prune",
    "replace",
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

/// Finds `passo <dev command>` in any command a shell line runs.
pub fn dev_command_in(command: &str) -> Option<&'static str> {
    dev_command_in_script(&shell::lex(command))
}

fn basename(text: &str) -> &str {
    text.rsplit('/').next().unwrap_or(text)
}

/// `passo <dev>` anywhere in any command: `sudo -u x passo ok`, `{ passo ok; }`, `find . -exec passo ok \;`.
fn dev_command_in_script(script: &Script) -> Option<&'static str> {
    for cmd in &script.commands {
        for (i, word) in cmd.words.iter().enumerate() {
            if basename(&word.text) != "passo" {
                continue;
            }
            if let Some(dev) = dev_after(&cmd.words[i + 1..]) {
                return Some(dev);
            }
        }
    }
    None
}

/// The dev decision in the words after `passo`, if any.
fn dev_after(words: &[shell::Word]) -> Option<&'static str> {
    let sub = words.first().map(|w| w.text.as_str())?;
    if let Some(dev) = DEV_COMMANDS.iter().find(|d| **d == sub) {
        return Some(dev);
    }
    let flags = || words[1..].iter().take_while(|w| w.text.starts_with('-'));
    // `passo map` is the agent's; cancelling a mapping is the dev's.
    if sub == "map" && flags().any(|w| w.text == "--cancel") {
        return Some("map --cancel");
    }
    // `passo learn` is the agent's; applying or dropping what it proposed is the dev's.
    if sub == "learn" {
        if flags().any(|w| w.text == "--apply" || w.text.starts_with("--apply=")) {
            return Some("learn --apply");
        }
        if flags().any(|w| w.text == "--cancel") {
            return Some("learn --cancel");
        }
    }
    None
}

/// `passo` in a program's place with no subcommand, or one only known when it runs: `passo $s`, `passo o${x}k`.
fn passo_without_subcommand(script: &Script) -> bool {
    script.commands.iter().any(|cmd| {
        let wrapped = cmd.words.first().is_some_and(|w| WRAPPERS.contains(&basename(&w.text)));
        cmd.words.iter().enumerate().any(|(i, word)| {
            (i == 0 || wrapped)
                && basename(&word.text) == "passo"
                && cmd.words.get(i + 1).is_none_or(|next| next.dynamic)
        })
    })
}

/// `LD_PRELOAD=./x.so passo ok`, `env PATH=/tmp git push`: a swapped environment for `passo` or `git`.
fn swapped_env(script: &Script) -> bool {
    script.commands.iter().any(|cmd| {
        let runs_ours = cmd.words.iter().any(|w| matches!(basename(&w.text), "passo" | "git"));
        let named = cmd.assignments.iter().map(|(name, _)| name.as_str()).chain(
            cmd.words
                .iter()
                .filter_map(|w| w.text.split_once('=').map(|(name, _)| name)),
        );
        runs_ours && named.into_iter().any(|name| SWAPS.contains(&name))
    })
}

/// What the project is busy with, as the lock sees it.
#[derive(Debug, Clone, Copy)]
pub enum Activity<'a> {
    Idle,
    Run(&'a Run),
    /// An agent is exploring the project to write its map: read-only.
    Mapping,
    /// An agent is turning the dev's repeated corrections into proposals: read-only.
    Learning,
}

impl<'a> Activity<'a> {
    /// An active run wins; otherwise a pending mapping, then a pending learning; otherwise nothing.
    pub fn of(run: Option<&'a Run>, mapping: bool, learning: bool) -> Activity<'a> {
        match run.filter(|r| r.active()) {
            Some(run) => Activity::Run(run),
            None if mapping => Activity::Mapping,
            None if learning => Activity::Learning,
            None => Activity::Idle,
        }
    }
}

pub const MAPPING_READ_ONLY: &str = "o mapeamento é só leitura: o mapa é entregue com passo done";
pub const LEARNING_READ_ONLY: &str = "aprender com os runs é só leitura: as propostas são entregues com passo done";

/// `git <write>` anywhere in any command, past git's own options (`git --no-pager -C x commit`).
fn git_write_in(script: &Script) -> Option<&'static str> {
    for cmd in &script.commands {
        let words: Vec<&str> = cmd.words.iter().map(|w| w.text.as_str()).collect();
        for at in (0..words.len()).filter(|&i| basename(words[i]) == "git") {
            let mut j = at + 1;
            while j < words.len() && words[j].starts_with('-') {
                j += if GIT_VALUE_OPTIONS.contains(&words[j]) { 2 } else { 1 };
            }
            if let Some(sub) = words.get(j)
                && let Some(found) = GIT_WRITES.iter().chain(GIT_FILE_OPS.iter()).find(|g| *g == sub)
            {
                return Some(found);
            }
            if words.get(j) == Some(&"branch") && words.get(j + 1).is_some_and(|w| w.starts_with('-')) {
                return Some("branch");
            }
        }
    }
    None
}

fn wildcard(text: &str) -> bool {
    text.contains(['*', '?', '['])
}

/// A path whose value is only known when it runs (a variable, a substitution, a wildcard) that could still reach
/// `.env` or `.passo`: the literal part after the last variable names one, or a wildcard on a dotted name could
/// match one (`.en?`, `.e*`, `.pass?`, `.*`). A plain `*` never matches a leading dot.
fn reaches_protected(word: &shell::Word) -> bool {
    if !word.dynamic {
        return false;
    }
    let tail = word.text.rsplit('$').next().unwrap_or(&word.text);
    let literal_hit = tail.split('/').any(|part| part == ".passo")
        || tail.rsplit('/').next().is_some_and(|name| is_secret(Path::new(name)));
    let pattern_hit = word.text.split('/').any(|part| {
        part.starts_with('.') && wildcard(part) && [".env", ".env.local", ".passo"].iter().any(|n| glob(part, n))
    });
    literal_hit || pattern_hit
}

/// `*`, `?` and `[…]` against a name, the way a shell expands one path component.
fn glob(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match p.first() {
            None => n.is_empty(),
            Some('*') => (0..=n.len()).any(|k| go(&p[1..], &n[k..])),
            Some('?') => !n.is_empty() && go(&p[1..], &n[1..]),
            Some('[') => match p.iter().position(|c| *c == ']') {
                Some(end) if !n.is_empty() => go(&p[end + 1..], &n[1..]),
                _ => false,
            },
            Some(c) => n.first() == Some(c) && go(&p[1..], &n[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    go(&p, &n)
}

/// A redirect target with a variable is safe when the variable part comes from `mktemp` (always outside the
/// project): `> "$(mktemp)"`, or `f=$(mktemp) && cat > "$f"`, or `$TMPDIR/x`.
fn temp_target(target: &shell::Word, script: &Script) -> bool {
    let from_mktemp = |subs: &[String]| {
        subs.first()
            .is_some_and(|s| basename(s.split_whitespace().next().unwrap_or("")) == "mktemp")
    };
    if from_mktemp(&target.subs) {
        return true;
    }
    target.vars.first().is_some_and(|var| {
        var == "TMPDIR"
            || script
                .commands
                .iter()
                .flat_map(|c| c.assignments.iter())
                .any(|(name, value)| name == var && from_mktemp(&value.subs))
    })
}

/// `base` joined with `path` and `.`/`..` resolved, without touching the disk.
fn resolve(base: &Path, path: &str) -> PathBuf {
    let joined = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        base.join(path)
    };
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The files a shell line writes (redirect targets) and names (words that look like paths), resolved from `cwd`.
/// Dynamic words (`"$(mktemp)"`, `$HOME/x`) are left out: their value is only known when the line runs.
fn shell_paths(script: &Script, cwd: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut writes = vec![];
    let mut reads = vec![];
    for cmd in &script.commands {
        for redirect in cmd
            .redirects
            .iter()
            .filter(|r| !r.target.dynamic && !r.target.text.is_empty())
        {
            let path = resolve(cwd, &redirect.target.text);
            if redirect.write {
                writes.push(path)
            } else {
                reads.push(path)
            }
        }
        for word in cmd.words.iter().skip(1).filter(|w| !w.dynamic) {
            if word.text.contains('/') || (word.text.starts_with('.') && word.text.len() > 1) {
                reads.push(resolve(cwd, &word.text));
            }
        }
    }
    (writes, reads)
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

pub fn judge(tool: &ToolUse, root: &Path, activity: Activity) -> Verdict {
    judge_with(tool, root, &[], activity)
}

/// `judge`, with attached projects: roots Passo reads and never writes, during a run or a mapping.
pub fn judge_with(tool: &ToolUse, root: &Path, read_only: &[PathBuf], activity: Activity) -> Verdict {
    let script = tool
        .command
        .as_deref()
        .filter(|_| tool.kind == ToolKind::Exec)
        .map(shell::lex);
    if let Some(dev) = script.as_ref().and_then(dev_command_in_script) {
        return Verdict::Deny(dev_command_refusal(dev));
    }
    let run = match activity {
        Activity::Idle => return Verdict::Allow,
        Activity::Run(run) => Some(run),
        Activity::Mapping | Activity::Learning => None,
    };
    let idle_read_only = if matches!(activity, Activity::Learning) {
        LEARNING_READ_ONLY
    } else {
        MAPPING_READ_ONLY
    };
    let read_only_stage = run.is_none_or(|r| r.turn() == Turn::Dev || !r.stage.writes());

    // What a shell line touches joins what the tool declared.
    let mut writes: Vec<PathBuf> = if tool.kind == ToolKind::Write {
        tool.paths.clone()
    } else {
        vec![]
    };
    let mut reads: Vec<PathBuf> = if tool.kind == ToolKind::Write {
        vec![]
    } else {
        tool.paths.clone()
    };
    if let (Some(script), Some(command)) = (&script, &tool.command) {
        // The raw text: heredoc bodies and every stage of a pipe count (`echo 'passo ok' | sh`).
        let mentions = crate::map::contains_word(command, "passo") || crate::map::contains_word(command, "git");
        if !script.readable && (mentions || read_only_stage) {
            return Verdict::Deny(unreadable(script.why.unwrap_or(shell::WHY_QUOTES)));
        }
        let dynamic_program = script
            .commands
            .iter()
            .any(|c| c.words.first().is_some_and(|w| w.dynamic));
        if dynamic_program && (mentions || read_only_stage) {
            return Verdict::Deny(unreadable(WHY_DYNAMIC_PROGRAM));
        }
        if passo_without_subcommand(script) {
            return Verdict::Deny(NO_SUBCOMMAND.into());
        }
        if swapped_env(script) {
            return Verdict::Deny(SWAPPED_ENV.into());
        }
        let named = script
            .commands
            .iter()
            .flat_map(|c| c.words.iter().skip(1).chain(c.redirects.iter().map(|r| &r.target)));
        if named.into_iter().any(reaches_protected) {
            return Verdict::Deny(PROTECTED_PATTERN.into());
        }
        let blind_write = script
            .commands
            .iter()
            .flat_map(|c| c.redirects.iter())
            .any(|r| r.write && (r.target.dynamic || wildcard(&r.target.text)) && !temp_target(&r.target, script));
        if blind_write && read_only_stage {
            return Verdict::Deny(DYNAMIC_TARGET.into());
        }
        let (w, r) = shell_paths(script, tool.cwd.as_deref().unwrap_or(root));
        writes.extend(w);
        reads.extend(r);
    }

    if let Some(attached) = read_only.iter().find(|r| writes.iter().any(|p| p.starts_with(r))) {
        return Verdict::Deny(format!(
            "{} é um projeto anexado: o Passo só o lê. Mude-o com o Passo de lá, num run próprio.",
            attached.display()
        ));
    }
    let touched = || writes.iter().chain(reads.iter());
    if touched().any(|p| inside_passo_dir(root, p)) || tool.command.as_deref().is_some_and(mentions_passo_dir) {
        return Verdict::Deny("a pasta .passo é do Passo: use `passo next`, `passo status` e `passo plan`".into());
    }
    if touched().any(|p| is_secret(p)) {
        return Verdict::Deny("arquivos .env ficam fora do alcance do agente durante um run".into());
    }
    if let Some(git) = script.as_ref().and_then(git_write_in) {
        if GIT_FILE_OPS.contains(&git) {
            return Verdict::Deny(format!(
                "`git {git}` muda arquivos por fora da trava: apague ou renomeie com as ferramentas de edição, e deixe o git com o Passo"
            ));
        }
        if run.is_none() {
            return Verdict::Deny(idle_read_only.into());
        }
        return Verdict::Deny(format!(
            "`git {git}` é com o Passo durante um run: ele faz os commits e cuida da branch"
        ));
    }
    if writes.iter().any(|p| p.starts_with(root)) {
        let Some(run) = run else {
            return Verdict::Deny(idle_read_only.into());
        };
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
pub fn judge_stop(activity: Activity) -> Option<String> {
    let run = match activity {
        Activity::Idle => return None,
        Activity::Mapping => return Some("O Passo espera o mapa: entregue com passo done <arquivo>.".into()),
        Activity::Learning => {
            return Some("O Passo espera as propostas do que aprender: entregue com passo done <arquivo>.".into());
        }
        Activity::Run(run) => run,
    };
    let run = Some(run).filter(|r| r.active() && r.turn() == Turn::Agent)?;
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
            ..Default::default()
        }
    }

    fn write(path: &str) -> ToolUse {
        ToolUse {
            name: "Write".into(),
            kind: ToolKind::Write,
            paths: vec![path.into()],
            command: None,
            ..Default::default()
        }
    }

    #[test]
    fn dev_commands_are_never_the_agents() {
        let root = Path::new("/p");
        assert!(matches!(
            judge(&exec("passo ok 1b"), root, Activity::Idle),
            Verdict::Deny(_)
        ));
        assert!(matches!(
            judge(&exec("cd x && ~/.local/bin/passo stop"), root, Activity::Idle),
            Verdict::Deny(_)
        ));
        assert_eq!(
            judge(&exec("passo done \"feito\""), root, Activity::Idle),
            Verdict::Allow
        );
        assert_eq!(judge(&exec("echo ok"), root, Activity::Idle), Verdict::Allow);
    }

    #[test]
    fn writes_only_while_building() {
        let root = Path::new("/p");
        let refine = run_at(Stage::Refinar);
        assert!(matches!(
            judge(&write("/p/src/a.rs"), root, Activity::Run(&refine)),
            Verdict::Deny(_)
        ));
        assert_eq!(
            judge(&write("/tmp/draft.json"), root, Activity::Run(&refine)),
            Verdict::Allow
        );
        let mut build = run_at(Stage::Construir);
        build.steps.push(crate::model::Step {
            id: "S1".into(),
            title: "t".into(),
            status: crate::model::StepStatus::Doing,
            ..Default::default()
        });
        assert_eq!(
            judge(&write("/p/src/a.rs"), root, Activity::Run(&build)),
            Verdict::Allow
        );
        assert!(matches!(
            judge(&write("/p/.passo/runs/x.json"), root, Activity::Run(&build)),
            Verdict::Deny(_)
        ));
        assert!(matches!(
            judge(&write("/p/.env"), root, Activity::Run(&build)),
            Verdict::Deny(_)
        ));
        assert_eq!(
            judge(&write("/p/.env.example"), root, Activity::Run(&build)),
            Verdict::Allow
        );
    }

    #[test]
    fn git_history_is_passos() {
        let root = Path::new("/p");
        let run = run_at(Stage::Construir);
        assert!(matches!(
            judge(&exec("git -C /p commit -m x"), root, Activity::Run(&run)),
            Verdict::Deny(_)
        ));
        assert!(matches!(
            judge(&exec("git branch -D main"), root, Activity::Run(&run)),
            Verdict::Deny(_)
        ));
        assert!(matches!(
            judge(&exec("git clean -fd"), root, Activity::Run(&run)),
            Verdict::Deny(_)
        ));
        assert!(matches!(
            judge(&exec("git rm src/old.rs"), root, Activity::Run(&run)),
            Verdict::Deny(_)
        ));
        assert_eq!(judge(&exec("git diff HEAD"), root, Activity::Run(&run)), Verdict::Allow);
        assert_eq!(judge(&exec("git commit -m x"), root, Activity::Idle), Verdict::Allow);
    }

    #[test]
    fn stop_only_blocks_on_agent_turn() {
        let run = run_at(Stage::Construir);
        assert!(judge_stop(Activity::Run(&run)).is_some());
        assert!(judge_stop(Activity::Idle).is_none());
    }

    #[test]
    fn mapping_is_read_only_and_cancelling_it_is_the_devs() {
        let root = Path::new("/p");
        assert_eq!(
            judge(&write("/p/src/a.rs"), root, Activity::Mapping),
            Verdict::Deny(MAPPING_READ_ONLY.into())
        );
        assert!(matches!(
            judge(&write("/p/.passo/map/x.md"), root, Activity::Mapping),
            Verdict::Deny(_)
        ));
        assert_eq!(judge(&write("/tmp/mapa.json"), root, Activity::Mapping), Verdict::Allow);
        let read = ToolUse {
            name: "Read".into(),
            kind: ToolKind::Read,
            paths: vec!["/p/src/a.rs".into()],
            command: None,
            cwd: None,
        };
        assert_eq!(judge(&read, root, Activity::Mapping), Verdict::Allow);
        assert!(matches!(
            judge(&exec("git stash"), root, Activity::Mapping),
            Verdict::Deny(_)
        ));
        assert_eq!(dev_command_in("passo map --cancel"), Some("map --cancel"));
        assert_eq!(dev_command_in("passo map"), None);
        assert!(judge_stop(Activity::Mapping).is_some());
        assert_eq!(
            judge(&write("/p/src/a.rs"), root, Activity::Learning),
            Verdict::Deny(LEARNING_READ_ONLY.into())
        );
        assert!(judge_stop(Activity::Learning).is_some());
        assert_eq!(dev_command_in("passo learn --apply 1,3"), Some("learn --apply"));
        assert_eq!(dev_command_in("passo learn --cancel"), Some("learn --cancel"));
        assert_eq!(dev_command_in("passo learn"), None);
    }

    #[test]
    fn attached_projects_are_read_only_while_busy() {
        let root = Path::new("/p");
        let attached = vec![PathBuf::from("/api")];
        let mut run = run_at(Stage::Construir);
        run.steps.push(crate::model::Step {
            id: "S1".into(),
            status: crate::model::StepStatus::Doing,
            ..Default::default()
        });
        assert!(matches!(
            judge_with(&write("/api/src/x.ts"), root, &attached, Activity::Run(&run)),
            Verdict::Deny(_)
        ));
        assert!(matches!(
            judge_with(&write("/api/src/x.ts"), root, &attached, Activity::Mapping),
            Verdict::Deny(_)
        ));
        assert_eq!(
            judge_with(&write("/api/src/x.ts"), root, &attached, Activity::Idle),
            Verdict::Allow
        );
        assert_eq!(
            judge_with(&write("/p/src/a.rs"), root, &attached, Activity::Run(&run)),
            Verdict::Allow
        );
    }

    #[test]
    fn the_lock_reads_shell_lines() {
        let root = Path::new("/p");
        let refine = run_at(Stage::Refinar);
        let mut plan = run_at(Stage::Planejar);
        plan.stage = Stage::Planejar;
        let mut build = run_at(Stage::Construir);
        build.steps.push(crate::model::Step {
            id: "S1".into(),
            status: crate::model::StepStatus::Doing,
            ..Default::default()
        });
        let denied = [
            ("sh -c 'passo stop'", &build),
            ("echo $(passo ok)", &build),
            ("x=`git commit -m a`", &build),
            ("\"passo\" no \"x\"", &build),
            ("p=passo; $p ok", &build),
            ("cat .env", &build),
            ("cat ./.env.local", &build),
            ("echo x > src/a.rs", &refine),
            ("xargs passo ok", &build),
            ("eval \"passo ok\"", &build),
        ];
        for (command, run) in denied {
            assert!(
                matches!(judge(&exec(command), root, Activity::Run(run)), Verdict::Deny(_)),
                "should deny: {command}"
            );
        }
        let allowed = [
            ("passo done \"ok\"", &build),
            ("git diff", &build),
            ("grep passo README.md", &build),
            ("echo \"passo ok\" > /tmp/nota.txt", &refine),
            ("cat .env.example", &build),
            ("npm test -- --grep 'ok'", &build),
            ("passo done - <<'EOF'\n{\"entendimento\": \"x\"}\nEOF", &refine),
            ("cat > \"$(mktemp)\" <<EOF\n{\"titulo\": \"$x\"}\nEOF", &plan),
        ];
        for (command, run) in allowed {
            assert_eq!(
                judge(&exec(command), root, Activity::Run(run)),
                Verdict::Allow,
                "should allow: {command}"
            );
        }
        assert_eq!(
            judge(&exec("cat .env"), root, Activity::Idle),
            Verdict::Allow,
            "no run, no lock"
        );
        assert!(matches!(
            judge(&exec("sh -c 'passo stop'"), root, Activity::Idle),
            Verdict::Deny(_)
        ));
        assert_eq!(
            judge(&exec("echo 'sem fim"), root, Activity::Run(&refine)),
            Verdict::Deny(unreadable(shell::WHY_QUOTES))
        );
        assert_eq!(
            judge(&exec("echo 'sem fim"), root, Activity::Run(&build)),
            Verdict::Allow
        );
        assert_eq!(
            judge(&exec("echo 'passo ok"), root, Activity::Run(&build)),
            Verdict::Deny(unreadable(shell::WHY_QUOTES))
        );
    }

    fn building() -> Run {
        let mut build = run_at(Stage::Construir);
        build.steps.push(crate::model::Step {
            id: "S1".into(),
            status: crate::model::StepStatus::Doing,
            ..Default::default()
        });
        build
    }

    #[test]
    fn the_known_ways_around_the_lock_are_closed() {
        let root = Path::new("/p");
        let build = building();
        let refine = run_at(Stage::Refinar);
        let envs = format!("{}passo ok", "env ".repeat(32));
        let sudos = format!("{}passo ok", "sudo nice ".repeat(17));
        let denied: Vec<(&str, &Run)> = vec![
            (&envs, &build),
            (&sudos, &build),
            ("{ passo ok; }", &build),
            ("if true; then passo ok; fi", &build),
            ("for x in 1; do passo ok; done", &build),
            ("f(){ passo ok; }; f", &build),
            ("bash -lc 'passo ok'", &build),
            ("sh -ec 'passo stop'", &build),
            ("bash -c -- 'passo ok'", &build),
            ("echo 'passo ok' | sh", &build),
            ("sh <<'EOF'\npasso ok\nEOF", &build),
            ("bash <<< 'passo ok'", &build),
            ("env -S 'passo ok'", &build),
            ("passo $s", &build),
            ("passo $(echo ok)", &build),
            ("passo o${x}k", &build),
            ("cat .en?", &build),
            ("cat .env*", &build),
            ("cat \"$PWD/.env\"", &build),
            ("cat .pass?/current", &build),
            ("echo x > .pass?/current", &build),
            ("echo x > src/$f.rs", &refine),
            ("echo x > src/a.r?", &refine),
            ("git --no-pager commit -m x", &build),
            ("git --git-dir=.git commit -m x", &build),
            ("git -p commit -m x", &build),
            ("git update-ref HEAD abc", &build),
            ("git symbolic-ref HEAD refs/heads/x", &build),
            (r"find . -exec passo ok \;", &build),
            ("sudo -u x git push", &build),
            ("LD_PRELOAD=./x.so passo ok", &build),
            ("$(printf 'pas%s' so) ok", &refine),
        ];
        for (command, run) in denied {
            assert!(
                matches!(judge(&exec(command), root, Activity::Run(run)), Verdict::Deny(_)),
                "should deny: {command}"
            );
        }
        let allowed: Vec<(&str, &Run)> = vec![
            ("env FOO=1 npm test", &build),
            ("sudo apt list", &build),
            ("{ npm test; }", &build),
            ("if npm test; then echo ok; fi", &build),
            ("bash -lc 'npm test'", &build),
            ("passo done - <<'EOF'\n{\"x\": \"passo ok\"}\nEOF", &refine),
            ("cat \"$HOME/.config/x\"", &refine),
            ("git --no-pager log", &build),
            ("echo x > src/$f.rs", &build),
            ("f=$(mktemp) && cat > \"$f\" <<'EOF'\n{\"a\": 1}\nEOF", &refine),
            ("cat > \"$(mktemp -d)/plano.json\" <<'EOF'\n{}\nEOF", &refine),
        ];
        for (command, run) in allowed {
            assert_eq!(
                judge(&exec(command), root, Activity::Run(run)),
                Verdict::Allow,
                "should allow: {command}"
            );
        }
        assert_eq!(
            judge(&exec("passo $s"), root, Activity::Run(&build)),
            Verdict::Deny(NO_SUBCOMMAND.into())
        );
        assert_eq!(
            judge(&exec("bash script.sh"), root, Activity::Run(&refine)),
            Verdict::Deny(unreadable(shell::WHY_SHELL))
        );
    }

    #[test]
    fn the_ceilings_keep_judging_fast() {
        let root = Path::new("/p");
        let build = building();
        let cases = [
            format!("{}passo ok", "env ".repeat(200)),
            format!("{}passo ok", "sudo nice ".repeat(100)),
            format!("{}passo ok{}", "echo $(".repeat(50), ")".repeat(50)),
        ];
        for command in &cases {
            let _ = judge(&exec(command), root, Activity::Run(&build));
            let started = std::time::Instant::now();
            let verdict = judge(&exec(command), root, Activity::Run(&build));
            let took = started.elapsed();
            assert!(matches!(verdict, Verdict::Deny(_)), "{command}");
            assert!(took.as_millis() < 5, "{} ms: {}", took.as_millis(), &command[..40]);
        }
        // The structure behind the time: one command per prefix chain, nothing multiplied.
        assert_eq!(shell::lex(&cases[0]).commands.len(), 1);
        assert!(!shell::lex(&cases[2]).readable);
        assert!(!shell::lex(&"echo a;".repeat(200)).readable, "past 128 commands");
        assert!(
            !shell::lex(&format!("echo {}", "a".repeat(20_000))).readable,
            "past 16 KiB"
        );
    }

    #[test]
    fn a_vocabulary_corpus_never_lets_passo_ok_through() {
        let root = Path::new("/p");
        let build = building();
        let vocabulary = [
            "env", "sudo", "nice", "sh", "-c", "{", "then", "do", "passo", "ok", "git", "commit", "$(", "<<EOF",
        ];
        let mut seed: u64 = 0xbeef;
        for _ in 0..500 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let len = 1 + (seed >> 33) % 12;
            let mut tokens = vec![];
            for _ in 0..len {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                tokens.push(vocabulary[((seed >> 33) as usize) % vocabulary.len()]);
            }
            let text = tokens.join(" ");
            let started = std::time::Instant::now();
            let verdict = judge(&exec(&text), root, Activity::Run(&build));
            assert!(started.elapsed().as_millis() < 50, "{text}");
            if tokens.windows(2).any(|w| w == ["passo", "ok"]) {
                assert!(matches!(verdict, Verdict::Deny(_)), "should deny: {text}");
            }
        }
    }

    #[test]
    fn judging_a_malformed_corpus_never_panics() {
        let root = Path::new("/p");
        let run = run_at(Stage::Refinar);
        let alphabet: Vec<char> = "ab $()`'\"\\;|&<>#\n{}-=*~./passogit".chars().collect();
        let mut seed: u64 = 0xfeed;
        for _ in 0..200 {
            let mut text = String::new();
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            for _ in 0..(seed >> 33) % 40 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                text.push(alphabet[((seed >> 33) as usize) % alphabet.len()]);
            }
            let _ = judge(&exec(&text), root, Activity::Run(&run));
        }
    }
}
