//! A light touch of color on a terminal: headings, marks and the dev's turn. Plain text everywhere else
//! (pipes, agents, `NO_COLOR`).

use std::io::IsTerminal;

fn enabled() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const ACCENT: &str = "\x1b[36m";
const RESET: &str = "\x1b[0m";

pub fn paint(text: &str) -> String {
    if !enabled() {
        return text.to_string();
    }
    text.lines()
        .map(|line| {
            if line.starts_with("# ") || line.starts_with("## ") {
                format!("{BOLD}{line}{RESET}")
            } else if line.starts_with("passo · ") {
                format!("{DIM}{line}{RESET}")
            } else {
                line.replace('✓', &format!("{GREEN}✓{RESET}"))
                    .replace('✗', &format!("{RED}✗{RESET}"))
                    .replace('▸', &format!("{ACCENT}▸{RESET}"))
                    .replace('★', &format!("{ACCENT}★{RESET}"))
                    .replace("vez do dev", &format!("{ACCENT}vez do dev{RESET}"))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn error(text: &str) -> String {
    if std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
        format!("{RED}passo:{RESET} {text}")
    } else {
        format!("passo: {text}")
    }
}
