//! A small POSIX shell lexer, written by hand, for the lock: which programs a command runs, with which words, and
//! which files it redirects to. Not a shell: what it cannot read comes back as `readable: false`, never a panic.
//!
//! Understood: single and double quotes, `\` escapes, the separators `;` `&&` `||` `|` `&` newline `(` `)`,
//! `$(…)` and backtick substitutions (each becomes a command of its own, read recursively), `${…}`, `$VAR` and
//! brace expansion (they make a word `dynamic`), redirects with or without a descriptor number, `#` comments,
//! heredocs (`<<` and `<<-`, quoted delimiter or not), whose body is data, and reserved words (`if`, `then`, `{`,
//! `!`, …), which only start a new command. `sh -c`, `eval` and `env -S` have their script read as commands too; a
//! shell without `-c` reads a script the lexer cannot see, so the line is unreadable.
//!
//! The lexer does not follow prefixes like `sudo` or `xargs`: the lock looks for `passo` and `git` in every word of
//! every command instead. One pass over the text, with ceilings on size, commands and words: anything past them is
//! unreadable, never slow.

/// A command line, split into the simple commands it runs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Script {
    pub commands: Vec<SimpleCommand>,
    /// False when something was not understood (unbalanced quotes, an unclosed `$(`, a heredoc without its end).
    pub readable: bool,
    /// Why it is unreadable, in the words the agent reads: the first cause found.
    pub why: Option<&'static str>,
}

impl Script {
    fn unreadable(&mut self, why: &'static str) {
        self.readable = false;
        self.why.get_or_insert(why);
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SimpleCommand {
    /// The program and its arguments, quotes removed.
    pub words: Vec<Word>,
    pub redirects: Vec<Redirect>,
    /// `VAR=x` before the program.
    pub assignments: Vec<(String, Word)>,
    /// Where it runs after the `cd`s before it in the line, relative to where the line starts (or absolute).
    pub dir: Option<String>,
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
    /// Had `$`, `~`, `*`, `?`, a brace expansion or a substitution outside single quotes: its value is only known
    /// when it runs.
    pub dynamic: bool,
    /// The commands of the substitutions in it, in order (`$(mktemp)` → `mktemp`).
    pub subs: Vec<String>,
    /// The variables it reads (`"$f"` → `f`).
    pub vars: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Redirect {
    pub target: Word,
    pub write: bool,
}

/// How deep `sh -c`, `eval` and substitutions may nest before the command counts as unreadable.
const MAX_DEPTH: usize = 6;
/// Ceilings: past any of them the line is unreadable.
pub const MAX_BYTES: usize = 16 * 1024;
pub const MAX_COMMANDS: usize = 128;
pub const MAX_WORDS: usize = 2048;

/// Programs whose `-c` argument is another command line; without `-c` they read a script the lexer cannot see.
pub const SHELLS: [&str; 6] = ["sh", "bash", "zsh", "dash", "ksh", "fish"];
/// Words that only start a command (`if x; then passo ok; fi`): at the start of a command they are skipped.
const RESERVED: [&str; 19] = [
    "{", "}", "!", "if", "then", "elif", "else", "fi", "for", "in", "do", "done", "while", "until", "case", "esac",
    "function", "select", "time",
];

pub const WHY_QUOTES: &str = "aspas, substituição ou heredoc sem fim";
pub const WHY_SHELL: &str = "shell sem -c";
pub const WHY_LONG: &str = "muito longo";
pub const WHY_DEEP: &str = "substituições demais uma dentro da outra";
pub const WHY_CD: &str = "cd com variável";

pub fn lex(command: &str) -> Script {
    let mut script = lex_at(command, 0);
    let words: usize = script
        .commands
        .iter()
        .map(|c| c.words.len() + c.assignments.len())
        .sum();
    if script.commands.len() > MAX_COMMANDS || words > MAX_WORDS {
        script.unreadable(WHY_LONG);
    }
    script
}

fn lex_at(command: &str, depth: usize) -> Script {
    let mut script = Script {
        commands: vec![],
        readable: true,
        why: None,
    };
    if depth > MAX_DEPTH {
        script.unreadable(WHY_DEEP);
        return script;
    }
    if command.len() > MAX_BYTES {
        script.unreadable(WHY_LONG);
        return script;
    }
    let mut lexer = Lexer {
        chars: command.chars().collect(),
        pos: 0,
        readable: true,
        subs: vec![],
        heredocs: vec![],
    };
    let tokens = lexer.tokens();
    script.commands = group(tokens);
    if !lexer.readable {
        script.unreadable(WHY_QUOTES);
    }
    if !follow_cd(&mut script.commands) {
        script.unreadable(WHY_CD);
    }
    for sub in std::mem::take(&mut lexer.subs) {
        merge(&mut script, lex_at(&sub, depth + 1));
    }
    let mut extra = Script {
        readable: true,
        ..Script::default()
    };
    for cmd in &script.commands {
        unwrap(cmd, depth, &mut extra);
    }
    merge(&mut script, extra);
    script
}

/// Gives each command the folder it runs in, following `cd`, `pushd`, `popd` and `cd -`. False when a `cd` goes
/// somewhere only known when it runs (`cd $x`, `cd ~`, a bare `cd`).
fn follow_cd(commands: &mut [SimpleCommand]) -> bool {
    let join = |dir: &Option<String>, to: &str| match dir {
        _ if to.starts_with('/') => to.to_string(),
        Some(d) => format!("{d}/{to}"),
        None => to.to_string(),
    };
    let mut dir: Option<String> = None;
    let mut previous: Option<String> = None;
    let mut stack: Vec<Option<String>> = vec![];
    let mut known = true;
    for cmd in commands.iter_mut() {
        cmd.dir = dir.clone();
        match cmd.program() {
            Some(program @ ("cd" | "pushd")) => {
                let target = cmd.words[1..]
                    .iter()
                    .find(|w| w.text == "-" || !w.text.starts_with('-'));
                match target {
                    Some(w) if w.text == "-" && !w.dynamic => std::mem::swap(&mut dir, &mut previous),
                    Some(w) if !w.dynamic => {
                        if program == "pushd" {
                            stack.push(dir.clone());
                        }
                        previous = dir.clone();
                        dir = Some(join(&dir, &w.text));
                    }
                    _ => known = false,
                }
            }
            Some("popd") => dir = stack.pop().flatten(),
            _ => {}
        }
    }
    known
}

fn merge(script: &mut Script, inner: Script) {
    if !inner.readable {
        script.unreadable(inner.why.unwrap_or(WHY_QUOTES));
    }
    script.commands.extend(inner.commands);
}

/// The script a command runs: `sh -c '…'`, `eval …`, `env -S '…'`. A shell without `-c` makes the line unreadable.
fn unwrap(cmd: &SimpleCommand, depth: usize, out: &mut Script) {
    let Some(program) = cmd.program() else { return };
    let rest = &cmd.words[1..];
    if SHELLS.contains(&program) {
        // `-c`, `-lc`, `-ec`: a single-dash cluster with a `c`; the script is the next word that is not an option.
        let flag = rest.iter().position(|w| {
            w.text.starts_with('-')
                && !w.text.starts_with("--")
                && w.text[1..].chars().all(|c| c.is_ascii_alphabetic())
                && w.text.contains('c')
        });
        match flag.and_then(|at| rest[at + 1..].iter().find(|w| !w.text.starts_with('-'))) {
            Some(script) if depth < MAX_DEPTH => merge(out, lex_at(&script.text, depth + 1)),
            Some(_) => out.unreadable(WHY_DEEP),
            None => out.unreadable(WHY_SHELL),
        }
        return;
    }
    if program == "eval" && depth < MAX_DEPTH {
        let text: Vec<&str> = rest.iter().map(|w| w.text.as_str()).collect();
        merge(out, lex_at(&text.join(" "), depth + 1));
        return;
    }
    if program == "env" {
        let split = rest.iter().enumerate().find_map(|(i, w)| match w.text.as_str() {
            "-S" | "--split-string" => rest.get(i + 1).map(|w| w.text.clone()),
            text => text.strip_prefix("--split-string=").map(str::to_string),
        });
        if let Some(script) = split {
            merge(out, lex_at(&script, depth + 1));
        }
    }
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
        // Brace expansion outside quotes (`{a,b}`, `pass{o,}`, `{1..3}`): an open brace, then a comma or `..`.
        let mut brace = false;
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
                '{' => {
                    self.pos += 1;
                    word.text.push(c);
                    brace = true;
                }
                ',' if brace => {
                    self.pos += 1;
                    word.text.push(c);
                    word.dynamic = true;
                }
                '.' if brace && self.peek(1) == Some('.') => {
                    self.pos += 2;
                    word.text.push_str("..");
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
                    Some(inner) => {
                        let inner = inner.trim_start_matches('(').trim_end_matches(')').to_string();
                        word.subs.push(inner.trim().to_string());
                        self.subs.push(inner);
                    }
                    None => self.readable = false,
                }
            }
            Some('{') => {
                self.pos += 1;
                match self.balanced('}') {
                    Some(inner) => {
                        let name: String = inner
                            .chars()
                            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                            .collect();
                        if !name.is_empty() {
                            word.vars.push(name);
                        }
                    }
                    None => self.readable = false,
                }
            }
            Some(c) if c.is_ascii_alphanumeric() || c == '_' => {
                let start = self.pos;
                while self.peek(0).is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                    self.pos += 1;
                }
                word.vars.push(self.chars[start..self.pos].iter().collect());
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
            word.subs.push(inner.trim().to_string());
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
                } else if current.words.is_empty()
                    && !quoted_name
                    && !word.dynamic
                    && RESERVED.contains(&word.text.as_str())
                {
                    // `if`, `then`, `{`…: what follows starts a command.
                } else if current.words.is_empty() && !quoted_name && is_assignment(&word.text) {
                    let (name, value) = word.text.split_once('=').unwrap_or_default();
                    let value = Word {
                        text: value.to_string(),
                        dynamic: word.dynamic,
                        subs: word.subs.clone(),
                        vars: word.vars.clone(),
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
        assert!(programs("bash -lc 'passo ok'").contains(&"passo".to_string()));
        assert!(programs("sh -ec -- 'passo stop'").contains(&"passo".to_string()));
        assert!(programs("env -S 'passo ok'").contains(&"passo".to_string()));
        // Prefixes are not followed: the lock looks for `passo` in every word instead.
        assert_eq!(programs("sudo -u x git push"), vec!["sudo"]);
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
