//! `--changed` reporting filter: full analysis, narrow reporting.
//!
//! Changed paths are always repo-root-relative. Finding paths (relative to the
//! scan root) are normalized to the same basis before **exact** comparison —
//! so scanning `svc/` while only root `pos.py` changed does not match
//! `svc/app/pos.py`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use async_doctor_core::{Advice, Candidate, Finding, Report};

use crate::paths::normalize_path;

/// Changed lines for one file, in NEW-side line numbers (inclusive ranges).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangedLines {
    /// Untracked, renamed without hunks, or unparsable — whole file changed.
    Whole,
    /// Inclusive (start, end) line ranges.
    Ranges(Vec<(u32, u32)>),
}

impl ChangedLines {
    fn contains(&self, line: u32) -> bool {
        match self {
            ChangedLines::Whole => true,
            ChangedLines::Ranges(rs) => rs.iter().any(|(a, b)| *a <= line && line <= *b),
        }
    }
}

/// Repo-root-relative path → changed line ranges.
pub type ChangedFiles = HashMap<String, ChangedLines>;

/// Outcome of resolving the changed-file set for `--changed`.
#[derive(Debug)]
pub enum ChangedSet {
    /// Filter reporting to these repo-root-relative paths.
    Filter {
        files: ChangedFiles,
        repo_root: PathBuf,
        note: Option<String>,
    },
    /// Could not determine a useful base — keep the full report.
    FullScan { note: String },
}

/// Resolve changed files, degrading gracefully when git metadata is incomplete.
pub fn resolve_changed(scan_path: &Path, git_ref: Option<&str>) -> ChangedSet {
    let Ok(repo_root) = repo_root_for(scan_path) else {
        return ChangedSet::FullScan {
            note: format!(
                "--changed: `{}` is not inside a git repository; reporting all findings",
                scan_path.display()
            ),
        };
    };

    match changed_files(&repo_root, git_ref) {
        Ok(files) => ChangedSet::Filter {
            files,
            repo_root,
            note: None,
        },
        Err(err) => ChangedSet::FullScan {
            note: format!(
                "--changed: could not resolve git base ({err:#}); reporting all findings"
            ),
        },
    }
}

/// Resolve changed paths and NEW-side line ranges relative to `repo_root`.
///
/// `git_ref` is `None` for the default (merge-base with the default branch).
pub fn changed_files(repo_root: &Path, git_ref: Option<&str>) -> anyhow::Result<ChangedFiles> {
    let repo_root = repo_root
        .canonicalize()
        .with_context(|| format!("canonicalize {}", repo_root.display()))?;

    let base = match git_ref {
        Some(r) if !r.is_empty() && r != "AUTO" => r.to_string(),
        _ => default_merge_base(&repo_root)?,
    };

    let mut files = ChangedFiles::new();
    // Committed vs base, unstaged, and staged differences — `-U0` gives
    // precise hunk ranges; union them per file. Explicit prefix + no-ext-diff
    // protect hunk parsing from user `diff.mnemonicPrefix` / `diff.noprefix` /
    // `diff.external` gitconfig settings.
    collect_diff_ranges(
        &repo_root,
        &[
            "diff",
            "--no-ext-diff",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "-U0",
            "--diff-filter=ACMR",
            &base,
        ],
        &mut files,
    )?;
    collect_diff_ranges(
        &repo_root,
        &[
            "diff",
            "--no-ext-diff",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "-U0",
            "--diff-filter=ACMR",
        ],
        &mut files,
    )?;
    collect_diff_ranges(
        &repo_root,
        &[
            "diff",
            "--no-ext-diff",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "-U0",
            "--diff-filter=ACMR",
            "--cached",
        ],
        &mut files,
    )?;
    let untracked = git_stdout(&repo_root, &["ls-files", "--others", "--exclude-standard"])?;
    for line in untracked.lines() {
        let line = line.trim();
        if !line.is_empty() {
            files.insert(normalize_path(line), ChangedLines::Whole);
        }
    }
    Ok(files)
}

/// Parse a `git diff -U0` stream into per-file NEW-side ranges and merge into `files`.
///
/// - `@@ -a[,n] +b[,m] @@` records `b..=b+m-1` (`m` omitted → 1 line).
/// - Pure deletion (`m == 0` at line `b`) marks `b` and `b + 1`.
/// - A file section with no usable `+++`/`@@` lines (rename-only, mode,
///   binary, unparsable) marks the whole file.
fn collect_diff_ranges(
    repo_root: &Path,
    args: &[&str],
    files: &mut ChangedFiles,
) -> anyhow::Result<()> {
    let out = git_stdout(repo_root, args)?;
    let mut file = String::new();
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    let mut hunks = 0usize;
    let mut malformed = false;
    let flush = |file: &mut String, ranges: &mut Vec<(u32, u32)>, files: &mut ChangedFiles| {
        if file.is_empty() {
            return;
        }
        let entry = ChangedLines::Ranges(std::mem::take(ranges));
        files
            .entry(std::mem::take(file))
            .and_modify(|e| merge_changed_lines(e, &entry))
            .or_insert(entry);
    };
    for line in out.lines() {
        if line.starts_with("diff --git") {
            if malformed {
                files.insert(std::mem::take(&mut file), ChangedLines::Whole);
            } else {
                flush(&mut file, &mut ranges, files);
            }
            hunks = 0;
            malformed = false;
            continue;
        }
        if let Some(rest) = line.strip_prefix("+++ ") {
            if let Some(path) = rest.strip_prefix("b/") {
                file = normalize_path(path);
            }
            continue;
        }
        if !line.starts_with("@@") {
            continue;
        }
        hunks += 1;
        let Some((start, count)) = parse_hunk_new_side(line) else {
            malformed = true;
            continue;
        };
        if count == 0 {
            // Pure deletion at line `start`: the join line may be either side.
            ranges.push((start, start.saturating_add(1)));
        } else {
            ranges.push((start, start + count - 1));
        }
    }
    if malformed {
        files.insert(file, ChangedLines::Whole);
    } else if !file.is_empty() && hunks == 0 {
        // Rename-only / mode-only / binary section: no NEW-side ranges.
        files.insert(file, ChangedLines::Whole);
    } else {
        flush(&mut file, &mut ranges, files);
    }
    Ok(())
}

/// Merge `incoming` into `existing`; `Whole` dominates.
fn merge_changed_lines(existing: &mut ChangedLines, incoming: &ChangedLines) {
    if matches!(existing, ChangedLines::Whole) || matches!(incoming, ChangedLines::Whole) {
        *existing = ChangedLines::Whole;
        return;
    }
    if let (ChangedLines::Ranges(a), ChangedLines::Ranges(b)) = (existing, incoming) {
        a.extend(b.iter().copied());
    }
}

/// Extract `(new_start, new_count)` from `@@ -o[,n] +s[,m] @@`.
fn parse_hunk_new_side(header: &str) -> Option<(u32, u32)> {
    let plus = header.find(" +")? + 2;
    let rest = &header[plus..];
    let end = rest.find(' ')?;
    let spec = &rest[..end];
    let (start, count) = match spec.split_once(',') {
        Some((s, c)) => (s.parse().ok()?, c.parse().ok()?),
        None => (spec.parse().ok()?, 1),
    };
    Some((start, count))
}

/// Keep findings/candidates/advice/unused-suppressions that touch a changed line.
///
/// `scan_root` is the directory that finding paths are relative to.
/// `changed` / `repo_root` use repo-root-relative paths.
pub fn filter_report(
    mut report: Report,
    changed: &ChangedFiles,
    scan_root: &Path,
    repo_root: &Path,
) -> Report {
    let canon_repo = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let mut rel_cache: HashMap<String, Option<String>> = HashMap::new();
    report
        .findings
        .retain(|f| finding_touches(f, changed, scan_root, &canon_repo, &mut rel_cache));
    report
        .candidates
        .retain(|c| candidate_touches(c, changed, scan_root, &canon_repo, &mut rel_cache));
    report
        .advice
        .retain(|a| advice_touches(a, changed, scan_root, &canon_repo, &mut rel_cache));
    report.unused_suppressions.retain(|u| {
        loc_in_changed(
            &u.file,
            u.line,
            changed,
            scan_root,
            &canon_repo,
            &mut rel_cache,
        )
    });
    report
}

fn finding_touches(
    f: &Finding,
    changed: &ChangedFiles,
    scan_root: &Path,
    canon_repo: &Path,
    rel_cache: &mut HashMap<String, Option<String>>,
) -> bool {
    loc_in_changed(
        &f.entry.file,
        f.entry.line,
        changed,
        scan_root,
        canon_repo,
        rel_cache,
    ) || f
        .chain
        .iter()
        .any(|s| loc_in_changed(&s.file, s.line, changed, scan_root, canon_repo, rel_cache))
}

fn candidate_touches(
    c: &Candidate,
    changed: &ChangedFiles,
    scan_root: &Path,
    canon_repo: &Path,
    rel_cache: &mut HashMap<String, Option<String>>,
) -> bool {
    loc_in_changed(
        &c.entry.file,
        c.entry.line,
        changed,
        scan_root,
        canon_repo,
        rel_cache,
    ) || c
        .chain
        .iter()
        .any(|s| loc_in_changed(&s.file, s.line, changed, scan_root, canon_repo, rel_cache))
}

fn advice_touches(
    a: &Advice,
    changed: &ChangedFiles,
    scan_root: &Path,
    canon_repo: &Path,
    rel_cache: &mut HashMap<String, Option<String>>,
) -> bool {
    loc_in_changed(
        &a.entry.file,
        a.entry.line,
        changed,
        scan_root,
        canon_repo,
        rel_cache,
    ) || a
        .chain
        .iter()
        .any(|s| loc_in_changed(&s.file, s.line, changed, scan_root, canon_repo, rel_cache))
}

fn loc_in_changed(
    path: &str,
    line: u32,
    changed: &ChangedFiles,
    scan_root: &Path,
    canon_repo: &Path,
    rel_cache: &mut HashMap<String, Option<String>>,
) -> bool {
    let repo_rel = if let Some(cached) = rel_cache.get(path) {
        cached.as_deref()
    } else {
        let computed = to_repo_relative(path, scan_root, canon_repo);
        rel_cache.insert(path.to_string(), computed);
        rel_cache.get(path).and_then(|v| v.as_deref())
    };
    let Some(repo_rel) = repo_rel else {
        return false;
    };
    // Exact path equality after repo-root normalization, then range match.
    changed.get(repo_rel).is_some_and(|r| r.contains(line))
}

/// Convert a finding path (relative to `scan_root`) into a repo-root-relative path.
fn to_repo_relative(path: &str, scan_root: &Path, canon_repo: &Path) -> Option<String> {
    let abs = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        scan_root.join(path)
    };
    let abs = abs.canonicalize().unwrap_or(abs);
    let rel = abs.strip_prefix(canon_repo).ok()?;
    Some(normalize_path(&rel.to_string_lossy()))
}

/// Resolve merge-base with a usable default branch.
///
/// - Prefer a candidate whose merge-base differs from `HEAD` (PR ahead of base).
/// - If merge-base equals `HEAD` for a **default** base (`main`/`master` /
///   `GITHUB_BASE_REF` / …), HEAD is on/at that base → return `"HEAD"` so
///   reporting filters to uncommitted + untracked only (not a full backlog scan).
/// - If the only tip-equal refs are feature branches (shallow detached CI
///   without the PR base), bail → FullScan with a note.
fn default_merge_base(repo_root: &Path) -> anyhow::Result<String> {
    let head = git_stdout(repo_root, &["rev-parse", "HEAD"])
        .context("rev-parse HEAD")?
        .trim()
        .to_string();
    let mut last_err = None;
    let mut skipped_same_as_head = Vec::new();
    let candidates = candidate_base_refs(repo_root);
    if candidates.is_empty() {
        bail!(
            "no base branch found (tried GITHUB_BASE_REF, origin/HEAD, \
             origin/main|master, main|master); shallow or detached checkout?"
        );
    }
    for branch in &candidates {
        match git_stdout(repo_root, &["merge-base", "HEAD", branch]) {
            Ok(out) => {
                let mb = out.trim();
                if mb.is_empty() {
                    continue;
                }
                if mb == head {
                    skipped_same_as_head.push(branch.clone());
                    continue;
                }
                return Ok(mb.to_string());
            }
            Err(e) => last_err = Some(e),
        }
    }
    if !skipped_same_as_head.is_empty() {
        // On the default branch tip → uncommitted-only. Feature-only tips
        // (shallow feat clone) → FullScan.
        if skipped_same_as_head.iter().any(|b| is_default_base_ref(b)) {
            return Ok("HEAD".to_string());
        }
        bail!(
            "no usable base (merge-base equals HEAD for [{}]); \
             shallow or detached checkout without the PR base branch?",
            skipped_same_as_head.join(", ")
        );
    }
    match last_err {
        Some(e) => Err(e).context(format!(
            "git merge-base HEAD with candidates [{}]",
            candidates.join(", ")
        )),
        None => bail!(
            "could not determine a merge-base with [{}]",
            candidates.join(", ")
        ),
    }
}

/// True for default-branch-like refs (not feature branches).
fn is_default_base_ref(name: &str) -> bool {
    let base = name.rsplit('/').next().unwrap_or(name);
    if matches!(base, "main" | "master") {
        return true;
    }
    if let Ok(gbr) = std::env::var("GITHUB_BASE_REF") {
        let gbr = gbr.trim();
        if !gbr.is_empty() && (name == gbr || name == format!("origin/{gbr}") || base == gbr) {
            return true;
        }
    }
    false
}

fn candidate_base_refs(repo_root: &Path) -> Vec<String> {
    let mut out = Vec::new();

    // GitHub Actions: GITHUB_BASE_REF is the PR base branch name (e.g. "main").
    if let Ok(base) = std::env::var("GITHUB_BASE_REF") {
        let base = base.trim();
        if !base.is_empty() {
            for candidate in [format!("origin/{base}"), base.to_string()] {
                if git_stdout(repo_root, &["rev-parse", "--verify", &candidate]).is_ok() {
                    out.push(candidate);
                }
            }
        }
    }

    // Upstream of current branch (works in many CI checkouts).
    if let Ok(up) = git_stdout(
        repo_root,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    ) {
        let up = up.trim();
        if !up.is_empty() && up != "@{upstream}" {
            out.push(up.to_string());
        }
    }

    // origin/HEAD → typically origin/main.
    if let Ok(sym) = git_stdout(repo_root, &["symbolic-ref", "refs/remotes/origin/HEAD"]) {
        let s = sym.trim();
        if let Some(name) = s.strip_prefix("refs/remotes/") {
            out.push(name.to_string());
        } else if !s.is_empty() {
            out.push(s.to_string());
        }
    } else if let Ok(sym) = git_stdout(repo_root, &["rev-parse", "--abbrev-ref", "origin/HEAD"]) {
        let s = sym.trim();
        if !s.is_empty() && s != "origin/HEAD" {
            out.push(s.to_string());
        }
    }

    for candidate in ["origin/main", "origin/master", "main", "master"] {
        if git_stdout(repo_root, &["rev-parse", "--verify", candidate]).is_ok() {
            out.push(candidate.to_string());
        }
    }

    // Preserve insertion order (GITHUB_BASE_REF first) while deduping.
    let mut seen = HashSet::new();
    out.retain(|c| seen.insert(c.clone()));
    out
}

fn git_stdout(repo_root: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(["-c", "core.quotepath=false"])
        .args(args)
        .current_dir(repo_root)
        .output()
        .with_context(|| format!("spawn git {}", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(String::from_utf8(output.stdout)?)
}

/// Best-effort git top-level for a scan path (file or directory).
pub fn repo_root_for(path: &Path) -> anyhow::Result<PathBuf> {
    let dir = if path.is_file() {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let out = git_stdout(dir, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(out.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_doctor_core::{Entry, EntryLabel, Fix};

    fn finding(file: &str) -> Finding {
        Finding {
            id: "1".into(),
            rule: "blocking-sleep".into(),
            entry: Entry {
                file: file.into(),
                line: 1,
                name: "h".into(),
                label: EntryLabel::Other,
            },
            chain: vec![],
            conditions: vec![],
            fix: Fix {
                hint: "x".into(),
                fix_group_id: "g".into(),
            },
            why: "async-doctor why a.py:1".into(),
            docs_anchor: "#blocking-sleep".into(),
            docs_url: String::new(),
        }
    }

    #[test]
    fn filter_exact_repo_relative_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let backend = repo.join("backend");
        std::fs::create_dir_all(&backend).unwrap();
        std::fs::write(backend.join("app.py"), "").unwrap();
        std::fs::create_dir_all(repo.join("tools")).unwrap();
        std::fs::write(repo.join("tools/helpers.py"), "").unwrap();

        let mut report = Report::empty_complete();
        report.findings = vec![finding("app.py")]; // relative to backend/
        let changed = ChangedFiles::from([("tools/helpers.py".to_string(), ChangedLines::Whole)]);
        let filtered = filter_report(report, &changed, &backend, repo);
        assert!(
            filtered.findings.is_empty(),
            "tools/helpers.py must not match backend/app.py"
        );

        let mut report = Report::empty_complete();
        report.findings = vec![finding("app.py")];
        let changed = ChangedFiles::from([("backend/app.py".to_string(), ChangedLines::Whole)]);
        let filtered = filter_report(report, &changed, &backend, repo);
        assert_eq!(filtered.findings.len(), 1);
    }

    #[test]
    fn filter_rejects_basename_suffix_false_match() {
        // Repro: root pos.py changed; scan svc/; svc/app/pos.py must NOT match.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let svc = repo.join("svc/app");
        std::fs::create_dir_all(&svc).unwrap();
        std::fs::write(repo.join("pos.py"), "").unwrap();
        std::fs::write(svc.join("pos.py"), "").unwrap();

        let mut report = Report::empty_complete();
        report.findings = vec![finding("app/pos.py")]; // relative to svc/
        let changed = ChangedFiles::from([("pos.py".to_string(), ChangedLines::Whole)]);
        let filtered = filter_report(report, &changed, &repo.join("svc"), repo);
        assert!(
            filtered.findings.is_empty(),
            "root pos.py must not match svc/app/pos.py"
        );

        let mut report = Report::empty_complete();
        report.findings = vec![finding("app/pos.py")];
        let changed = ChangedFiles::from([("svc/app/pos.py".to_string(), ChangedLines::Whole)]);
        let filtered = filter_report(report, &changed, &repo.join("svc"), repo);
        assert_eq!(filtered.findings.len(), 1);
    }
}
