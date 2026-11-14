//! Git subprocess diffing, hunk-range parsing, and merge-base discovery for `--changed`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

use crate::paths::normalize_path;

use super::{ChangedFiles, ChangedLines};

/// Resolve changed paths and NEW-side line ranges relative to `repo_root`.
///
/// `git_ref` is `None` for the default (merge-base with the default branch).
pub(super) fn collect_changed_files(
    repo_root: &Path,
    git_ref: Option<&str>,
) -> anyhow::Result<ChangedFiles> {
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
    for diff_args in [
        &[
            "diff",
            "--no-ext-diff",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "-U0",
            "--diff-filter=ACMR",
            base.as_str(),
        ][..],
        &[
            "diff",
            "--no-ext-diff",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "-U0",
            "--diff-filter=ACMR",
        ][..],
        &[
            "diff",
            "--no-ext-diff",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "-U0",
            "--diff-filter=ACMR",
            "--cached",
        ][..],
    ] {
        collect_diff_ranges(&repo_root, diff_args, &mut files)?;
    }
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

/// Resolve merge-base with a usable default branch.
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

    let mut seen = HashSet::new();
    out.retain(|c| seen.insert(c.clone()));
    out
}

pub(super) fn git_stdout(repo_root: &Path, args: &[&str]) -> anyhow::Result<String> {
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
pub(super) fn find_repo_root(path: &Path) -> anyhow::Result<PathBuf> {
    let dir = if path.is_file() {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let out = git_stdout(dir, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(out.trim()))
}
