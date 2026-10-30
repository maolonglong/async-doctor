//! Inline `# async-doctor: ignore[rule]: reason` suppressions.
//!
//! Zero-config: comments only, no config file. A reason is required. Unused
//! suppressions are reported so agents do not leave dead ignores behind.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use async_doctor_core::{Report, UnusedSuppression};

/// One suppression comment in source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suppression {
    pub file: String,
    pub line: u32,
    pub rule: String,
    pub reason: String,
}

/// Result of applying suppressions to a report.
#[derive(Debug)]
pub struct SuppressOutcome {
    pub report: Report,
}

/// Parse suppressions under `root`, apply them to `report`, and collect unused.
pub fn apply(mut report: Report, root: &Path) -> anyhow::Result<SuppressOutcome> {
    let files = files_to_scan(&report, root);
    let mut by_file: HashMap<String, Vec<Suppression>> = HashMap::new();
    for file in files {
        let rel = relativize(root, &file);
        let text = match fs::read_to_string(&file) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let parsed = parse_suppressions(&rel, &text);
        if !parsed.is_empty() {
            by_file.insert(rel, parsed);
        }
    }

    let mut used: HashSet<(String, u32, String)> = HashSet::new();
    let mut findings = Vec::new();
    for f in std::mem::take(&mut report.findings) {
        if let Some(sup) =
            matching_suppression(&f.rule, &f.entry.file, f.entry.line, &f.chain, &by_file)
        {
            used.insert((sup.file.clone(), sup.line, sup.rule.clone()));
            continue;
        }
        findings.push(f);
    }
    let mut candidates = Vec::new();
    for c in std::mem::take(&mut report.candidates) {
        if let Some(sup) =
            matching_suppression(&c.rule, &c.entry.file, c.entry.line, &c.chain, &by_file)
        {
            used.insert((sup.file.clone(), sup.line, sup.rule.clone()));
            continue;
        }
        candidates.push(c);
    }

    let mut unused = Vec::new();
    for sups in by_file.values() {
        for s in sups {
            if !used.contains(&(s.file.clone(), s.line, s.rule.clone())) {
                unused.push(UnusedSuppression {
                    file: s.file.clone(),
                    line: s.line,
                    rule: s.rule.clone(),
                    reason: s.reason.clone(),
                });
            }
        }
    }
    unused.sort_by(|a, b| (&a.file, a.line, &a.rule).cmp(&(&b.file, b.line, &b.rule)));

    report.findings = findings;
    report.candidates = candidates;
    report.unused_suppressions = unused;
    Ok(SuppressOutcome { report })
}

fn matching_suppression<'a>(
    rule: &str,
    entry_file: &str,
    entry_line: u32,
    chain: &[async_doctor_core::ChainStep],
    by_file: &'a HashMap<String, Vec<Suppression>>,
) -> Option<&'a Suppression> {
    // Suppression applies when it sits on the same line as a chain hop or entry
    // (or the immediately preceding line), for the matching rule.
    let mut locs: Vec<(&str, u32)> = vec![(entry_file, entry_line)];
    for step in chain {
        locs.push((&step.file, step.line));
    }
    for (file, line) in locs {
        let key = normalize_rel(file);
        let Some(sups) = by_file.get(&key).or_else(|| {
            by_file
                .iter()
                .find(|(k, _)| paths_match(k, &key))
                .map(|(_, v)| v)
        }) else {
            continue;
        };
        for s in sups {
            if s.rule != rule {
                continue;
            }
            if s.line == line || s.line + 1 == line {
                return Some(s);
            }
        }
    }
    None
}

/// Parse `# async-doctor: ignore[rule]: reason` (reason required, non-empty).
pub fn parse_suppressions(file: &str, source: &str) -> Vec<Suppression> {
    let mut out = Vec::new();
    for (idx, line) in source.lines().enumerate() {
        let line_no = (idx + 1) as u32;
        if let Some(sup) = parse_line(file, line_no, line) {
            out.push(sup);
        }
    }
    out
}

fn parse_line(file: &str, line: u32, text: &str) -> Option<Suppression> {
    let hash = text.find('#')?;
    let comment = text[hash + 1..].trim_start();
    let rest = comment
        .strip_prefix("async-doctor:")
        .or_else(|| comment.strip_prefix("async-doctor :"))?
        .trim_start();
    let rest = rest.strip_prefix("ignore")?;
    let rest = rest.trim_start();
    if !rest.starts_with('[') {
        return None;
    }
    let end = rest.find(']')?;
    let rule = rest[1..end].trim();
    if rule.is_empty() {
        return None;
    }
    let after = rest[end + 1..].trim_start();
    // Require `: reason` with non-empty reason.
    let reason = after.strip_prefix(':')?.trim();
    if reason.is_empty() {
        return None;
    }
    let reason = reason.split(" # ").next().unwrap_or(reason).trim();
    if reason.is_empty() {
        return None;
    }
    Some(Suppression {
        file: normalize_rel(file),
        line,
        rule: rule.to_string(),
        reason: reason.to_string(),
    })
}

fn files_to_scan(report: &Report, root: &Path) -> Vec<PathBuf> {
    let mut paths = HashSet::new();
    let add = |paths: &mut HashSet<PathBuf>, rel: &str| {
        let p = root.join(rel);
        if p.is_file() {
            paths.insert(p);
        }
    };
    for f in &report.findings {
        add(&mut paths, &f.entry.file);
        for s in &f.chain {
            add(&mut paths, &s.file);
        }
    }
    for c in &report.candidates {
        add(&mut paths, &c.entry.file);
        for s in &c.chain {
            add(&mut paths, &s.file);
        }
    }
    walk_py(root, &mut paths);
    paths.into_iter().collect()
}

fn walk_py(dir: &Path, into: &mut HashSet<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with('.')
            || name == "node_modules"
            || name == "target"
            || name == ".venv"
            || name == "venv"
            || name == "__pycache__"
            || name == "site-packages"
        {
            continue;
        }
        if path.is_dir() {
            walk_py(&path, into);
        } else if path.extension().and_then(|s| s.to_str()) == Some("py") {
            into.insert(path);
        }
    }
}

fn relativize(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .map(|p| normalize_rel(&p.to_string_lossy()))
        .unwrap_or_else(|_| normalize_rel(&file.to_string_lossy()))
}

fn normalize_rel(p: &str) -> String {
    p.replace('\\', "/").trim_start_matches("./").to_string()
}

fn paths_match(a: &str, b: &str) -> bool {
    let a = normalize_rel(a);
    let b = normalize_rel(b);
    a == b || a.ends_with(&b) || b.ends_with(&a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_requires_reason() {
        let src = "time.sleep(1)  # async-doctor: ignore[blocking-sleep]\n";
        assert!(parse_suppressions("a.py", src).is_empty());
        let src = "time.sleep(1)  # async-doctor: ignore[blocking-sleep]: known sync boundary\n";
        let s = parse_suppressions("a.py", src);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].rule, "blocking-sleep");
        assert_eq!(s[0].reason, "known sync boundary");
    }

    #[test]
    fn parse_preceding_line_form() {
        let src = "# async-doctor: ignore[blocking-http]: third-party sync client in thread\nrequests.get(url)\n";
        let s = parse_suppressions("a.py", src);
        assert_eq!(s[0].line, 1);
    }
}
