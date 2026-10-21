//! Configuration as plain data. Loading and `extends` live in the store; the engine only reads this.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{Size, Stage};

/// Who does a stage: an agent, or Passo on its own (instantly, no tokens).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Agente,
    Auto,
}

/// How a size runs the stages. Sizes change how deep each stage goes, never the order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub refinar: Mode,
    pub planejar: Mode,
    pub verificar: Mode,
    /// Ask the dev to approve the plan. Open questions always reach the dev, whatever this says.
    pub approve_plan: bool,
    pub max_questions: usize,
    pub max_steps: usize,
    /// A step with a `teste` must change a test file.
    pub require_tests: bool,
    /// Diff limits checked by Passo (0 = no limit). Past them, the run stops and the dev decides.
    pub max_files: usize,
    pub max_lines: usize,
    /// Touching a sensitive path stops the run.
    pub block_sensitive: bool,
    /// Times verify may send the run back to build before the dev is called.
    pub max_rounds: u32,
    /// Failed attempts of a step before its next attempt asks for the strong tier (0 = never).
    pub escalate_after: u32,
    /// Tool calls per step (per stage, outside build) before the agent is told to hand in or ask (0 = no limit).
    pub max_tool_calls: u32,
    /// Lines of the project map each stage may read (0 = no map in that stage).
    pub map_lines: MapLines,
}

/// How many lines of the project map each agent stage reads. Delivering reads none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapLines {
    pub refinar: usize,
    pub planejar: usize,
    pub construir: usize,
    pub verificar: usize,
}

impl MapLines {
    pub const OFF: MapLines = MapLines {
        refinar: 0,
        planejar: 0,
        construir: 0,
        verificar: 0,
    };
    pub const DEFAULT: MapLines = MapLines {
        refinar: 60,
        planejar: 120,
        construir: 60,
        verificar: 100,
    };

    pub fn for_stage(&self, stage: Stage) -> usize {
        match stage {
            Stage::Refinar => self.refinar,
            Stage::Planejar => self.planejar,
            Stage::Construir => self.construir,
            Stage::Verificar => self.verificar,
            Stage::Entregar => 0,
        }
    }
}

impl Profile {
    pub fn mode(&self, stage: Stage) -> Mode {
        match stage {
            Stage::Refinar => self.refinar,
            Stage::Planejar => self.planejar,
            Stage::Verificar => self.verificar,
            // Building is always an agent's job; delivering is always Passo's, with the dev's acceptance.
            Stage::Construir => Mode::Agente,
            Stage::Entregar => Mode::Auto,
        }
    }

    pub fn defaults(size: Size) -> Profile {
        match size {
            Size::Pp => Profile {
                refinar: Mode::Auto,
                planejar: Mode::Auto,
                verificar: Mode::Auto,
                approve_plan: false,
                max_questions: 1,
                max_steps: 1,
                require_tests: false,
                max_files: 3,
                max_lines: 120,
                block_sensitive: true,
                max_rounds: 1,
                // A small fix that fails is the wrong size, not the wrong model.
                escalate_after: 0,
                max_tool_calls: 30,
                map_lines: MapLines::OFF,
            },
            Size::P => Profile {
                refinar: Mode::Agente,
                planejar: Mode::Agente,
                verificar: Mode::Agente,
                approve_plan: true,
                max_questions: 3,
                max_steps: 3,
                require_tests: true,
                max_files: 10,
                max_lines: 400,
                block_sensitive: false,
                max_rounds: 2,
                escalate_after: 1,
                max_tool_calls: 60,
                map_lines: MapLines::DEFAULT,
            },
            Size::M => Profile {
                refinar: Mode::Agente,
                planejar: Mode::Agente,
                verificar: Mode::Agente,
                approve_plan: true,
                max_questions: 5,
                max_steps: 6,
                require_tests: true,
                max_files: 0,
                max_lines: 0,
                block_sensitive: false,
                max_rounds: 2,
                escalate_after: 1,
                max_tool_calls: 80,
                map_lines: MapLines::DEFAULT,
            },
            Size::G => Profile {
                refinar: Mode::Agente,
                planejar: Mode::Agente,
                verificar: Mode::Agente,
                approve_plan: true,
                max_questions: 8,
                max_steps: 12,
                require_tests: true,
                max_files: 0,
                max_lines: 0,
                block_sensitive: false,
                max_rounds: 3,
                escalate_after: 1,
                max_tool_calls: 120,
                map_lines: MapLines::DEFAULT,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Checks {
    /// After every build step: fast (typecheck, lint, unit tests of the area).
    pub step: Vec<String>,
    /// At verify: the full suite.
    #[serde(rename = "final")]
    pub final_: Vec<String>,
    pub timeout_secs: u64,
}

impl Default for Checks {
    fn default() -> Self {
        Checks {
            step: vec![],
            final_: vec![],
            timeout_secs: 600,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitMode {
    /// A branch per run; Passo commits each step.
    Branch,
    /// Work in place, no commits.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    /// Accepting merges the run branch back.
    Merge,
    /// Accepting leaves the branch for a pull request.
    Keep,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Git {
    pub mode: GitMode,
    pub delivery: Delivery,
}

impl Default for Git {
    fn default() -> Self {
        Git {
            mode: GitMode::Branch,
            delivery: Delivery::Merge,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// Tries per step before the dev is called.
    pub attempts: u32,
    /// Globs that count as sensitive.
    pub sensitive: Vec<String>,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            attempts: 3,
            sensitive: [
                "**/auth/**",
                "**/migrations/**",
                "**/*.sql",
                ".github/**",
                "**/.env*",
                "**/payment*/**",
                "**/secrets/**",
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tests {
    /// Globs that count as test files.
    pub patterns: Vec<String>,
    /// Programs a step's test command (`rodar`) may start with, besides the ones the checks use.
    pub runners: Vec<String>,
}

impl Default for Tests {
    fn default() -> Self {
        Tests {
            patterns: ["**/*test*", "**/*spec*", "**/tests/**", "**/test/**", "**/__tests__/**"]
                .map(String::from)
                .to_vec(),
            runners: [
                "cargo", "npm", "pnpm", "yarn", "bun", "node", "pytest", "python", "go", "make", "just", "mvn",
                "gradle", "dotnet", "mix", "bundle", "rspec",
            ]
            .map(String::from)
            .to_vec(),
        }
    }
}

/// What a step's test command may never contain: it is a plain call to a test runner, not a shell line.
const SHELL_TOKENS: [(&str, &str); 10] = [
    ("$(", "`$(`"),
    ("`", "crase"),
    ("&&", "`&&`"),
    ("||", "`||`"),
    (";", "`;`"),
    ("|", "`|`"),
    (">", "`>`"),
    ("<", "`<`"),
    ("\n", "quebra de linha"),
    ("\\", "`\\`"),
];

impl Config {
    /// Programs a test command may start with: `tests.runners` and the first word of every configured check.
    pub fn test_runners(&self) -> Vec<String> {
        let mut runners = self.tests.runners.clone();
        for check in self.checks.step.iter().chain(&self.checks.final_) {
            if let Some(program) = check.split_whitespace().next() {
                let program = program.rsplit('/').next().unwrap_or(program).to_string();
                if !runners.contains(&program) {
                    runners.push(program);
                }
            }
        }
        runners
    }

    /// Why a step's test command is not a plain call to a test runner, if it is not. `field` names it in the error.
    pub fn refuse_test_command(&self, field: &str, command: &str) -> Option<String> {
        if let Some((_, name)) = SHELL_TOKENS.iter().find(|(token, _)| command.contains(token)) {
            return Some(format!(
                "`{field}` não pode ter {name}: o comando de teste é uma chamada simples, sem shell"
            ));
        }
        let first = command.split_whitespace().next().unwrap_or("");
        let program = first.rsplit('/').next().unwrap_or(first);
        let runners = self.test_runners();
        if !runners.iter().any(|r| r == program) {
            return Some(format!(
                "`{field}` começa com `{program}`, que não é um executor de testes (permitidos: {}; ou acrescente em tests.runners)",
                runners.join(", ")
            ));
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Other config files merged below this one (paths, or `preset:<name>`).
    pub extends: Vec<String>,
    /// Size when the agent does not say.
    pub default_size: Size,
    pub checks: Checks,
    pub git: Git,
    pub limits: Limits,
    pub tests: Tests,
    pub sizes: BTreeMap<Size, Profile>,
    /// Model tiers the agents ask for, per platform: `{ leve = { <plataforma> = "<modelo>" } }`. Each platform
    /// fills in its own defaults for [`TIERS`]; the config only has to name what differs.
    pub models: BTreeMap<String, BTreeMap<String, String>>,
    /// Repositories next to this one that requests depend on: read-only for Passo, their map read with a prefix.
    pub projects: Vec<AttachedProject>,
}

/// Another repository this project depends on (`../api`). Passo reads its map and never writes to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachedProject {
    /// The prefix its map's ids get: `api/armazenamento`.
    pub name: String,
    /// Relative to this project's root, or absolute.
    pub path: String,
}

/// The built-in model tiers: a cheap one and a strong one. Projects may add their own.
pub const TIERS: [&str; 2] = ["leve", "forte"];

impl Default for Config {
    fn default() -> Self {
        Config {
            extends: vec![],
            default_size: Size::M,
            checks: Checks::default(),
            git: Git::default(),
            limits: Limits::default(),
            tests: Tests::default(),
            sizes: Size::ALL.into_iter().map(|s| (s, Profile::defaults(s))).collect(),
            models: BTreeMap::new(),
            projects: vec![],
        }
    }
}

impl Config {
    pub fn profile(&self, size: Size) -> Profile {
        self.sizes
            .get(&size)
            .cloned()
            .unwrap_or_else(|| Profile::defaults(size))
    }

    pub fn model(&self, tier: &str, platform: &str) -> Option<&str> {
        self.models.get(tier).and_then(|m| m.get(platform)).map(String::as_str)
    }
}
