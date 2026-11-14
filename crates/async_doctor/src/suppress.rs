//! Inline `# async-doctor: ignore[rule]: reason` suppressions.
//!
//! Zero-config: comments only, no config file. A reason is required. Unused
//! suppressions are reported so agents do not leave dead ignores behind.
//! Comment detection uses a whole-file Python lexical scanner (`suppress/lexer.rs`)
//! so `#` inside single- or multi-line string literals is ignored, trailing
//! ignores on the closing line of multi-line calls match the call start line,
//! standalone ignores above `@decorator` stacks match the decorated `async def`,
//! and stacked pragmas / comma-separated rule lists work out of the box.

mod lexer;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use async_doctor_core::{Report, UnusedSuppression};

use crate::paths::{normalize_path, paths_equal};
use lexer::{ParseResult, parse_file};

/// One suppression comment rule in source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suppression {
    pub file: String,
    pub line: u32,
    pub rule: String,
    pub reason: String,
    /// True when the ignore comment appeared on its own line with no code before `#`.
    pub standalone: bool,
    /// Additional target line after skipping `@decorator` lines below a
    /// standalone ignore comment.
    pub decorated_def_line: Option<u32>,
    /// Opening lines of bracketed expressions that closed on `line`, plus
    /// `(stmt_start..=line)` when a multi-line statement finishes at depth 0.
    pub covered_lines: Vec<u32>,
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
#[allow(dead_code)]
pub fn apply(report: Report, root: &Path) -> anyhow::Result<SuppressOutcome> {
    apply_scoped(report, root, None)
}

/// Parse suppressions under `root`, apply them to `report`, and collect unused.
///
/// When `single_file` is `Some(target)`, discovery does NOT walk sibling `.py`
/// files in `root`: only `target` and chain-hop files referenced by `report`
/// are read, and `unused_suppressions` / `missing_reason` are reported strictly
/// for `target`.
pub fn apply_scoped(
    mut report: Report,
    root: &Path,
    single_file: Option<&Path>,
) -> anyhow::Result<SuppressOutcome> {
    let single_target = single_file.map(|f| {
        let canon = dunce::canonicalize(f).unwrap_or_else(|_| f.to_path_buf());
        let rel = relativize(root, &canon);
        (canon, rel)
    });
    let files = files_to_scan(
        &report,
        root,
        single_target.as_ref().map(|(p, _)| p.as_path()),
    )?;
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
        if single_target
            .as_ref()
            .is_none_or(|(_, target_rel)| paths_equal(&rel, target_rel))
        {
            missing_reason.extend(parsed.missing_reason);
        }
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
            if let Some((_, ref target_rel)) = single_target
                && !paths_equal(&s.file, target_rel)
            {
                continue;
            }
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
            if s.matches_line(line) {
                return Some(s);
            }
        }
    }
    None
}

impl Suppression {
    pub(crate) fn matches_line(&self, line: u32) -> bool {
        self.line == line
            || (self.standalone && self.line + 1 == line)
            || self.decorated_def_line == Some(line)
            || self.covered_lines.contains(&line)
    }
}

fn files_to_scan<'a>(
    report: &'a Report,
    root: &Path,
    single_file: Option<&Path>,
) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths: HashSet<PathBuf> = if let Some(target) = single_file {
        let mut set = HashSet::new();
        if target.is_file() {
            set.insert(target.to_path_buf());
        }
        set
    } else if let Some(candidates) = &report.ignore_candidate_files {
        candidates.iter().cloned().collect()
    } else {
        async_doctor_ty::discover_python_files(root)?
            .into_iter()
            .collect()
    };

    let mut rel_seen: HashSet<&'a str> = HashSet::new();
    let mut add = |paths: &mut HashSet<PathBuf>, rel: &'a str| {
        if !rel.is_empty() && rel_seen.insert(rel) {
            let p = root.join(rel);
            if !paths.contains(&p) && p.is_file() {
                paths.insert(p);
            }
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
    for a in &report.advice {
        add(&mut paths, &a.entry.file);
        for s in &a.chain {
            add(&mut paths, &s.file);
        }
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

        let paths = files_to_scan(&Report::empty_complete(), root, None).unwrap();
        assert_eq!(paths.len(), 1, "{paths:?}");
        assert!(
            paths[0].ends_with("app.py"),
            "expected only app.py, got {paths:?}"
        );
    }
}
