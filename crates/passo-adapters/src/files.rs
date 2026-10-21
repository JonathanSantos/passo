//! File helpers for installers: write a file, keep the team's own content when re-installing.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{Map, Value};

pub fn write(root: &Path, rel: &str, content: &str) -> Result<PathBuf> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, content)?;
    Ok(PathBuf::from(rel))
}

pub fn read_json(path: &Path) -> Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Replaces Passo's hook entries in a hooks table (`{ Event: [entries] }`), keeping everyone else's.
pub fn merge_hooks(hooks: &mut Map<String, Value>, event: &str, entry: Value) {
    let mut list: Vec<Value> = hooks.get(event).and_then(Value::as_array).cloned().unwrap_or_default();
    list.retain(|e| !is_passo(e));
    list.push(entry);
    hooks.insert(event.to_string(), Value::Array(list));
}

/// Passo's hook commands always read `… hook <event> --platform <id>`.
fn is_passo(entry: &Value) -> bool {
    let text = entry.to_string();
    text.contains(" hook ") && text.contains("--platform")
}

/// Keeps a block of text between markers in a file the team also edits.
pub fn managed_block(root: &Path, rel: &str, block: &str) -> Result<PathBuf> {
    const START: &str = "<!-- passo:start -->";
    const END: &str = "<!-- passo:end -->";
    let path = root.join(rel);
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let section = format!("{START}\n{}\n{END}", block.trim());
    let next = match (current.find(START), current.find(END)) {
        (Some(a), Some(b)) if b > a => format!("{}{section}{}", &current[..a], &current[b + END.len()..]),
        _ if current.trim().is_empty() => format!("{section}\n"),
        _ => format!("{}\n\n{section}\n", current.trim_end()),
    };
    write(root, rel, &next)
}

/// Frontmatter list in YAML flow style.
pub fn yaml_list(items: &[String]) -> String {
    format!(
        "[{}]",
        items.iter().map(|i| format!("'{i}'")).collect::<Vec<_>>().join(", ")
    )
}

/// A YAML scalar that survives colons and quotes.
pub fn yaml_text(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}
