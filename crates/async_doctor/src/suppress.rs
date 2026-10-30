//! Inline `# async-doctor: ignore[rule]: reason` suppressions.
//!
//! Zero-config: comments only, no config file. A reason is required. Unused
//! suppressions are reported so agents do not leave dead ignores behind.
//! Comment detection uses a small Python tokenizer so `#` inside strings is
//! ignored.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use async_doctor_core::{Report, UnusedSuppression};

use crate::paths::{normalize_path, paths_equal};

/// One suppression comment in source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suppression {
    pub file: String,
    pub line: u32,
    pub rule: String,
    pub reason: String,
}

/// An `ignore[rule]` comment that omitted the required reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingReasonIgnore {
    pub file: String,
    pub line: u32,
    pub rule: String,
}

/// Result of applying suppressions to a report.
#[derive(Debug)]
pub struct SuppressOutcome {
    pub report: Report,
    /// Findings removed by a matching ignore comment.
    pub suppressed_findings: u32,
    pub missing_reason: Vec<MissingReasonIgnore>,
}

/// Parse suppressions under `root`, apply them to `report`, and collect unused.
pub fn apply(mut report: Report, root: &Path) -> anyhow::Result<SuppressOutcome> {
    let files = files_to_scan(&report, root);
    let mut by_file: HashMap<String, Vec<Suppression>> = HashMap::new();
    let mut missing_reason = Vec::new();
    for file in files {
        let rel = relativize(root, &file);
        let text = match fs::read_to_string(&file) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let parsed = parse_file(&rel, &text);
        missing_reason.extend(parsed.missing_reason);
        if !parsed.suppressions.is_empty() {
            by_file.insert(rel, parsed.suppressions);
        }
    }

    let mut used: HashSet<(String, u32, String)> = HashSet::new();
    let mut findings = Vec::new();
    let mut suppressed_findings = 0u32;
    for f in std::mem::take(&mut report.findings) {
        if let Some(sup) =
            matching_suppression(&f.rule, &f.entry.file, f.entry.line, &f.chain, &by_file)
        {
            used.insert((sup.file.clone(), sup.line, sup.rule.clone()));
            suppressed_findings += 1;
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
    missing_reason.sort_by(|a, b| (&a.file, a.line, &a.rule).cmp(&(&b.file, b.line, &b.rule)));

    report.findings = findings;
    report.candidates = candidates;
    report.unused_suppressions = unused;
    Ok(SuppressOutcome {
        report,
        suppressed_findings,
        missing_reason,
    })
}

fn matching_suppression<'a>(
    rule: &str,
    entry_file: &str,
    entry_line: u32,
    chain: &[async_doctor_core::ChainStep],
    by_file: &'a HashMap<String, Vec<Suppression>>,
) -> Option<&'a Suppression> {
    let mut locs: Vec<(&str, u32)> = vec![(entry_file, entry_line)];
    for step in chain {
        locs.push((&step.file, step.line));
    }
    for (file, line) in locs {
        let key = normalize_path(file);
        let Some(sups) = by_file.get(&key).or_else(|| {
            by_file
                .iter()
                .find(|(k, _)| paths_equal(k, &key))
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

#[derive(Debug, Default)]
struct ParseResult {
    suppressions: Vec<Suppression>,
    missing_reason: Vec<MissingReasonIgnore>,
}

fn parse_file(file: &str, source: &str) -> ParseResult {
    let mut out = ParseResult::default();
    for (idx, line) in source.lines().enumerate() {
        let line_no = (idx + 1) as u32;
        let Some(comment) = comment_text(line) else {
            continue;
        };
        match parse_ignore_comment(comment) {
            IgnoreParse::Suppression { rule, reason } => {
                out.suppressions.push(Suppression {
                    file: normalize_path(file),
                    line: line_no,
                    rule,
                    reason,
                });
            }
            IgnoreParse::MissingReason { rule } => {
                out.missing_reason.push(MissingReasonIgnore {
                    file: normalize_path(file),
                    line: line_no,
                    rule,
                });
            }
            IgnoreParse::None => {}
        }
    }
    out
}

/// Extract the `# …` comment body from a physical line, ignoring `#` inside
/// string / bytes / f-string literals (simple Python tokenizer).
fn comment_text(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        // Triple-quoted strings first.
        if matches_at(bytes, i, b"'''") || matches_at(bytes, i, b"\"\"\"") {
            let quote = &bytes[i..i + 3];
            i += 3;
            while i + 2 < bytes.len() {
                if &bytes[i..i + 3] == quote {
                    i += 3;
                    break;
                }
                if bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                i += 1;
            }
            continue;
        }
        // Prefixes: r/R/f/F/b/B/u/U and combinations before a quote.
        if let Some((start, q)) = string_start(bytes, i) {
            i = start + 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if bytes[i] == q {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if c == b'#' {
            return Some(line[i + 1..].trim());
        }
        i += 1;
    }
    None
}

fn matches_at(bytes: &[u8], i: usize, pat: &[u8]) -> bool {
    i + pat.len() <= bytes.len() && &bytes[i..i + pat.len()] == pat
}

fn string_start(bytes: &[u8], i: usize) -> Option<(usize, u8)> {
    // Optional string prefixes then a single quote.
    let mut j = i;
    while j < bytes.len()
        && matches!(
            bytes[j],
            b'r' | b'R' | b'f' | b'F' | b'b' | b'B' | b'u' | b'U'
        )
    {
        j += 1;
    }
    if j < bytes.len() && (bytes[j] == b'\'' || bytes[j] == b'"') {
        // Avoid treating prefixes that aren't actually prefixes (e.g. identifier).
        if j > i {
            // Must be at start or after non-identifier char.
            if i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_') {
                return None;
            }
        }
        return Some((j, bytes[j]));
    }
    None
}

enum IgnoreParse {
    Suppression { rule: String, reason: String },
    MissingReason { rule: String },
    None,
}

fn parse_ignore_comment(comment: &str) -> IgnoreParse {
    let rest = match comment
        .strip_prefix("async-doctor:")
        .or_else(|| comment.strip_prefix("async-doctor :"))
    {
        Some(r) => r.trim_start(),
        None => return IgnoreParse::None,
    };
    let rest = match rest.strip_prefix("ignore") {
        Some(r) => r.trim_start(),
        None => return IgnoreParse::None,
    };
    if !rest.starts_with('[') {
        return IgnoreParse::None;
    }
    let Some(end) = rest.find(']') else {
        return IgnoreParse::None;
    };
    let rule = rest[1..end].trim();
    if rule.is_empty() {
        return IgnoreParse::None;
    }
    let after = rest[end + 1..].trim_start();
    match after.strip_prefix(':') {
        Some(reason) => {
            let reason = reason.split(" # ").next().unwrap_or(reason).trim();
            if reason.is_empty() {
                IgnoreParse::MissingReason {
                    rule: rule.to_string(),
                }
            } else {
                IgnoreParse::Suppression {
                    rule: rule.to_string(),
                    reason: reason.to_string(),
                }
            }
        }
        None => IgnoreParse::MissingReason {
            rule: rule.to_string(),
        },
    }
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
        .map(|p| normalize_path(&p.to_string_lossy()))
        .unwrap_or_else(|_| normalize_path(&file.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_requires_reason() {
        let src = "time.sleep(1)  # async-doctor: ignore[blocking-sleep]\n";
        let p = parse_file("a.py", src);
        assert!(p.suppressions.is_empty());
        assert_eq!(p.missing_reason.len(), 1);
        assert_eq!(p.missing_reason[0].rule, "blocking-sleep");

        let src = "time.sleep(1)  # async-doctor: ignore[blocking-sleep]: known sync boundary\n";
        let p = parse_file("a.py", src);
        assert_eq!(p.suppressions.len(), 1);
        assert_eq!(p.suppressions[0].reason, "known sync boundary");
    }

    #[test]
    fn hash_inside_string_is_not_a_comment() {
        let src = "x = \"# async-doctor: ignore[blocking-sleep]: fake\"\ntime.sleep(1)\n";
        let p = parse_file("a.py", src);
        assert!(p.suppressions.is_empty(), "{p:?}");
        assert!(p.missing_reason.is_empty());
    }

    #[test]
    fn hash_inside_single_quotes_ignored() {
        let src = "x = '# async-doctor: ignore[blocking-sleep]: fake'\n";
        assert!(parse_file("a.py", src).suppressions.is_empty());
    }
}
