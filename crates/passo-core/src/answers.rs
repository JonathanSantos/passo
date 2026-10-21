//! The dev's decisions, from the CLI (`passo ok 1b 2=texto`) or typed in the agent's chat (`ok 1b`).

#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// 1-based question number in the gate; `None` for an unnumbered value (a single question, or a hint).
    pub index: Option<usize>,
    /// An option letter or the dev's own words.
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decision {
    pub answers: Vec<Answer>,
}

impl Decision {
    /// Unnumbered words, joined: a hint for a stuck run, or a free answer to a single question.
    pub fn note(&self) -> Option<String> {
        let text: Vec<&str> = self
            .answers
            .iter()
            .filter(|a| a.index.is_none())
            .map(|a| a.value.as_str())
            .collect();
        let text = text.join(" ");
        if text.trim().is_empty() {
            None
        } else {
            Some(text.trim().to_string())
        }
    }
}

/// `1b` → (1, "b"); `q2=texto` / `2:texto` / `2=` → (2, "texto"), the text may continue in the next words.
fn starter(word: &str) -> Option<(usize, String, bool)> {
    let rest = word.strip_prefix(['q', 'Q']).unwrap_or(word);
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let index: usize = digits.parse().ok()?;
    let tail = &rest[digits.len()..];
    if let Some(text) = tail.strip_prefix(['=', ':']) {
        return Some((index, text.to_string(), true));
    }
    if tail.len() == 1 && tail.chars().all(|c| c.is_ascii_alphabetic()) {
        return Some((index, tail.to_uppercase(), false));
    }
    None
}

/// Parses answers from words. Free text after `N=` runs until the next numbered answer.
pub fn parse_answers(words: &[String]) -> Decision {
    let mut answers: Vec<Answer> = vec![];
    let mut open_text = false;
    for word in words
        .iter()
        .flat_map(|w| w.split_whitespace().map(str::to_string).collect::<Vec<_>>())
    {
        if let Some((index, value, free)) = starter(&word) {
            answers.push(Answer {
                index: Some(index),
                value,
            });
            open_text = free;
            continue;
        }
        match answers.last_mut() {
            Some(last) if open_text || last.index.is_none() => {
                if !last.value.is_empty() {
                    last.value.push(' ');
                }
                last.value.push_str(&word);
            }
            _ => {
                answers.push(Answer {
                    index: None,
                    value: word,
                });
                open_text = false;
            }
        }
    }
    Decision { answers }
}

/// A decision the dev typed in the agent's chat. Only the platform's prompt hook calls this, so it is the dev's.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatDecision {
    Approve(Decision),
    Reject(String),
}

/// Recognizes `ok`, `ok 1b 2a`, `aprovo`, `não: motivo`, `no: motivo` at the start of a chat message.
/// Anything else is a normal message and stays with the agent.
pub fn parse_chat(message: &str) -> Option<ChatDecision> {
    let text = message.trim();
    let lower = text.to_lowercase();
    for word in ["ok", "aprovo", "aprovado", "pode seguir", "lgtm"] {
        if let Some(rest) = lower.strip_prefix(word)
            && (rest.is_empty() || rest.starts_with([' ', ',', '.', '!']))
        {
            let rest = text[word.len()..].trim_start_matches([',', '.', '!', ' ']);
            let words: Vec<String> = rest.split_whitespace().map(str::to_string).collect();
            let decision = parse_answers(&words);
            // "ok, mas antes troque X" is not an approval: unnumbered words mean the dev is still talking.
            if decision.answers.iter().any(|a| a.index.is_none()) {
                return None;
            }
            return Some(ChatDecision::Approve(decision));
        }
    }
    for word in ["não:", "nao:", "no:", "recuso:"] {
        if lower.starts_with(word) {
            let reason = text[word.len()..].trim();
            if !reason.is_empty() {
                return Some(ChatDecision::Reject(reason.to_string()));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split(' ').map(str::to_string).collect()
    }

    #[test]
    fn letters_and_free_text() {
        let d = parse_answers(&words("1b q2=usar o cache local 3:a"));
        assert_eq!(
            d.answers[0],
            Answer {
                index: Some(1),
                value: "B".into()
            }
        );
        assert_eq!(
            d.answers[1],
            Answer {
                index: Some(2),
                value: "usar o cache local".into()
            }
        );
        assert_eq!(
            d.answers[2],
            Answer {
                index: Some(3),
                value: "a".into()
            }
        );
    }

    #[test]
    fn unnumbered_is_a_note() {
        let d = parse_answers(&["tente sem o mock".to_string()]);
        assert_eq!(d.note().as_deref(), Some("tente sem o mock"));
    }

    #[test]
    fn chat_ok_variants() {
        assert_eq!(parse_chat("ok"), Some(ChatDecision::Approve(Decision::default())));
        assert!(matches!(parse_chat("OK 1b 2a"), Some(ChatDecision::Approve(d)) if d.answers.len() == 2));
        assert_eq!(
            parse_chat("não: falta o filtro"),
            Some(ChatDecision::Reject("falta o filtro".into()))
        );
        assert_eq!(parse_chat("ok, mas antes troque o nome"), None);
        assert_eq!(parse_chat("okay vamos ver"), None);
        assert_eq!(parse_chat("como assim?"), None);
    }
}
