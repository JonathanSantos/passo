//! Markdown files with a YAML frontmatter between `---` lines: agents, stages and checklists. The frontmatter is
//! real YAML (block lists, quoted text with colons, maps), read by the same parser as `passo.yaml`.

use std::collections::BTreeMap;

use serde_yaml_ng::{Mapping, Value};

#[derive(Debug, Clone, Default)]
pub struct Doc {
    pub fields: Mapping,
    pub body: String,
}

impl Doc {
    fn get(&self, key: &str) -> Option<&Value> {
        self.fields.get(key)
    }

    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Field names, in file order.
    pub fn keys(&self) -> Vec<String> {
        self.fields.keys().map(scalar).collect()
    }

    /// A field as text; a list is joined with commas.
    pub fn text(&self, key: &str) -> Option<String> {
        match self.get(key)? {
            Value::Null => None,
            Value::Sequence(items) => Some(items.iter().map(scalar).collect::<Vec<_>>().join(", ")),
            other => Some(scalar(other)),
        }
    }

    /// A field as a list; a single text is split on commas.
    pub fn list(&self, key: &str) -> Vec<String> {
        match self.get(key) {
            Some(Value::Sequence(items)) => items.iter().map(scalar).filter(|s| !s.is_empty()).collect(),
            Some(Value::Null) | None => vec![],
            Some(other) => scalar(other)
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        }
    }
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.trim().to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => String::new(),
        other => serde_yaml_ng::to_string(other).unwrap_or_default().trim().to_string(),
    }
}

/// Splits frontmatter from body. An error names the line in the file (the opening `---` is line 1).
pub fn parse(text: &str) -> Result<Doc, String> {
    let text = text.trim_start_matches('\u{feff}');
    let plain = || Doc {
        fields: Mapping::new(),
        body: text.trim().to_string(),
    };
    let Some(rest) = text.strip_prefix("---") else {
        return Ok(plain());
    };
    let Some(end) = rest.find("\n---") else {
        return Ok(plain());
    };
    let head = &rest[..end];
    let body = rest[end + 4..].trim_start_matches('-').trim().to_string();
    let fields = match serde_yaml_ng::from_str::<Value>(head) {
        Ok(Value::Mapping(map)) => map,
        Ok(Value::Null) => Mapping::new(),
        Ok(_) => return Err("o frontmatter deve ser um mapa de `chave: valor`".into()),
        Err(e) => {
            // `head` starts right after the opening `---`, on the same line.
            let line = e.location().map(|l| l.line()).unwrap_or(1);
            return Err(format!("frontmatter inválido na linha {line}: {e}"));
        }
    };
    Ok(Doc { fields, body })
}

/// Splits a body by `## heading` into sections; text before the first heading goes under `todos`.
pub fn sections(body: &str) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut key = "todos".to_string();
    let mut buf = String::new();
    for line in body.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            push(&mut out, &key, &buf);
            key = heading.trim().to_lowercase();
            buf.clear();
        } else {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    push(&mut out, &key, &buf);
    out
}

fn push(out: &mut BTreeMap<String, String>, key: &str, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let entry = out.entry(key.to_string()).or_default();
    if !entry.is_empty() {
        entry.push_str("\n\n");
    }
    entry.push_str(text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_fields_and_sections() {
        let doc = parse(
            "---\nname: construtor\ntools: [Read, \"Edit\"]\nmodel: leve\n---\nintro\n## pp\ncurto\n## M\nlongo\n",
        )
        .unwrap();
        assert_eq!(doc.text("name").as_deref(), Some("construtor"));
        assert_eq!(doc.list("tools"), vec!["Read", "Edit"]);
        let s = sections(&doc.body);
        assert_eq!(s["todos"], "intro");
        assert_eq!(s["pp"], "curto");
        assert_eq!(s["m"], "longo");
    }

    #[test]
    fn block_lists_and_quoted_colons() {
        let doc = parse("---\ndescription: \"Faz: um passo\"\nchecks:\n  - cargo clippy\n  - cargo test\n---\ncorpo")
            .unwrap();
        assert_eq!(doc.text("description").as_deref(), Some("Faz: um passo"));
        assert_eq!(doc.list("checks"), vec!["cargo clippy", "cargo test"]);
        assert_eq!(doc.body, "corpo");
    }

    #[test]
    fn no_frontmatter_or_empty_one() {
        let doc = parse("# só corpo").unwrap();
        assert!(doc.keys().is_empty());
        assert_eq!(doc.body, "# só corpo");
        let empty = parse("---\n---\ncorpo").unwrap();
        assert!(empty.keys().is_empty());
        assert_eq!(empty.body, "corpo");
    }

    #[test]
    fn invalid_yaml_names_the_line() {
        let err = parse("---\nname: x\nmodel: a: b\n---\n").unwrap_err();
        assert!(err.contains("linha 3"), "{err}");
    }
}
