//! A small POSIX shell lexer, written by hand, for the lock: which programs a command runs, with which words, and
//! which files it redirects to. Not a shell: what it cannot read comes back as `readable: false`, never a panic.
//!
//! Understood: single and double quotes, `\` escapes, the separators `;` `&&` `||` `|` `&` newline `(` `)`,
//! `$(…)` and backtick substitutions (each becomes a command of its own, read recursively), `${…}` and `$VAR`
//! (they make a word `dynamic`), redirects with or without a descriptor number, `#` comments, and heredocs
//! (`<<` and `<<-`, quoted delimiter or not), whose body is data. Programs that run another command
//! (`sh -c`, `eval`, `xargs`, `env`, `sudo`, …) add that command to the list too.

/// A command line, split into the simple commands it runs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Script {
    pub commands: Vec<SimpleCommand>,
    /// False when something was not understood (unbalanced quotes, an unclosed `$(`, a heredoc without its end).
    pub readable: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SimpleCommand {
    /// The program and its arguments, quotes removed.
    pub words: Vec<Word>,
    pub redirects: Vec<Redirect>,
    /// `VAR=x` before the program.
    pub assignments: Vec<(String, Word)>,
}

impl SimpleCommand {
    /// The program's name, without its path.
    pub fn program(&self) -> Option<&str> {
        self.words.first().map(|w| w.text.rsplit('/').next().unwrap_or(&w.text))
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Word {
    pub text: String,
    /// Had `$`, `~`, `*`, `?` or a substitution outside single quotes: its value is only known when it runs.
    pub dynamic: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Redirect {
    pub target: Word,
    pub write: bool,
}

/// How deep `sh -c`, `eval` and substitutions may nest before the command counts as unreadable.
const MAX_DEPTH: usize = 6;

/// Programs whose argument is another command line.
const SHELLS: [&str; 4] = ["sh", "bash", "zsh", "dash"];
/// Programs that run the command that follows them.
const PREFIXES: [&str; 9] = [
    "xargs", "env", "sudo", "nohup", "time", "command", "exec", "nice", "timeout",
];

pub fn lex(command: &str) -> Script {
    lex_at(command, 0)
}

fn lex_at(command: &str, depth: usize) -> Script {
    if depth > MAX_DEPTH {
        return Script {
            commands: vec![],
            readable: false,
        };
    }
    let mut lexer = Lexer {
        chars: command.chars().collect(),
        pos: 0,
        readable: true,
        subs: vec![],
        heredocs: vec![],
    };
    let tokens = lexer.tokens();
    let mut script = Script {
        commands: group(tokens),
        readable: lexer.readable,
    };
    for sub in std::mem::take(&mut lexer.subs) {
        let inner = lex_at(&sub, depth + 1);
        script.readable &= inner.readable;
        script.commands.extend(inner.commands);
    }
    let mut extra = vec![];
    for cmd in &script.commands {
        if let Some(inner) = unwrap(cmd, depth) {
            script.readable &= inner.readable;
            extra.extend(inner.commands);
        }
    }
    script.commands.extend(extra);
    script
}

/// The command a wrapper runs: `sh -c '…'`, `eval …`, `sudo …`, `xargs …`.
fn unwrap(cmd: &SimpleCommand, depth: usize) -> Option<Script> {
    let program = cmd.program()?;
    let rest = &cmd.words[1..];
    if SHELLS.contains(&program) {
        let at = rest.iter().position(|w| w.text == "-c")?;
        let inner = rest.get(at + 1)?;
        return Some(lex_at(&inner.text, depth + 1));
    }
    if program == "eval" {
        let text: Vec<&str> = rest.iter().map(|w| w.text.as_str()).collect();
        return Some(lex_at(&text.join(" "), depth + 1));
    }
    if PREFIXES.contains(&program) {
        // Skip options, numbers (`nice -n 10`, `timeout 5`) and `VAR=x`. An option may take an argument the lexer
        // cannot know about (`sudo -u x git push`), so the first two remaining words are both taken as the program:
        // an extra command only gives the lock more to look at.
        let candidates: Vec<usize> = rest
            .iter()
            .enumerate()
            .filter(|(_, w)| {
                let numeric = !w.text.is_empty() && w.text.chars().all(|c| c.is_ascii_digit() || c == '.');
                !w.text.starts_with('-') && !numeric && !is_assignment(&w.text)
            })
            .map(|(i, _)| i)
            .take(2)
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let mut script = Script {
            commands: vec![],
            readable: true,
        };
        for i in candidates {
            let inner = SimpleCommand {
                words: rest[i..].to_vec(),
                redirects: vec![],
                assignments: vec![],
            };
            if let Some(deeper) = unwrap(&inner, depth + 1) {
                script.readable &= deeper.readable;
                script.commands.extend(deeper.commands);
            }
            script.commands.insert(0, inner);
        }
        return Some(script);
    }
    None
}

fn is_assignment(text: &str) -> bool {
    match text.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit())
        }
        None => false,
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(Word, /* quoted name: never an assignment */ bool),
    Separator,
    Redirect {
        write: bool,
    },
    /// A redirect whose target is data, not a path (`<<<`, `>&2`).
    Discard,
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    readable: bool,
    /// Substitutions met on the way: their text, to be read as commands of their own.
    subs: Vec<String>,
    /// Heredocs waiting for the next newline: (delimiter, strip leading tabs).
    heredocs: Vec<(String, bool)>,
}

impl Lexer {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).copied()
    }

    fn tokens(&mut self) -> Vec<Token> {
        let mut out = vec![];
        while let Some(c) = self.peek(0) {
            match c {
                ' ' | '\t' => self.pos += 1,
                '#' => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.pos += 1;
                    }
                }
                '\n' => {
                    self.pos += 1;
                    out.push(Token::Separator);
                    self.read_heredocs();
                }
                ';' | '&' | '|' | '(' | ')' => {
                    if c == '&' && self.peek(1) == Some('>') {
                        self.pos += 2;
                        if self.peek(0) == Some('>') {
                            self.pos += 1;
                        }
                        out.push(Token::Redirect { write: true });
                        continue;
                    }
                    self.pos += 1;
                    if (c == '&' || c == '|') && self.peek(0) == Some(c) {
                        self.pos += 1;
                    }
                    out.push(Token::Separator);
                }
                '<' | '>' => out.push(self.redirect()),
                _ => {
                    let (word, quoted_name) = self.word();
                    // A descriptor number glued to a redirect: `2> err.txt`.
                    if matches!(self.peek(0), Some('<' | '>'))
                        && !word.text.is_empty()
                        && word.text.chars().all(|c| c.is_ascii_digit())
                        && !word.dynamic
                    {
                        continue;
                    }
                    out.push(Token::Word(word, quoted_name));
                }
            }
        }
        if !self.heredocs.is_empty() {
            self.readable = false;
        }
        out
    }

    fn redirect(&mut self) -> Token {
        let c = self.peek(0).unwrap_or('>');
        self.pos += 1;
        if c == '<' {
            if self.peek(0) == Some('<') {
                self.pos += 1;
                if self.peek(0) == Some('<') {
                    // Here-string: the next word is data.
                    self.pos += 1;
                    self.skip_blanks();
                    self.word();
                    return Token::Discard;
                }
                let strip = self.peek(0) == Some('-');
                if strip {
                    self.pos += 1;
                }
                self.skip_blanks();
                let (delimiter, _) = self.word();
                if delimiter.text.is_empty() {
                    self.readable = false;
                } else {
                    self.heredocs.push((delimiter.text, strip));
                }
                return Token::Discard;
            }
            if self.peek(0) == Some('&') {
                self.pos += 1;
                self.skip_blanks();
                self.word();
                return Token::Discard;
            }
            return Token::Redirect { write: false };
        }
        if self.peek(0) == Some('>') || self.peek(0) == Some('|') {
            self.pos += 1;
        }
        if self.peek(0) == Some('&') {
            // `>&2`, `2>&1`: a descriptor, not a file.
            self.pos += 1;
            self.skip_blanks();
            self.word();
            return Token::Discard;
        }
        Token::Redirect { write: true }
    }

    fn skip_blanks(&mut self) {
        while matches!(self.peek(0), Some(' ' | '\t')) {
            self.pos += 1;
        }
    }

    /// Consumes heredoc bodies after a newline, up to each delimiter line.
    fn read_heredocs(&mut self) {
        for (delimiter, strip) in std::mem::take(&mut self.heredocs) {
            let mut found = false;
            while self.pos < self.chars.len() {
                let start = self.pos;
                while self.peek(0).is_some_and(|c| c != '\n') {
                    self.pos += 1;
                }
                let line: String = self.chars[start..self.pos].iter().collect();
                if self.peek(0) == Some('\n') {
                    self.pos += 1;
                }
                let line = if strip {
                    line.trim_start_matches('\t')
                } else {
                    line.as_str()
                };
                if line == delimiter {
                    found = true;
                    break;
                }
            }
            if !found {
                self.readable = false;
            }
        }
    }

    /// One word, quotes removed. The flag says the word started quoted (so `"A"=b` is not an assignment).
    fn word(&mut self) -> (Word, bool) {
        let mut word = Word::default();
        let quoted_name = matches!(self.peek(0), Some('\'' | '"'));
        let at_start = |w: &Word| w.text.is_empty();
        while let Some(c) = self.peek(0) {
            match c {
                ' ' | '\t' | '\n' | ';' | '&' | '|' | '(' | ')' | '<' | '>' => break,
                '\'' => {
                    self.pos += 1;
                    let mut closed = false;
                    while let Some(c) = self.peek(0) {
                        self.pos += 1;
                        if c == '\'' {
                            closed = true;
                            break;
                        }
                        word.text.push(c);
                    }
                    if !closed {
                        self.readable = false;
                    }
                }
                '"' => {
                    self.pos += 1;
                    let mut closed = false;
                    while let Some(c) = self.peek(0) {
                        match c {
                            '"' => {
                                self.pos += 1;
                                closed = true;
                                break;
                            }
                            '\\' => {
                                self.pos += 1;
                                if let Some(next) = self.peek(0) {
                                    self.pos += 1;
                                    if !matches!(next, '"' | '\\' | '$' | '`' | '\n') {
                                        word.text.push('\\');
                                    }
                                    if next != '\n' {
                                        word.text.push(next);
                                    }
                                }
                            }
                            '$' => self.dollar(&mut word),
                            '`' => self.backtick(&mut word),
                            _ => {
                                self.pos += 1;
                                word.text.push(c);
                            }
                        }
                    }
                    if !closed {
                        self.readable = false;
                    }
                }
                '\\' => {
                    self.pos += 1;
                    match self.peek(0) {
                        Some('\n') => self.pos += 1,
                        Some(next) => {
                            self.pos += 1;
                            word.text.push(next);
                        }
                        None => {}
                    }
                }
                '$' => self.dollar(&mut word),
                '`' => self.backtick(&mut word),
                '~' if at_start(&word) => {
                    self.pos += 1;
                    word.text.push(c);
                    word.dynamic = true;
                }
                '*' | '?' | '[' => {
                    self.pos += 1;
                    word.text.push(c);
                    word.dynamic = true;
                }
                _ => {
                    self.pos += 1;
                    word.text.push(c);
                }
            }
        }
        (word, quoted_name)
    }

    /// `$(…)`, `$((…))`, `${…}`, `$NAME`, `$?`: all make the word dynamic; a command substitution is kept to read.
    fn dollar(&mut self, word: &mut Word) {
        self.pos += 1;
        word.dynamic = true;
        word.text.push('$');
        match self.peek(0) {
            Some('(') => {
                self.pos += 1;
                match self.balanced(')') {
                    Some(inner) => self
                        .subs
                        .push(inner.trim_start_matches('(').trim_end_matches(')').to_string()),
                    None => self.readable = false,
                }
            }
            Some('{') => {
                self.pos += 1;
                if self.balanced('}').is_none() {
                    self.readable = false;
                }
            }
            Some(c) if c.is_ascii_alphanumeric() || c == '_' => {
                while self.peek(0).is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                    self.pos += 1;
                }
            }
            Some('?' | '#' | '@' | '*' | '!' | '$' | '-') => self.pos += 1,
            _ => {}
        }
    }

    fn backtick(&mut self, word: &mut Word) {
        self.pos += 1;
        word.dynamic = true;
        let mut inner = String::new();
        let mut closed = false;
        while let Some(c) = self.peek(0) {
            self.pos += 1;
            match c {
                '`' => {
                    closed = true;
                    break;
                }
                '\\' => {
                    if let Some(next) = self.peek(0) {
                        self.pos += 1;
                        inner.push(next);
                    }
                }
                _ => inner.push(c),
            }
        }
        if closed {
            self.subs.push(inner);
        } else {
            self.readable = false;
        }
    }

    /// Reads up to the matching `close`, minding nested pairs and quotes. `None` when it never closes.
    fn balanced(&mut self, close: char) -> Option<String> {
        let open = if close == ')' { '(' } else { '{' };
        let mut depth = 1;
        let mut text = String::new();
        while let Some(c) = self.peek(0) {
            self.pos += 1;
            match c {
                '\'' | '"' => {
                    text.push(c);
                    while let Some(q) = self.peek(0) {
                        self.pos += 1;
                        text.push(q);
                        if q == '\\' && c == '"' {
                            if let Some(next) = self.peek(0) {
                                self.pos += 1;
                                text.push(next);
                            }
                            continue;
                        }
                        if q == c {
                            break;
                        }
                    }
                }
                '\\' => {
                    text.push(c);
                    if let Some(next) = self.peek(0) {
                        self.pos += 1;
                        text.push(next);
                    }
                }
                _ if c == open => {
                    depth += 1;
                    text.push(c);
                }
                _ if c == close => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(text);
                    }
                    text.push(c);
                }
                _ => text.push(c),
            }
        }
        None
    }
}

/// Groups tokens into simple commands: words and assignments, with the redirects that follow them.
fn group(tokens: Vec<Token>) -> Vec<SimpleCommand> {
    let mut out = vec![];
    let mut current = SimpleCommand::default();
    let mut pending: Option<bool> = None;
    let mut discard = false;
    let flush = |current: &mut SimpleCommand, out: &mut Vec<SimpleCommand>| {
        let cmd = std::mem::take(current);
        if !cmd.words.is_empty() || !cmd.redirects.is_empty() || !cmd.assignments.is_empty() {
            out.push(cmd);
        }
    };
    for token in tokens {
        match token {
            Token::Separator => {
                pending = None;
                discard = false;
                flush(&mut current, &mut out);
            }
            Token::Redirect { write } => pending = Some(write),
            Token::Discard => discard = true,
            Token::Word(word, quoted_name) => {
                if let Some(write) = pending.take() {
                    current.redirects.push(Redirect { target: word, write });
                } else if std::mem::take(&mut discard) {
                    // A heredoc delimiter or a descriptor was already consumed; any word here is an argument.
                    current.words.push(word);
                } else if current.words.is_empty() && !quoted_name && is_assignment(&word.text) {
                    let (name, value) = word.text.split_once('=').unwrap_or_default();
                    let value = Word {
                        text: value.to_string(),
                        dynamic: word.dynamic,
                    };
                    current.assignments.push((name.to_string(), value));
                } else {
                    current.words.push(word);
                }
            }
        }
    }
    flush(&mut current, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn programs(command: &str) -> Vec<String> {
        lex(command)
            .commands
            .iter()
            .filter_map(|c| c.program().map(str::to_string))
            .collect()
    }

    #[test]
    fn quotes_escapes_and_separators() {
        let s = lex(r#"cd "my dir" && echo 'a;b' | grep x; ls \; done"#);
        assert!(s.readable);
        assert_eq!(
            programs(r#"cd "my dir" && echo 'a;b' | grep x; ls"#),
            vec!["cd", "echo", "grep", "ls"]
        );
        assert_eq!(s.commands[0].words[1].text, "my dir");
        assert_eq!(s.commands[1].words[1].text, "a;b");
        assert_eq!(s.commands[3].words[1].text, ";");
    }

    #[test]
    fn substitutions_and_wrappers_become_commands() {
        assert!(programs("echo $(passo ok)").contains(&"passo".to_string()));
        assert!(programs("x=`git commit -m a`").contains(&"git".to_string()));
        assert!(programs("sh -c 'passo stop'").contains(&"passo".to_string()));
        assert!(programs("eval \"passo ok\"").contains(&"passo".to_string()));
        assert!(programs("xargs passo ok").contains(&"passo".to_string()));
        assert!(programs("sudo -u x env A=1 nice -n 10 git push").contains(&"git".to_string()));
        assert_eq!(lex("\"passo\" ok").commands[0].program(), Some("passo"));
    }

    #[test]
    fn dynamic_words_redirects_and_assignments() {
        let s = lex("p=passo; $p ok");
        assert_eq!(s.commands[0].assignments[0].0, "p");
        assert!(s.commands[1].words[0].dynamic);
        let s = lex("echo x > src/a.rs 2>/dev/null >&2");
        assert_eq!(s.commands[0].redirects.len(), 2);
        assert_eq!(s.commands[0].redirects[0].target.text, "src/a.rs");
        assert!(s.commands[0].redirects[0].write);
        assert_eq!(s.commands[0].words.len(), 2, "the descriptor number is not a word");
    }

    #[test]
    fn heredocs_are_data() {
        let s = lex("passo done - <<'EOF'\n{\"passo\": \"ok; rm -rf /\"}\nEOF\n");
        assert!(s.readable);
        assert_eq!(programs("passo done - <<'EOF'\n{\"a\": 1}\nEOF"), vec!["passo"]);
        let s = lex("cat > \"$(mktemp)\" <<EOF\n{ \"x\": \"$HOME\" }\nEOF");
        assert!(s.readable);
        assert_eq!(s.commands[0].program(), Some("cat"));
        assert!(!lex("cat <<EOF\nnunca fecha").readable);
    }

    #[test]
    fn what_cannot_be_read_says_so() {
        for broken in [
            "echo 'sem fim",
            "echo \"sem fim",
            "echo $(passo ok",
            "echo `x",
            "cat <<",
        ] {
            assert!(!lex(broken).readable, "{broken}");
        }
    }

    #[test]
    fn a_malformed_corpus_never_panics() {
        // Fixed seed: the same 200 strings every run.
        let alphabet: Vec<char> = "ab $()`'\"\\;|&<>#\n{}-=*~passogit".chars().collect();
        let mut seed: u64 = 0x5eed;
        for _ in 0..200 {
            let mut text = String::new();
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let len = (seed >> 33) % 40;
            for _ in 0..len {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                text.push(alphabet[((seed >> 33) as usize) % alphabet.len()]);
            }
            let _ = lex(&text);
        }
    }
}
