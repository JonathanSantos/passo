//! Hook payload reading shared by the platforms: both send JSON with a tool name, its input and a cwd, under
//! slightly different keys.

use std::path::{Path, PathBuf};

use passo_core::lock::{ToolKind, ToolUse};
use serde_json::Value;

const PATH_FIELDS: [&str; 6] = ["file_path", "filePath", "path", "notebook_path", "notebookPath", "uri"];
const LIST_FIELDS: [&str; 3] = ["files", "filePaths", "paths"];

pub fn text(raw: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| raw.get(*k).and_then(Value::as_str))
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

/// Tool input may come as an object or as a JSON string.
fn input(raw: &Value, keys: &[&str]) -> Value {
    for key in keys {
        match raw.get(*key) {
            Some(Value::String(s)) => {
                if let Ok(v) = serde_json::from_str(s) {
                    return v;
                }
            }
            Some(v @ Value::Object(_)) => return v.clone(),
            _ => {}
        }
    }
    Value::Null
}

/// Classifies a tool by name. Exact names first, then words in the name; unknown tools are `Other` (allowed).
pub fn kind_of(name: &str) -> ToolKind {
    let lower = name.to_lowercase();
    match lower.as_str() {
        "read" | "view" | "glob" | "grep" | "ls" | "read_file" | "readfile" | "search" | "codebase" => {
            return ToolKind::Read;
        }
        "write" | "edit" | "multiedit" | "notebookedit" | "create" | "apply_patch" | "str_replace_editor" => {
            return ToolKind::Write;
        }
        "bash" | "shell" | "powershell" | "run_in_terminal" | "runinterminal" | "execute" => return ToolKind::Exec,
        _ => {}
    }
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    if has(&[
        "edit", "write", "create", "replace", "insert", "patch", "delete", "rename", "move",
    ]) {
        ToolKind::Write
    } else if has(&["terminal", "bash", "shell", "command", "exec", "run"]) {
        ToolKind::Exec
    } else if has(&["read", "search", "grep", "find", "list", "glob", "view", "open"]) {
        ToolKind::Read
    } else {
        ToolKind::Other
    }
}

/// Builds a `ToolUse` from a payload, given the keys this platform uses.
pub fn tool(raw: &Value, cwd: Option<&Path>, name_keys: &[&str], input_keys: &[&str]) -> Option<ToolUse> {
    let name = text(raw, name_keys)?;
    let args = input(raw, input_keys);
    let mut paths: Vec<String> = PATH_FIELDS
        .iter()
        .filter_map(|f| args.get(*f)?.as_str().map(str::to_string))
        .collect();
    for field in LIST_FIELDS {
        if let Some(Value::Array(items)) = args.get(field) {
            paths.extend(items.iter().filter_map(|v| v.as_str().map(str::to_string)));
        }
    }
    let command = text(&args, &["command", "cmd"]);
    // apply_patch carries the files inside the patch text.
    for field in ["patch", "input", "command"] {
        if let Some(body) = args.get(field).and_then(Value::as_str) {
            for line in body.lines() {
                for prefix in ["*** Add File: ", "*** Update File: ", "*** Delete File: "] {
                    if let Some(path) = line.strip_prefix(prefix) {
                        paths.push(path.trim().to_string());
                    }
                }
            }
        }
    }
    let mut kind = kind_of(&name);
    if kind == ToolKind::Exec && command.as_deref().is_some_and(|c| c.contains("*** Begin Patch")) {
        kind = ToolKind::Write;
    }
    let base = cwd.map(Path::to_path_buf).unwrap_or_default();
    let paths = paths
        .into_iter()
        .map(|p| p.strip_prefix("file://").map(str::to_string).unwrap_or(p))
        .map(|p| absolute(&base, &p))
        .collect();
    Some(ToolUse {
        name,
        kind,
        paths,
        command,
    })
}

fn absolute(base: &Path, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    let joined = if path.is_absolute() { path } else { base.join(path) };
    // Resolve symlinks (macOS /var → /private/var) so paths compare with the project root.
    std::fs::canonicalize(&joined).unwrap_or_else(|_| {
        match (
            joined.parent().and_then(|p| std::fs::canonicalize(p).ok()),
            joined.file_name(),
        ) {
            (Some(parent), Some(name)) => parent.join(name),
            _ => joined,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn kinds_by_name() {
        assert_eq!(kind_of("Edit"), ToolKind::Write);
        assert_eq!(kind_of("replace_string_in_file"), ToolKind::Write);
        assert_eq!(kind_of("run_in_terminal"), ToolKind::Exec);
        assert_eq!(kind_of("file_search"), ToolKind::Read);
        assert_eq!(kind_of("TodoWrite"), ToolKind::Write);
        assert_eq!(kind_of("WebFetch"), ToolKind::Other);
    }

    #[test]
    fn string_input_and_patch_files() {
        let raw = json!({"toolName": "apply_patch", "toolArgs": "{\"input\": \"*** Begin Patch\\n*** Update File: src/a.rs\\n\"}"});
        let tool = tool(&raw, Some(Path::new("/p")), &["toolName"], &["toolArgs"]).unwrap();
        assert_eq!(tool.kind, ToolKind::Write);
        assert_eq!(tool.paths, vec![PathBuf::from("/p/src/a.rs")]);
    }
}
