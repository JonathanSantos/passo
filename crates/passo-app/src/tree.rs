//! A short tree of the repository for the agents that refine and plan: where things are, without file contents.
//! Cheap models spend most of their tokens finding their way around; the tree spends a few dozen lines instead.

use std::collections::BTreeMap;
use std::path::Path;

use globset::{Glob, GlobSetBuilder};
use passo_core::Config;

use crate::infra::{Git, project_files};

const CODE: [&str; 24] = [
    "rs", "js", "mjs", "cjs", "ts", "tsx", "jsx", "py", "go", "java", "kt", "rb", "php", "cs", "c", "h", "cpp", "hpp",
    "swift", "scala", "ex", "exs", "vue", "svelte",
];

/// Builds the tree, at most `limit` lines: folders two levels deep with their file counts, the largest code
/// files, where the tests live, and the configured checks.
pub fn build(root: &Path, git: Option<&Git>, config: &Config, limit: usize) -> String {
    let files = project_files(root, git);
    let mut sections: Vec<Vec<String>> = vec![];

    let mut top: BTreeMap<String, usize> = BTreeMap::new();
    let mut second: BTreeMap<String, usize> = BTreeMap::new();
    let mut at_root = 0;
    for file in &files {
        let parts: Vec<&str> = file.split('/').collect();
        match parts.len() {
            1 => at_root += 1,
            2 => *top.entry(parts[0].to_string()).or_default() += 1,
            _ => {
                *top.entry(parts[0].to_string()).or_default() += 1;
                *second.entry(format!("{}/{}", parts[0], parts[1])).or_default() += 1;
            }
        }
    }
    let mut tree = vec![format!("Pastas ({} arquivos; {at_root} na raiz):", files.len())];
    for (dir, count) in &top {
        tree.push(format!("  {dir}/ ({count})"));
        for (sub, n) in second
            .range(format!("{dir}/")..)
            .take_while(|(k, _)| k.starts_with(&format!("{dir}/")))
        {
            tree.push(format!("    {sub}/ ({n})"));
        }
    }
    sections.push(tree);

    let mut code: Vec<(usize, &String)> = files
        .iter()
        .filter(|f| f.rsplit_once('.').is_some_and(|(_, ext)| CODE.contains(&ext)))
        .map(|f| {
            (
                std::fs::read_to_string(root.join(f))
                    .map(|t| t.lines().count())
                    .unwrap_or(0),
                f,
            )
        })
        .collect();
    code.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    if !code.is_empty() {
        let mut lines = vec!["Maiores arquivos de código (linhas):".to_string()];
        lines.extend(code.iter().take(10).map(|(n, f)| format!("  {f} ({n})")));
        sections.push(lines);
    }

    let mut builder = GlobSetBuilder::new();
    for pattern in &config.tests.patterns {
        if let Ok(glob) = Glob::new(pattern) {
            builder.add(glob);
        }
    }
    if let Ok(tests) = builder.build() {
        let mut dirs: BTreeMap<String, usize> = BTreeMap::new();
        for file in files.iter().filter(|f| tests.is_match(f)) {
            let dir = file
                .rsplit_once('/')
                .map(|(d, _)| format!("{d}/"))
                .unwrap_or_else(|| "(raiz)".into());
            *dirs.entry(dir).or_default() += 1;
        }
        if !dirs.is_empty() {
            let mut lines = vec!["Testes:".to_string()];
            lines.extend(dirs.iter().take(8).map(|(d, n)| format!("  {d} ({n})")));
            sections.push(lines);
        }
    }

    let show = |v: &[String]| {
        if v.is_empty() {
            "nenhuma".to_string()
        } else {
            v.join(" · ")
        }
    };
    sections.push(vec![
        format!("Checagens por passo: {}", show(&config.checks.step)),
        format!("Checagens finais: {}", show(&config.checks.final_)),
    ]);

    // The folder tree gives way first: the other sections are short and always fit.
    let fixed: usize = sections[1..].iter().map(Vec::len).sum();
    let room = limit.saturating_sub(fixed).max(2);
    if sections[0].len() > room {
        let hidden = sections[0].len() - (room - 1);
        sections[0].truncate(room - 1);
        sections[0].push(format!("  … e mais {hidden} pasta(s)"));
    }
    let mut lines: Vec<String> = sections.into_iter().flatten().collect();
    lines.truncate(limit);
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_big_tree_fits_the_limit() {
        let root = std::env::temp_dir().join(format!("passo-tree-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        for i in 0..200 {
            let dir = root.join(format!("pkg{}/mod{}", i % 40, i % 7));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("f{i}.rs")), "fn x() {}\n".repeat(i % 13 + 1)).unwrap();
        }
        std::fs::create_dir_all(root.join("node_modules/dep")).unwrap();
        std::fs::write(root.join("node_modules/dep/index.js"), "x").unwrap();
        let tree = build(&root, None, &Config::default(), 60);
        assert!(tree.lines().count() <= 60, "{tree}");
        assert!(tree.contains("pkg0/") && tree.contains("Maiores arquivos de código"));
        assert!(!tree.contains("node_modules"));
        std::fs::remove_dir_all(root).ok();
    }
}
