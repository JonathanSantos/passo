//! Diff coverage: of the lines a run changed, how many some test executed. The lcov file comes from the project's
//! own coverage tool; the changed lines from git. Both are facts the app hands in; the arithmetic lives here.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Lines per file.
pub type Lines = BTreeMap<String, BTreeSet<u32>>;

/// What an lcov file says: the lines that can run, and the ones that did at least once.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lcov {
    pub instrumented: Lines,
    pub executed: Lines,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiffCoverage {
    /// False when nothing could be measured (no git base, the tool failed, no lcov).
    pub measured: bool,
    /// Executed changed lines over instrumented changed lines, rounded down; 100 when no changed line counts.
    pub percent: u32,
    /// Changed lines the lcov counts (lines like `use` or comments do not).
    pub lines: usize,
    /// Changed lines no test executed, per file.
    pub uncovered: Vec<(String, Vec<u32>)>,
    /// Why it was not measured.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

impl DiffCoverage {
    pub fn skipped(reason: impl Into<String>) -> DiffCoverage {
        DiffCoverage {
            reason: reason.into(),
            ..Default::default()
        }
    }

    pub fn uncovered_lines(&self) -> usize {
        self.uncovered.iter().map(|(_, l)| l.len()).sum()
    }

    /// `Cobertura do diff: 83% (12 linhas sem teste em 2 arquivos)`.
    pub fn summary(&self) -> String {
        if !self.measured {
            return format!("Cobertura do diff não medida: {}", self.reason);
        }
        if self.uncovered.is_empty() {
            return format!("Cobertura do diff: {}%", self.percent);
        }
        format!(
            "Cobertura do diff: {}% ({} linha(s) sem teste em {} arquivo(s))",
            self.percent,
            self.uncovered_lines(),
            self.uncovered.len()
        )
    }

    /// `src/x.rs 40-52, 60; src/y.rs 10`.
    pub fn uncovered_text(&self) -> String {
        self.uncovered
            .iter()
            .map(|(file, lines)| format!("{file} {}", ranges(lines)))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// `40-52, 60`: consecutive lines collapse into a range.
pub fn ranges(lines: &[u32]) -> String {
    let mut out: Vec<String> = vec![];
    let mut i = 0;
    while i < lines.len() {
        let start = lines[i];
        let mut end = start;
        while i + 1 < lines.len() && lines[i + 1] == end + 1 {
            i += 1;
            end = lines[i];
        }
        out.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
        i += 1;
    }
    out.join(", ")
}

/// Reads `SF:` and `DA:<line>,<hits>` records. Absolute paths under `root` become relative to it.
pub fn parse_lcov(text: &str, root: &str) -> Lcov {
    let root = root.trim_end_matches('/');
    let mut lcov = Lcov::default();
    let mut file: Option<String> = None;
    for line in text.lines().map(str::trim) {
        if let Some(path) = line.strip_prefix("SF:") {
            let path = path.trim();
            let relative = path
                .strip_prefix(root)
                .and_then(|p| p.strip_prefix('/'))
                .unwrap_or(path)
                .trim_start_matches("./");
            file = Some(relative.to_string());
        } else if line == "end_of_record" {
            file = None;
        } else if let (Some(file), Some(data)) = (&file, line.strip_prefix("DA:")) {
            let mut parts = data.split(',');
            let (Some(Ok(number)), Some(Ok(hits))) = (
                parts.next().map(|n| n.trim().parse::<u32>()),
                parts.next().map(|h| h.trim().parse::<u64>()),
            ) else {
                continue;
            };
            lcov.instrumented.entry(file.clone()).or_default().insert(number);
            if hits > 0 {
                lcov.executed.entry(file.clone()).or_default().insert(number);
            }
        }
    }
    lcov
}

/// Of the changed lines the lcov instruments, how many it executed.
pub fn diff_coverage(changed: &Lines, executed: &Lines, instrumented: &Lines) -> DiffCoverage {
    let empty = BTreeSet::new();
    let mut lines = 0;
    let mut covered: usize = 0;
    let mut uncovered = vec![];
    for (file, changed_lines) in changed {
        let counts = instrumented.get(file).unwrap_or(&empty);
        let ran = executed.get(file).unwrap_or(&empty);
        let mut missing = vec![];
        for line in changed_lines.iter().filter(|l| counts.contains(l)) {
            lines += 1;
            if ran.contains(line) {
                covered += 1;
            } else {
                missing.push(*line);
            }
        }
        if !missing.is_empty() {
            uncovered.push((file.clone(), missing));
        }
    }
    // Nothing that counts changed: nothing left uncovered.
    let percent = (covered * 100).checked_div(lines).unwrap_or(100) as u32;
    DiffCoverage {
        measured: true,
        percent,
        lines,
        uncovered,
        reason: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(entries: &[(&str, &[u32])]) -> Lines {
        entries
            .iter()
            .map(|(f, l)| (f.to_string(), l.iter().copied().collect()))
            .collect()
    }

    #[test]
    fn lcov_with_two_files_and_lines_never_run() {
        let text = "TN:\nSF:/p/src/a.rs\nDA:1,3\nDA:2,0\nDA:3,1\nend_of_record\nSF:src/b.rs\nDA:10,0\nend_of_record\n";
        let lcov = parse_lcov(text, "/p");
        assert_eq!(
            lcov.instrumented,
            lines(&[("src/a.rs", &[1, 2, 3]), ("src/b.rs", &[10])])
        );
        assert_eq!(lcov.executed, lines(&[("src/a.rs", &[1, 3])]));
    }

    #[test]
    fn lines_the_lcov_does_not_instrument_do_not_count() {
        let changed = lines(&[("src/a.rs", &[1, 2, 3, 4, 5]), ("src/b.rs", &[10, 11, 12])]);
        let instrumented = lines(&[("src/a.rs", &[2, 3, 4, 5]), ("src/b.rs", &[10, 11, 12])]);
        let executed = lines(&[("src/a.rs", &[2, 3, 4, 5]), ("src/b.rs", &[10])]);
        let c = diff_coverage(&changed, &executed, &instrumented);
        assert_eq!((c.lines, c.percent), (7, 71));
        assert_eq!(c.uncovered, vec![("src/b.rs".to_string(), vec![11, 12])]);
        assert_eq!(c.uncovered_text(), "src/b.rs 11-12");
        assert_eq!(
            c.summary(),
            "Cobertura do diff: 71% (2 linha(s) sem teste em 1 arquivo(s))"
        );
        assert_eq!(ranges(&[1, 2, 3, 7, 9, 10]), "1-3, 7, 9-10");
    }

    #[test]
    fn nothing_instrumented_changed_is_full_coverage() {
        let c = diff_coverage(&lines(&[("README", &[1])]), &Lines::new(), &Lines::new());
        assert_eq!((c.percent, c.lines), (100, 0));
    }
}
