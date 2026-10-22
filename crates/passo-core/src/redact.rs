//! Hides secrets in text that is about to be kept (the log, the run, the report). No regex: a few plain rules, by
//! word and by line, that leave everything else alone (file names, commands, small numbers).
//!
//! - A key that ends in `token`, `secret`, `password`… followed by `=` or `:` gets its value hidden
//!   (`TOKEN=abc`, `password: abc`, `"password": "abc"`, `Authorization: Bearer abc`). Followed by a space only as
//!   an option (`--token abc`). A common word followed by a space is never a key: `token de sessão` stays. A short
//!   number is never a secret (`session: 2`, `src/auth: 3 arquivos`); `pwd` and `passwd` count only as a whole key,
//!   and the shell's `PWD` and `OLDPWD` never.
//! - A few programs take a password in a short option: `mysql -p…`, `sshpass -p`, `psql -W`, `curl -u user:…`,
//!   `openssl -pass`. Outside that table, `-x value` is not a key.
//! - Words with a known token prefix (`ghp_`, `sk-`, `AKIA…`, `AIza`, PEM blocks) are hidden whole; `sk-` only with a
//!   token's length after it (`sk-learn-extra` stays).
//! - A `.env`-style line (`NAME_IN_CAPS=value`) with a value of 16 characters or more gets its value hidden.

pub const HIDDEN: &str = "«oculto»";

const KEYS: [&str; 14] = [
    "token",
    "secret",
    "password",
    "api_key",
    "apikey",
    "api-key",
    "auth",
    "authorization",
    "credential",
    "credentials",
    "cookie",
    "session",
    "private_key",
    "access_key",
];

/// Header-like keys whose value may be a scheme and a credential: `Authorization: Bearer abc`.
const SCHEMES: [&str; 4] = ["bearer", "basic", "token", "digest"];

const PREFIXES: [&str; 5] = ["ghp_", "gho_", "github_pat_", "AIza", "glpat-"];
/// Prefixes that are also common words in names (`sk-learn`): they need a token after them.
const TOKEN_PREFIXES: [&str; 3] = ["sk-", "sk_live_", "sk_test_"];

/// Programs that take a password in a short option: (program, option, the value is the part after `:`).
const SHORT_OPTIONS: [(&str, &str, bool); 7] = [
    ("mysql", "-p", false),
    ("mariadb", "-p", false),
    ("sshpass", "-p", false),
    ("psql", "-W", false),
    ("curl", "-u", true),
    ("wget", "-u", true),
    ("openssl", "-pass", false),
];

pub fn redact(text: &str) -> String {
    redact_with(text, &[])
}

/// `redact`, with extra words treated as keys (`audit.secrets` in `passo.yaml`).
pub fn redact_with(text: &str, extra: &[String]) -> String {
    let text = hide_pem(text);
    let lines: Vec<String> = text.split('\n').map(|line| redact_line(line, extra)).collect();
    lines.join("\n")
}

fn is_key(word: &str, extra: &[String]) -> bool {
    let bare = word.trim_start_matches('-');
    if bare == "PWD" || bare == "OLDPWD" {
        return false;
    }
    let word = bare.to_lowercase();
    !word.is_empty()
        && (word == "pwd"
            || word == "passwd"
            || KEYS.iter().any(|k| word.ends_with(k))
            || extra.iter().any(|k| word.ends_with(&k.to_lowercase())))
}

/// A value worth hiding: 8 characters or more, or anything that is not a plain number (`2`, `3.5`).
fn secret_value(chars: &[char]) -> bool {
    chars.len() >= 8 || chars.iter().any(|c| !c.is_ascii_digit() && *c != '.')
}

fn hide_pem(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("-----BEGIN") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        // The block ends at the dashes that close the `-----END …-----` line.
        match after
            .find("-----END")
            .and_then(|end| after[end + 8..].find("-----").map(|close| end + 8 + close + 5))
        {
            Some(end) => {
                out.push_str(HIDDEN);
                rest = &after[end..];
            }
            None => {
                out.push_str(HIDDEN);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.')
}

fn redact_line(line: &str, extra: &[String]) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut hide: Vec<(usize, usize)> = vec![];

    // `.env` lines: NAME_IN_CAPS=long value.
    let trimmed_start = chars.iter().take_while(|c| c.is_whitespace()).count();
    if let Some(eq) = chars[trimmed_start..].iter().position(|c| *c == '=') {
        let name: String = chars[trimmed_start..trimmed_start + eq].iter().collect();
        let value_start = trimmed_start + eq + 1;
        let value_len = chars.len() - value_start;
        if !name.is_empty()
            && name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            && value_len >= 16
            && !chars[value_start..].iter().any(|c| c.is_whitespace())
        {
            hide.push((value_start, chars.len()));
        }
    }

    // key=value and key: value.
    for (i, &c) in chars.iter().enumerate() {
        if c != '=' && c != ':' {
            continue;
        }
        if c == ':'
            && (chars.get(i + 1) == Some(&'/') || chars.get(i + 1) == Some(&':') || (i > 0 && chars[i - 1] == ':'))
        {
            continue;
        }
        // The key just left of the separator, maybe inside quotes.
        let mut end = i;
        while end > 0 && chars[end - 1] == ' ' {
            end -= 1;
        }
        if end > 0 && matches!(chars[end - 1], '"' | '\'') {
            end -= 1;
        }
        let mut start = end;
        while start > 0 && ident_char(chars[start - 1]) {
            start -= 1;
        }
        let key: String = chars[start..end].iter().collect();
        if !is_key(&key, extra) {
            continue;
        }
        // The value: quoted, or up to the next blank or delimiter.
        let mut v = i + 1;
        while v < chars.len() && chars[v] == ' ' {
            v += 1;
        }
        if v >= chars.len() {
            continue;
        }
        let (value_start, value_end) = if matches!(chars[v], '"' | '\'') {
            let quote = chars[v];
            let close = chars[v + 1..]
                .iter()
                .position(|c| *c == quote)
                .map(|p| v + 1 + p)
                .unwrap_or(chars.len());
            (v + 1, close)
        } else {
            let stop = |c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | '&' | '|' | ')' | '}');
            let mut e = v;
            while e < chars.len() && !stop(chars[e]) {
                e += 1;
            }
            let word: String = chars[v..e].iter().collect();
            // `Authorization: Bearer abc`: the scheme and the credential are one value.
            if c == ':' && SCHEMES.contains(&word.to_lowercase().as_str()) && e < chars.len() && chars[e] == ' ' {
                let mut e2 = e + 1;
                while e2 < chars.len() && !stop(chars[e2]) {
                    e2 += 1;
                }
                (v, e2)
            } else {
                (v, e)
            }
        };
        if value_end > value_start && secret_value(&chars[value_start..value_end]) {
            hide.push((value_start, value_end));
        }
    }

    let words = word_spans(&chars);
    // A short option from the table, after its program: `mysql -phunter2`, `sshpass -p x`, `curl -u ana:x`.
    let text = |(s, e): (usize, usize)| chars[s..e].iter().collect::<String>();
    let mut program: Option<&str> = None;
    for (i, &span) in words.iter().enumerate() {
        let word = text(span);
        let name = word.rsplit('/').next().unwrap_or(&word);
        if let Some((p, _, _)) = SHORT_OPTIONS.iter().find(|(p, _, _)| *p == name) {
            program = Some(p);
            continue;
        }
        let Some(program) = program else { continue };
        for (_, option, after_colon) in SHORT_OPTIONS.iter().filter(|(p, _, _)| *p == program) {
            let (s, e) = if word == *option {
                match words.get(i + 1) {
                    Some(&next) => next,
                    None => continue,
                }
            } else if word.len() > option.len() && word.starts_with(option) && option.len() == 2 {
                (span.0 + option.chars().count(), span.1)
            } else {
                continue;
            };
            let s = if *after_colon {
                match chars[s..e].iter().position(|c| *c == ':') {
                    Some(at) => s + at + 1,
                    None => continue,
                }
            } else {
                s
            };
            if e > s {
                hide.push((s, e));
            }
        }
    }

    // --option value.
    for pair in words.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let option: String = chars[a.0..a.1].iter().collect();
        let value: String = chars[b.0..b.1].iter().collect();
        if option.starts_with("--") && !option.contains('=') && is_key(&option, extra) && !value.starts_with('-') {
            let (s, e) = strip_quotes(&chars, b);
            hide.push((s, e));
        }
    }

    // Known token prefixes, word by word.
    for (s, e) in word_spans_by(&chars, |c| {
        c.is_whitespace() || matches!(c, '"' | '\'' | '=' | ':' | ',' | '(' | ')')
    }) {
        let word: String = chars[s..e].iter().collect();
        let token_after = |p: &str| {
            word.strip_prefix(p).is_some_and(|rest| {
                rest.len() >= 20
                    && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                    && rest.chars().any(|c| c.is_ascii_digit())
            })
        };
        let prefixed = PREFIXES.iter().any(|p| word.starts_with(p) && word.len() > p.len() + 6)
            || TOKEN_PREFIXES.iter().any(|p| token_after(p))
            || is_slack(&word)
            || is_aws_key(&word);
        if prefixed {
            hide.push((s, e));
        }
    }

    apply(&chars, hide)
}

fn is_slack(word: &str) -> bool {
    let bytes = word.as_bytes();
    bytes.len() > 10
        && word.starts_with("xox")
        && matches!(bytes[3], b'a' | b'b' | b'p' | b'r' | b's')
        && bytes[4] == b'-'
}

fn is_aws_key(word: &str) -> bool {
    word.len() == 20
        && word.starts_with("AKIA")
        && word[4..].chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

fn word_spans(chars: &[char]) -> Vec<(usize, usize)> {
    word_spans_by(chars, char::is_whitespace)
}

fn word_spans_by(chars: &[char], split: impl Fn(char) -> bool) -> Vec<(usize, usize)> {
    let mut out = vec![];
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && split(chars[i]) {
            i += 1;
        }
        let start = i;
        while i < chars.len() && !split(chars[i]) {
            i += 1;
        }
        if i > start {
            out.push((start, i));
        }
    }
    out
}

fn strip_quotes(chars: &[char], (s, e): (usize, usize)) -> (usize, usize) {
    if e - s >= 2 && matches!(chars[s], '"' | '\'') && chars[e - 1] == chars[s] {
        (s + 1, e - 1)
    } else {
        (s, e)
    }
}

/// Replaces the ranges (merged when they overlap) with the marker.
fn apply(chars: &[char], mut hide: Vec<(usize, usize)>) -> String {
    if hide.is_empty() {
        return chars.iter().collect();
    }
    hide.sort();
    let mut merged: Vec<(usize, usize)> = vec![];
    for (s, e) in hide {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    let mut out = String::new();
    let mut at = 0;
    for (s, e) in merged {
        out.extend(&chars[at..s]);
        out.push_str(HIDDEN);
        at = e;
    }
    out.extend(&chars[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_hidden() {
        let cases = [
            ("export TOKEN=abc123", "export TOKEN=«oculto»"),
            ("password: hunter2", "password: «oculto»"),
            (
                r#"{"password": "hunter2", "user": "ana"}"#,
                r#"{"password": "«oculto»", "user": "ana"}"#,
            ),
            (
                r#"curl -H "Authorization: Bearer abc123def456" https://api.x/y"#,
                r#"curl -H "Authorization: «oculto»" https://api.x/y"#,
            ),
            ("deploy --token abc123 --verbose", "deploy --token «oculto» --verbose"),
            ("cli --api-key=xyz789", "cli --api-key=«oculto»"),
            ("usei ghp_0123456789abcdefghij no push", "usei «oculto» no push"),
            ("key AKIAABCDEFGHIJKLMNOP aqui", "key «oculto» aqui"),
            ("STRIPE=sk_live_0123456789abcdef0123", "STRIPE=«oculto»"),
            ("mysql -u ana -phunter2 db", "mysql -u ana -p«oculto» db"),
            ("mysql -p hunter2", "mysql -p «oculto»"),
            ("sshpass -p hunter2 ssh x", "sshpass -p «oculto» ssh x"),
            ("curl -u ana:hunter2 https://x", "curl -u ana:«oculto» https://x"),
            ("openssl enc -pass pass:hunter2", "openssl enc -pass «oculto»"),
            ("chave sk-proj-a1b2c3d4e5f6g7h8i9j0k1 aqui", "chave «oculto» aqui"),
            ("passwd: hunter2", "passwd: «oculto»"),
            ("API_KEY=0123456789abcdef0123", "API_KEY=«oculto»"),
            ("DATABASE_URL=postgres://u:p@host:5432/db", "DATABASE_URL=«oculto»"),
            (
                "a\n-----BEGIN RSA PRIVATE KEY-----\nMIIEow\n-----END RSA PRIVATE KEY-----\nb",
                "a\n«oculto»\nb",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(redact(input), expected, "{input}");
        }
    }

    #[test]
    fn ordinary_text_stays() {
        for text in [
            "passo done \"token de sessão\"",
            "src/auth/login.rs",
            "AUTH_MODE=oidc",
            "https://github.com/x/y",
            "cargo test task::",
            "ok 12:30 started",
            "tests/x_test.rs:12 `prova_tal`; src/x.rs:42",
            "session: 2",
            "src/auth: 3 arquivos",
            "PWD=/Users/x OLDPWD=/tmp",
            "pip install sk-learn-extra",
            "grep -p padrão arquivo",
        ] {
            assert_eq!(redact(text), text, "{text}");
        }
    }

    #[test]
    fn hiding_twice_changes_nothing() {
        for text in [
            "TOKEN=abc123",
            "mysql -phunter2",
            "curl -u ana:x",
            "chave sk-proj-a1b2c3d4e5f6g7h8i9j0k1",
        ] {
            let once = redact(text);
            assert_eq!(redact(&once), once, "{text}");
        }
    }

    #[test]
    fn extra_keys_from_the_config() {
        assert_eq!(
            redact_with("minha_chave=abc", &["minha_chave".into()]),
            "minha_chave=«oculto»"
        );
        assert_eq!(redact("minha_chave=abc"), "minha_chave=abc");
    }
}
