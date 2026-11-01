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
///
/// Only reads first-party project files (same discovery as extraction: honors
/// `.gitignore` / excludes) plus any paths named in the report. Never walks
/// gitignored trees.
pub fn apply(mut report: Report, root: &Path) -> anyhow::Result<SuppressOutcome> {
    let files = files_to_scan(&report, root)?;
    let mut by_file: HashMap<String, Vec<Suppression>> = HashMap::new();
    let mut missing_reason = Vec::new();
    let scan_one = |file: &PathBuf| -> Option<(String, ParseResult)> {
        let text = fs::read_to_string(file).ok()?;
        if !text.contains("async-doctor") {
            return None;
        }
        let rel = relativize(root, file);
        let parsed = parse_file(&rel, &text);
        if parsed.suppressions.is_empty() && parsed.missing_reason.is_empty() {
            return None;
        }
        Some((rel, parsed))
    };
    let hits: Vec<(String, ParseResult)> = if files.len() >= 64 {
        let nthreads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(8);
        let chunk_size = files.len().div_ceil(nthreads).max(1);
        std::thread::scope(|s| {
            let handles: Vec<_> = files
                .chunks(chunk_size)
                .map(|chunk| s.spawn(move || chunk.iter().filter_map(scan_one).collect::<Vec<_>>()))
                .collect();
            let mut out = Vec::new();
            for h in handles {
                out.extend(h.join().expect("suppress worker"));
            }
            out
        })
    } else {
        files.iter().filter_map(scan_one).collect()
    };
    for (rel, parsed) in hits {
        missing_reason.extend(parsed.missing_reason);
        if !parsed.suppressions.is_empty() {
            by_file.insert(rel, parsed.suppressions);
        }
    }

    if by_file.is_empty() {
        missing_reason.sort_by(|a, b| (&a.file, a.line, &a.rule).cmp(&(&b.file, b.line, &b.rule)));
        report.unused_suppressions = Vec::new();
        return Ok(SuppressOutcome {
            report,
            suppressed_findings: 0,
            missing_reason,
        });
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
    let mut advice = Vec::new();
    for a in std::mem::take(&mut report.advice) {
        if let Some(sup) =
            matching_suppression(&a.rule, &a.entry.file, a.entry.line, &a.chain, &by_file)
        {
            used.insert((sup.file.clone(), sup.line, sup.rule.clone()));
            continue;
        }
        advice.push(a);
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
    report.advice = advice;
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

fn files_to_scan<'a>(report: &'a Report, root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths = HashSet::new();
    let mut rel_seen: HashSet<&'a str> = HashSet::new();
    let mut add = |paths: &mut HashSet<PathBuf>, rel: &'a str| {
        if !rel.is_empty() && rel_seen.insert(rel) {
            let p = root.join(rel);
            if p.is_file() {
                paths.insert(p);
            }
        }
    };
    // Finding / candidate locations (entry + chain hops) — suppressions apply there.
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
    // Same first-party set extraction uses — required so unused suppressions in
    // clean files are still reported, without re-walking gitignored trees.
    for p in async_doctor_ty::discover_python_files(root)? {
        paths.insert(p);
    }
    Ok(paths.into_iter().collect())
}

fn relativize(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .map(|p| normalize_path(&p.to_string_lossy()))
        .unwrap_or_else(|_| normalize_path(&file.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    use async_doctor_core::Report;

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

    #[test]
    fn files_to_scan_skips_gitignored_py() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(root)
                    .env("GIT_AUTHOR_NAME", "t")
                    .env("GIT_AUTHOR_EMAIL", "t@t")
                    .env("GIT_COMMITTER_NAME", "t")
                    .env("GIT_COMMITTER_EMAIL", "t@t")
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "-b", "main"]);
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        fs::write(root.join("app.py"), "async def h():\n    return 1\n").unwrap();
        fs::create_dir_all(root.join("ignored")).unwrap();
        fs::write(root.join("ignored/x.py"), "x = 1\n").unwrap();
        git(&["add", ".gitignore", "app.py"]);
        git(&["commit", "-m", "init"]);

        let paths = files_to_scan(&Report::empty_complete(), root).unwrap();
        assert_eq!(paths.len(), 1, "{paths:?}");
        assert!(
            paths[0].ends_with("app.py"),
            "expected only app.py, got {paths:?}"
        );
    }
}
