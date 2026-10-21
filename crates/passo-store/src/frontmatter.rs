//! A tiny frontmatter reader: `key: value` and `key: [a, b]` between `---` lines. Enough for agent, stage and
//! checklist files, and readable by anyone who has seen a Claude Code agent file.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    Text(String),
    List(Vec<String>),
}

#[derive(Debug, Clone, Default)]
pub struct Doc {
    pub fields: BTreeMap<String, Field>,
    pub body: String,
}

impl Doc {
    pub fn text(&self, key: &str) -> Option<String> {
        match self.fields.get(key)? {
            Field::Text(t) => Some(t.clone()),
            Field::List(items) => Some(items.join(", ")),
        }
    }

    pub fn list(&self, key: &str) -> Vec<String> {
        match self.fields.get(key) {
            Some(Field::List(items)) => items.clone(),
            Some(Field::Text(t)) if !t.is_empty() => t.split(',').map(|s| s.trim().to_string()).collect(),
            _ => vec![],
        }
    }
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\''))) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

pub fn parse(text: &str) -> Doc {
    let text = text.trim_start_matches('\u{feff}');
    let Some(rest) = text.strip_prefix("---") else {
        return Doc {
            fields: BTreeMap::new(),
            body: text.trim().to_string(),
        };
    };
    let Some(end) = rest.find("\n---") else {
        return Doc {
            fields: BTreeMap::new(),
            body: text.trim().to_string(),
        };
    };
    let head = &rest[..end];
    let body = rest[end + 4..].trim_start_matches(['-']).trim().to_string();
    let mut fields = BTreeMap::new();
    for line in head.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_string();
        let value = value.trim();
        if key.is_empty() || key.starts_with('#') {
            continue;
        }
        let field = if value.starts_with('[') && value.ends_with(']') {
            Field::List(
                value[1..value.len() - 1]
                    .split(',')
                    .map(unquote)
                    .filter(|s| !s.is_empty())
                    .collect(),
            )
        } else {
            Field::Text(unquote(value))
        };
        fields.insert(key, field);
    }
    Doc { fields, body }
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
        );
        assert_eq!(doc.text("name").as_deref(), Some("construtor"));
        assert_eq!(doc.list("tools"), vec!["Read", "Edit"]);
        let s = sections(&doc.body);
        assert_eq!(s["todos"], "intro");
        assert_eq!(s["pp"], "curto");
        assert_eq!(s["m"], "longo");
    }
}
