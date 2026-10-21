//! Loads `passo.toml` in layers. Weakest to strongest:
//!
//! 1. built-in defaults
//! 2. `~/.passo/passo.toml` (the dev's own taste)
//! 3. each file in `extends`, in order (`preset:rust`, `../shared/passo.toml`, `~/team.toml`)
//! 4. the project's `.passo/passo.toml`
//!
//! Tables merge key by key; lists and values replace. That is the whole `extends` model.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use passo_core::Config;
use toml::{Table, Value};

use crate::assets;
use crate::layout::{Layout, global_home, home_dir};

/// Where each layer came from, for `passo doctor`.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub config: Config,
    pub layers: Vec<String>,
}

pub fn load(layout: &Layout) -> Result<Loaded> {
    let mut merged = Value::try_from(Config::default())?
        .as_table()
        .cloned()
        .unwrap_or_default();
    let mut layers = vec!["padrões do Passo".to_string()];
    if let Some(home) = global_home() {
        let file = home.join("passo.toml");
        if file.is_file() {
            apply_file(&mut merged, &file, &mut layers, &mut BTreeSet::new())?;
        }
    }
    let project = layout.config();
    if project.is_file() {
        apply_file(&mut merged, &project, &mut layers, &mut BTreeSet::new())?;
    }
    merged.remove("extends");
    let config: Config = Value::Table(merged)
        .try_into()
        .map_err(|e: toml::de::Error| anyhow!("configuração inválida: {}", e.message()))?;
    Ok(Loaded { config, layers })
}

/// Applies a file's `extends` first, then the file itself.
fn apply_file(into: &mut Table, file: &Path, layers: &mut Vec<String>, seen: &mut BTreeSet<String>) -> Result<()> {
    let key = file.display().to_string();
    if !seen.insert(key.clone()) {
        bail!("extends em ciclo: {key}");
    }
    let text = std::fs::read_to_string(file).with_context(|| format!("lendo {key}"))?;
    let table: Table = text
        .parse()
        .map_err(|e: toml::de::Error| anyhow!("{key}: {}", e.message()))?;
    apply_table(into, table, file.parent(), &key, layers, seen)
}

fn apply_table(
    into: &mut Table,
    table: Table,
    base: Option<&Path>,
    name: &str,
    layers: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
) -> Result<()> {
    if let Some(Value::Array(parents)) = table.get("extends") {
        for parent in parents {
            let parent = parent
                .as_str()
                .ok_or_else(|| anyhow!("{name}: extends deve ser uma lista de textos"))?;
            if let Some(preset) = parent.strip_prefix("preset:") {
                let text = assets::preset(preset).ok_or_else(|| {
                    anyhow!(
                        "{name}: preset `{preset}` não existe (há: {})",
                        assets::PRESETS.map(|p| p.0).join(", ")
                    )
                })?;
                let preset_table: Table = text.parse().map_err(|e: toml::de::Error| anyhow!("{e}"))?;
                apply_table(into, preset_table, None, parent, layers, seen)?;
            } else {
                apply_file(into, &resolve(parent, base), layers, seen)?;
            }
        }
    }
    merge(into, table);
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

/// Deep merge: tables merge, everything else replaces.
pub fn merge(into: &mut Table, from: Table) {
    for (key, value) in from {
        match (into.get_mut(&key), value) {
            (Some(Value::Table(existing)), Value::Table(incoming)) => merge(existing, incoming),
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

    #[test]
    fn extends_layers_merge_tables_and_replace_lists() {
        let dir = std::env::temp_dir().join(format!("passo-config-{}", std::process::id()));
        let passo = dir.join(".passo");
        std::fs::create_dir_all(&passo).unwrap();
        std::fs::write(
            dir.join("team.toml"),
            "[checks]\nstep = [\"make lint\"]\nfinal = [\"make test\"]\n[sizes.p]\nverificar = \"auto\"\n",
        )
        .unwrap();
        std::fs::write(
            passo.join("passo.toml"),
            "extends = [\"preset:rust\", \"../team.toml\"]\ndefault_size = \"p\"\n[checks]\nstep = [\"cargo check\"]\n",
        )
        .unwrap();
        let loaded = load(&Layout::new(&dir)).unwrap();
        assert_eq!(loaded.config.checks.step, vec!["cargo check".to_string()]);
        assert_eq!(loaded.config.checks.final_, vec!["make test".to_string()]);
        assert_eq!(loaded.config.default_size, Size::P);
        let p = loaded.config.profile(Size::P);
        assert_eq!(p.verificar, Mode::Auto);
        assert_eq!(p.max_steps, 3, "untouched keys keep their defaults");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn unknown_keys_are_reported() {
        let dir = std::env::temp_dir().join(format!("passo-config-bad-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".passo")).unwrap();
        std::fs::write(dir.join(".passo/passo.toml"), "[checks]\nstpe = []\n").unwrap();
        let err = load(&Layout::new(&dir)).unwrap_err().to_string();
        assert!(err.contains("stpe"), "{err}");
        std::fs::remove_dir_all(dir).ok();
    }
}
