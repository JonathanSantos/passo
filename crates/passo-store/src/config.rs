//! Loads `passo.yaml` in layers. Weakest to strongest:
//!
//! 1. built-in defaults
//! 2. `~/.passo/passo.yaml` (the dev's own taste)
//! 3. each file in `extends`, in order (`preset:rust`, `../shared/passo.yaml`, `~/team.yaml`)
//! 4. the project's `.passo/passo.yaml`
//!
//! Maps merge key by key; lists and values replace. That is the whole `extends` model.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use passo_core::Config;
use serde_yaml_ng::{Mapping, Value};

use crate::assets;
use crate::layout::{Layout, global_home, home_dir};

pub const FILE: &str = "passo.yaml";

/// What a project that still has the old file reads.
pub const TOML_MIGRATION: &str =
    "o Passo agora lê .passo/passo.yaml; converta o passo.toml (mesmas chaves, sintaxe YAML) e apague o antigo";

/// Where each layer came from, for `passo doctor`.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub config: Config,
    pub layers: Vec<String>,
}

/// Fails when the project still has a `passo.toml` from before the move to YAML.
pub fn check_legacy(layout: &Layout) -> Result<()> {
    if layout.passo().join("passo.toml").exists() {
        bail!("{TOML_MIGRATION}");
    }
    Ok(())
}

pub fn load(layout: &Layout) -> Result<Loaded> {
    check_legacy(layout)?;
    let mut merged = match serde_yaml_ng::to_value(Config::default())? {
        Value::Mapping(map) => map,
        _ => Mapping::new(),
    };
    let mut layers = vec!["padrões do Passo".to_string()];
    if let Some(home) = global_home() {
        let file = home.join(FILE);
        if file.is_file() {
            apply_file(&mut merged, &file, &mut layers, &mut BTreeSet::new())?;
        }
    }
    let project = layout.config();
    if project.is_file() {
        apply_file(&mut merged, &project, &mut layers, &mut BTreeSet::new())?;
    }
    merged.remove("extends");
    let config: Config =
        serde_yaml_ng::from_value(Value::Mapping(merged)).map_err(|e| anyhow!("configuração inválida: {e}"))?;
    Ok(Loaded { config, layers })
}

fn parse(text: &str, name: &str) -> Result<Mapping> {
    match serde_yaml_ng::from_str::<Value>(text).map_err(|e| anyhow!("{name}: {e}"))? {
        Value::Mapping(map) => Ok(map),
        Value::Null => Ok(Mapping::new()),
        _ => bail!("{name}: a configuração deve ser um mapa de `chave: valor`"),
    }
}

/// Applies a file's `extends` first, then the file itself.
fn apply_file(into: &mut Mapping, file: &Path, layers: &mut Vec<String>, seen: &mut BTreeSet<String>) -> Result<()> {
    let key = file.display().to_string();
    if !seen.insert(key.clone()) {
        bail!("extends em ciclo: {key}");
    }
    let text = std::fs::read_to_string(file).with_context(|| format!("lendo {key}"))?;
    let map = parse(&text, &key)?;
    apply_map(into, map, file.parent(), &key, layers, seen)
}

fn apply_map(
    into: &mut Mapping,
    map: Mapping,
    base: Option<&Path>,
    name: &str,
    layers: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
) -> Result<()> {
    let parents = match map.get("extends") {
        None | Some(Value::Null) => vec![],
        Some(Value::String(one)) => vec![one.clone()],
        Some(Value::Sequence(items)) => items
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| anyhow!("{name}: extends deve ser uma lista de textos"))?,
        Some(_) => bail!("{name}: extends deve ser uma lista de textos"),
    };
    for parent in parents {
        if let Some(preset) = parent.strip_prefix("preset:") {
            let text = assets::preset(preset).ok_or_else(|| {
                anyhow!(
                    "{name}: preset `{preset}` não existe (há: {})",
                    assets::PRESETS.map(|p| p.0).join(", ")
                )
            })?;
            apply_map(into, parse(text, &parent)?, None, &parent, layers, seen)?;
        } else {
            apply_file(into, &resolve(&parent, base), layers, seen)?;
        }
    }
    merge(into, map);
    layers.push(name.to_string());
    Ok(())
}

fn resolve(path: &str, base: Option<&Path>) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = home_dir()
    {
        return home.join(rest);
    }
    let path = PathBuf::from(path);
    match base {
        Some(base) if path.is_relative() => base.join(path),
        _ => path,
    }
}

/// Deep merge: maps merge, everything else replaces.
pub fn merge(into: &mut Mapping, from: Mapping) {
    for (key, value) in from {
        match (into.get_mut(&key), value) {
            (Some(Value::Mapping(existing)), Value::Mapping(incoming)) => merge(existing, incoming),
            (_, value) => {
                into.insert(key, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use passo_core::{Mode, Size};

    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("passo-config-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join(".passo")).unwrap();
        dir
    }

    #[test]
    fn extends_layers_merge_maps_and_replace_lists() {
        let dir = project("layers");
        std::fs::write(
            dir.join("team.yaml"),
            "checks:\n  step: [make lint]\n  final: [make test]\nsizes:\n  p:\n    verificar: auto\n",
        )
        .unwrap();
        std::fs::write(
            dir.join(".passo/passo.yaml"),
            "extends: [preset:rust, ../team.yaml]\ndefault_size: p\nchecks:\n  step:\n    - cargo check\n",
        )
        .unwrap();
        let loaded = load(&Layout::new(&dir)).unwrap();
        assert_eq!(loaded.config.checks.step, vec!["cargo check".to_string()]);
        assert_eq!(loaded.config.checks.final_, vec!["make test".to_string()]);
        assert_eq!(loaded.config.default_size, Size::P);
        let p = loaded.config.profile(Size::P);
        assert_eq!(p.verificar, Mode::Auto);
        assert_eq!(p.max_steps, 3, "untouched keys keep their defaults");
        assert!(loaded.layers.iter().any(|l| l == "preset:rust"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn test_runners_in_yaml_widen_the_usual_list() {
        let dir = project("runners");
        std::fs::write(dir.join(".passo/passo.yaml"), "tests:\n  runners: [vitest]\n").unwrap();
        let config = load(&Layout::new(&dir)).unwrap().config;
        let runners = config.test_runners();
        assert!(
            runners.contains(&"vitest".to_string()) && runners.contains(&"cargo".to_string()),
            "{runners:?}"
        );
        assert_eq!(config.refuse_test_command("passos[0].rodar", "cargo test x"), None);

        std::fs::write(
            dir.join(".passo/passo.yaml"),
            "tests:\n  runners: [vitest]\n  runners_only: true\n",
        )
        .unwrap();
        let only = load(&Layout::new(&dir)).unwrap().config.test_runners();
        assert_eq!(only, vec!["vitest".to_string()]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn extends_relative_to_the_file_that_names_it() {
        let dir = project("relative");
        std::fs::create_dir_all(dir.join("shared/nested")).unwrap();
        std::fs::write(dir.join("shared/base.yaml"), "extends: [nested/deep.yaml]\n").unwrap();
        std::fs::write(dir.join("shared/nested/deep.yaml"), "limits:\n  attempts: 5\n").unwrap();
        std::fs::write(dir.join(".passo/passo.yaml"), "extends: ../shared/base.yaml\n").unwrap();
        assert_eq!(load(&Layout::new(&dir)).unwrap().config.limits.attempts, 5);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn extends_in_a_cycle_is_an_error() {
        let dir = project("cycle");
        std::fs::write(dir.join("a.yaml"), "extends: [b.yaml]\n").unwrap();
        std::fs::write(dir.join("b.yaml"), "extends: [a.yaml]\n").unwrap();
        std::fs::write(dir.join(".passo/passo.yaml"), "extends: [../a.yaml]\n").unwrap();
        let err = load(&Layout::new(&dir)).unwrap_err().to_string();
        assert!(err.contains("ciclo"), "{err}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn unknown_keys_are_reported() {
        let dir = project("bad");
        std::fs::write(dir.join(".passo/passo.yaml"), "checks:\n  stpe: []\n").unwrap();
        let err = format!("{:#}", load(&Layout::new(&dir)).unwrap_err());
        assert!(err.contains("stpe"), "{err}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_old_config_asks_for_migration() {
        let dir = project("legacy");
        std::fs::write(dir.join(".passo/passo.toml"), "default_size = \"p\"\n").unwrap();
        let err = load(&Layout::new(&dir)).unwrap_err().to_string();
        assert_eq!(err, TOML_MIGRATION);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn every_preset_parses() {
        for (name, text) in assets::PRESETS {
            parse(text, name).unwrap();
        }
    }
}
