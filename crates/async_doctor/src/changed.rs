//! `--changed` reporting filter: full analysis, narrow reporting.
//!
//! Changed paths are always repo-root-relative. Finding paths (relative to the
//! scan root) are normalized to the same basis before comparison — so scanning
//! `backend/` while `tools/helpers.py` changed does not false-match.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use async_doctor_core::{Candidate, Finding, Report};

use crate::paths::{normalize_path, paths_equal};

/// Outcome of resolving the changed-file set for `--changed`.
#[derive(Debug)]
pub enum ChangedSet {
    /// Filter reporting to these repo-root-relative paths.
    Filter {
        files: HashSet<String>,
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

/// Resolve the set of changed paths relative to `repo_root`.
///
/// `git_ref` is `None` for the default (merge-base with the default branch).
pub fn changed_files(repo_root: &Path, git_ref: Option<&str>) -> anyhow::Result<HashSet<String>> {
    let repo_root = repo_root
        .canonicalize()
        .with_context(|| format!("canonicalize {}", repo_root.display()))?;

    let base = match git_ref {
        Some(r) if !r.is_empty() && r != "AUTO" => r.to_string(),
        _ => default_merge_base(&repo_root)?,
    };

    let mut files = HashSet::new();
    // Committed + unstaged + staged differences vs base.
    collect_diff_names(
        &repo_root,
        &["diff", "--name-only", "--diff-filter=ACMR", &base],
        &mut files,
    )?;
    collect_diff_names(
        &repo_root,
        &["diff", "--name-only", "--diff-filter=ACMR"],
        &mut files,
    )?;
    collect_diff_names(
        &repo_root,
        &["diff", "--name-only", "--cached", "--diff-filter=ACMR"],
        &mut files,
    )?;
    let untracked = git_stdout(&repo_root, &["ls-files", "--others", "--exclude-standard"])?;
    for line in untracked.lines() {
        let line = line.trim();
        if !line.is_empty() {
            files.insert(normalize_path(line));
        }
    }
    Ok(files)
}

/// Keep findings/candidates whose entry or chain touches a changed file.
///
/// `scan_root` is the directory that finding paths are relative to.
/// `changed` / `repo_root` use repo-root-relative paths.
pub fn filter_report(
    mut report: Report,
    changed: &HashSet<String>,
    scan_root: &Path,
    repo_root: &Path,
) -> Report {
    report
        .findings
        .retain(|f| finding_touches(f, changed, scan_root, repo_root));
    report
        .candidates
        .retain(|c| candidate_touches(c, changed, scan_root, repo_root));
    report
}

fn finding_touches(
    f: &Finding,
    changed: &HashSet<String>,
    scan_root: &Path,
    repo_root: &Path,
) -> bool {
    path_in_changed(&f.entry.file, changed, scan_root, repo_root)
        || f.chain
            .iter()
            .any(|s| path_in_changed(&s.file, changed, scan_root, repo_root))
}

fn candidate_touches(
    c: &Candidate,
    changed: &HashSet<String>,
    scan_root: &Path,
    repo_root: &Path,
) -> bool {
    path_in_changed(&c.entry.file, changed, scan_root, repo_root)
        || c.chain
            .iter()
            .any(|s| path_in_changed(&s.file, changed, scan_root, repo_root))
}

fn path_in_changed(
    path: &str,
    changed: &HashSet<String>,
    scan_root: &Path,
    repo_root: &Path,
) -> bool {
    let Some(repo_rel) = to_repo_relative(path, scan_root, repo_root) else {
        return false;
    };
    changed.iter().any(|c| paths_equal(c, &repo_rel))
}

/// Convert a finding path (relative to `scan_root`) into a repo-root-relative path.
fn to_repo_relative(path: &str, scan_root: &Path, repo_root: &Path) -> Option<String> {
    let abs = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        scan_root.join(path)
    };
    let abs = abs.canonicalize().unwrap_or(abs);
    let repo_root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let rel = abs.strip_prefix(&repo_root).ok()?;
    Some(normalize_path(&rel.to_string_lossy()))
}

fn default_merge_base(repo_root: &Path) -> anyhow::Result<String> {
    let mut last_err = None;
    for branch in candidate_base_refs(repo_root) {
        match git_stdout(repo_root, &["merge-base", "HEAD", &branch]) {
            Ok(out) => {
                let mb = out.trim();
                if !mb.is_empty() {
                    return Ok(mb.to_string());
                }
            }
            Err(e) => last_err = Some(e),
        }
    }
    // Fetch-free fallback: diff against HEAD itself (uncommitted only).
    if git_stdout(repo_root, &["rev-parse", "--verify", "HEAD"]).is_ok() {
        return Ok("HEAD".to_string());
    }
    match last_err {
        Some(e) => Err(e).context("git merge-base"),
        None => bail!("could not determine a merge-base (no default branch, no HEAD)"),
    }
}

fn candidate_base_refs(repo_root: &Path) -> Vec<String> {
    let mut out = Vec::new();
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
    if let Ok(sym) = git_stdout(repo_root, &["symbolic-ref", "refs/remotes/origin/HEAD"]) {
        let s = sym.trim();
        if let Some(name) = s.strip_prefix("refs/remotes/") {
            out.push(name.to_string());
        } else if !s.is_empty() {
            out.push(s.to_string());
        }
    }
    for candidate in ["origin/main", "origin/master", "main", "master"] {
        if git_stdout(repo_root, &["rev-parse", "--verify", candidate]).is_ok() {
            out.push(candidate.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn collect_diff_names(
    repo_root: &Path,
    args: &[&str],
    into: &mut HashSet<String>,
) -> anyhow::Result<()> {
    let out = git_stdout(repo_root, args)?;
    for line in out.lines() {
        let line = line.trim();
        if !line.is_empty() {
            into.insert(normalize_path(line));
        }
    }
    Ok(())
}

fn git_stdout(repo_root: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git")
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
        let changed = HashSet::from(["tools/helpers.py".into()]);
        let filtered = filter_report(report, &changed, &backend, repo);
        assert!(
            filtered.findings.is_empty(),
            "tools/helpers.py must not match backend/app.py"
        );

        let mut report = Report::empty_complete();
        report.findings = vec![finding("app.py")];
        let changed = HashSet::from(["backend/app.py".into()]);
        let filtered = filter_report(report, &changed, &backend, repo);
        assert_eq!(filtered.findings.len(), 1);
    }
}
