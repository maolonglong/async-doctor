//! `--changed` reporting filter: full analysis, narrow reporting.
//!
//! Changed paths come from git (merge-base with the default branch by default,
//! plus uncommitted work). A finding is reported when any hop in its evidence
//! chain (or its entry) touches a changed project file.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use async_doctor_core::{Candidate, Finding, Report};

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
    // Untracked files (agent edits often land here before the first commit).
    let untracked = git_stdout(&repo_root, &["ls-files", "--others", "--exclude-standard"])?;
    for line in untracked.lines() {
        let line = line.trim();
        if !line.is_empty() {
            files.insert(normalize_rel(line));
        }
    }
    Ok(files)
}

/// Keep findings/candidates whose entry or chain touches a changed file.
pub fn filter_report(mut report: Report, changed: &HashSet<String>) -> Report {
    report.findings.retain(|f| finding_touches(f, changed));
    report.candidates.retain(|c| candidate_touches(c, changed));
    report
}

fn finding_touches(f: &Finding, changed: &HashSet<String>) -> bool {
    path_in_changed(&f.entry.file, changed)
        || f.chain.iter().any(|s| path_in_changed(&s.file, changed))
}

fn candidate_touches(c: &Candidate, changed: &HashSet<String>) -> bool {
    path_in_changed(&c.entry.file, changed)
        || c.chain.iter().any(|s| path_in_changed(&s.file, changed))
}

fn path_in_changed(path: &str, changed: &HashSet<String>) -> bool {
    let norm = normalize_rel(path);
    if changed.contains(&norm) {
        return true;
    }
    // Match on basename / suffix so report-relative paths align with git paths.
    changed
        .iter()
        .any(|c| c == &norm || c.ends_with(&norm) || norm.ends_with(c))
}

fn default_merge_base(repo_root: &Path) -> anyhow::Result<String> {
    let default_branch = default_branch_ref(repo_root)?;
    let out = git_stdout(repo_root, &["merge-base", "HEAD", &default_branch])?;
    let mb = out.trim();
    if mb.is_empty() {
        bail!("git merge-base with {default_branch} returned empty");
    }
    Ok(mb.to_string())
}

fn default_branch_ref(repo_root: &Path) -> anyhow::Result<String> {
    // Prefer origin/HEAD → origin/<branch>, then common local names.
    if let Ok(out) = git_stdout(repo_root, &["symbolic-ref", "refs/remotes/origin/HEAD"]) {
        let s = out.trim();
        if let Some(name) = s.strip_prefix("refs/remotes/") {
            return Ok(name.to_string());
        }
        if !s.is_empty() {
            return Ok(s.to_string());
        }
    }
    for candidate in ["origin/main", "origin/master", "main", "master"] {
        if git_stdout(repo_root, &["rev-parse", "--verify", candidate]).is_ok() {
            return Ok(candidate.to_string());
        }
    }
    bail!("could not determine default branch (tried origin/HEAD, main, master)")
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
            into.insert(normalize_rel(line));
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

fn normalize_rel(p: &str) -> String {
    p.replace('\\', "/").trim_start_matches("./").to_string()
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
        }
    }

    #[test]
    fn filter_keeps_only_touched_files() {
        let mut report = Report::empty_complete();
        report.findings = vec![finding("app.py"), finding("other.py")];
        let changed = HashSet::from(["app.py".into()]);
        let filtered = filter_report(report, &changed);
        assert_eq!(filtered.findings.len(), 1);
        assert_eq!(filtered.findings[0].entry.file, "app.py");
    }
}
