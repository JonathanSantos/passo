//! What ships inside the binary: agents, stage instructions, checklists, presets and the agent skill.
//! A project overrides any of them with a file of the same name under `.passo/`.

macro_rules! asset {
    ($path:literal) => {
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/", $path))
    };
}

pub const AGENTS: [(&str, &str); 4] = [
    ("refinador", asset!("agents/refinador.md")),
    ("planejador", asset!("agents/planejador.md")),
    ("construtor", asset!("agents/construtor.md")),
    ("revisor", asset!("agents/revisor.md")),
];

pub const STAGES: [(&str, &str); 4] = [
    ("refinar", asset!("stages/refinar.md")),
    ("planejar", asset!("stages/planejar.md")),
    ("construir", asset!("stages/construir.md")),
    ("verificar", asset!("stages/verificar.md")),
];

pub const CHECKLISTS: [(&str, &str); 4] = [
    ("api-http", asset!("checklists/api-http.md")),
    ("cli", asset!("checklists/cli.md")),
    ("ui", asset!("checklists/ui.md")),
    ("dados", asset!("checklists/dados.md")),
];

pub const PRESETS: [(&str, &str); 4] = [
    ("rust", asset!("presets/rust.toml")),
    ("node", asset!("presets/node.toml")),
    ("python", asset!("presets/python.toml")),
    ("go", asset!("presets/go.toml")),
];

/// The skill every platform installs: how to drive a run with the CLI.
pub const SKILL: &str = asset!("skill/SKILL.md");

/// The starting config `passo init` writes.
pub const CONFIG_TEMPLATE: &str = asset!("passo.toml");

pub fn preset(name: &str) -> Option<&'static str> {
    PRESETS.iter().find(|(n, _)| *n == name).map(|(_, text)| *text)
}
