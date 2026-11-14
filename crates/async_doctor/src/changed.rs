//! `--changed` reporting filter: full analysis when Python/config changed,
//! fast short-circuit when zero Python/config files changed.
//!
//! Changed paths are always repo-root-relative. Finding paths (relative to the
//! scan root) are normalized to the same basis before **exact** comparison —
//! so scanning `svc/` while only root `pos.py` changed does not match
//! `svc/app/pos.py`.

mod git;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

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
    fn overlaps(&self, start: u32, end: u32) -> bool {
        match self {
            ChangedLines::Whole => true,
            ChangedLines::Ranges(rs) => rs.iter().any(|(a, b)| *a <= end && start <= *b),
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
pub fn changed_files(repo_root: &Path, git_ref: Option<&str>) -> anyhow::Result<ChangedFiles> {
    git::collect_changed_files(repo_root, git_ref)
}

/// Best-effort git top-level for a scan path (file or directory).
pub fn repo_root_for(path: &Path) -> anyhow::Result<PathBuf> {
    git::find_repo_root(path)
}

/// True when any changed path is a `.py`/`.pyi` file or a Python project/lock/config
/// file that could affect module resolution or analysis.
pub fn has_python_or_config_changes(files: &ChangedFiles) -> bool {
    files.keys().any(|p| is_python_or_config_path(p))
}

fn is_python_or_config_path(rel_path: &str) -> bool {
    let path = Path::new(rel_path);
    if let Some(ext) = path.extension().and_then(|e| e.to_str())
        && (ext.eq_ignore_ascii_case("py") || ext.eq_ignore_ascii_case("pyi"))
    {
        return true;
    }
    let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = file_name.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "pyproject.toml"
            | "ruff.toml"
            | ".ruff.toml"
            | "pyrightconfig.json"
            | "uv.lock"
            | "poetry.lock"
            | "pdm.lock"
            | "pipfile"
            | "pipfile.lock"
            | "setup.cfg"
    ) {
        return true;
    }
    if lower.starts_with("requirements") && lower.ends_with(".txt") {
        return true;
    }
    lower.ends_with(".txt")
        && path.parent().is_some_and(|parent| {
            parent.components().any(|c| {
                c.as_os_str()
                    .to_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case("requirements"))
            })
        })
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
    let item_spans = &report.item_spans;
    report.findings.retain(|f| {
        finding_touches(
            f,
            item_spans,
            changed,
            scan_root,
            &canon_repo,
            &mut rel_cache,
        )
    });
    report.candidates.retain(|c| {
        candidate_touches(
            c,
            item_spans,
            changed,
            scan_root,
            &canon_repo,
            &mut rel_cache,
        )
    });
    report.advice.retain(|a| {
        advice_touches(
            a,
            item_spans,
            changed,
            scan_root,
            &canon_repo,
            &mut rel_cache,
        )
    });
    let retained_ids: HashSet<&str> = report
        .findings
        .iter()
        .map(|f| f.id.as_str())
        .chain(report.candidates.iter().map(|c| c.id.as_str()))
        .chain(report.advice.iter().map(|a| a.id.as_str()))
        .collect();
    let retained_owned: HashSet<String> = retained_ids.into_iter().map(str::to_string).collect();
    report
        .item_spans
        .retain(|id, _| retained_owned.contains(id));
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
    item_spans: &BTreeMap<String, Vec<(String, u32, u32)>>,
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
        || spans_touch(&f.id, item_spans, changed, scan_root, canon_repo, rel_cache)
}

fn candidate_touches(
    c: &Candidate,
    item_spans: &BTreeMap<String, Vec<(String, u32, u32)>>,
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
        || spans_touch(&c.id, item_spans, changed, scan_root, canon_repo, rel_cache)
}

fn advice_touches(
    a: &Advice,
    item_spans: &BTreeMap<String, Vec<(String, u32, u32)>>,
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
        || spans_touch(&a.id, item_spans, changed, scan_root, canon_repo, rel_cache)
}

fn spans_touch(
    id: &str,
    item_spans: &BTreeMap<String, Vec<(String, u32, u32)>>,
    changed: &ChangedFiles,
    scan_root: &Path,
    canon_repo: &Path,
    rel_cache: &mut HashMap<String, Option<String>>,
) -> bool {
    let Some(spans) = item_spans.get(id) else {
        return false;
    };
    spans.iter().any(|(file, start, end)| {
        span_in_changed(
            file, *start, *end, changed, scan_root, canon_repo, rel_cache,
        )
    })
}

fn loc_in_changed(
    path: &str,
    line: u32,
    changed: &ChangedFiles,
    scan_root: &Path,
    canon_repo: &Path,
    rel_cache: &mut HashMap<String, Option<String>>,
) -> bool {
    span_in_changed(path, line, line, changed, scan_root, canon_repo, rel_cache)
}

fn span_in_changed(
    path: &str,
    start: u32,
    end: u32,
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
    changed
        .get(repo_rel)
        .is_some_and(|r| r.overlaps(start, end))
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
    fn python_or_config_detection_matches_expected_files() {
        assert!(is_python_or_config_path("src/app.py"));
        assert!(is_python_or_config_path("stubs/api.pyi"));
        assert!(is_python_or_config_path("pyproject.toml"));
        assert!(is_python_or_config_path("sub/ruff.toml"));
        assert!(is_python_or_config_path(".ruff.toml"));
        assert!(is_python_or_config_path("pyrightconfig.json"));
        assert!(is_python_or_config_path("uv.lock"));
        assert!(is_python_or_config_path("poetry.lock"));
        assert!(is_python_or_config_path("requirements-dev.txt"));
        assert!(is_python_or_config_path("requirements/prod.txt"));
        assert!(!is_python_or_config_path("README.md"));
        assert!(!is_python_or_config_path("docs/notes.txt"));
        assert!(!is_python_or_config_path("frontend/app.tsx"));
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
        report.findings = vec![finding("app.py")];
        let changed = ChangedFiles::from([("tools/helpers.py".to_string(), ChangedLines::Whole)]);
        let filtered = filter_report(report, &changed, &backend, repo);
        assert!(filtered.findings.is_empty());

        let mut report = Report::empty_complete();
        report.findings = vec![finding("app.py")];
        let changed = ChangedFiles::from([("backend/app.py".to_string(), ChangedLines::Whole)]);
        let filtered = filter_report(report, &changed, &backend, repo);
        assert_eq!(filtered.findings.len(), 1);
    }

    #[test]
    fn filter_rejects_basename_suffix_false_match() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let svc = repo.join("svc/app");
        std::fs::create_dir_all(&svc).unwrap();
        std::fs::write(repo.join("pos.py"), "").unwrap();
        std::fs::write(svc.join("pos.py"), "").unwrap();

        let mut report = Report::empty_complete();
        report.findings = vec![finding("app/pos.py")];
        let changed = ChangedFiles::from([("pos.py".to_string(), ChangedLines::Whole)]);
        let filtered = filter_report(report, &changed, &repo.join("svc"), repo);
        assert!(filtered.findings.is_empty());

        let mut report = Report::empty_complete();
        report.findings = vec![finding("app/pos.py")];
        let changed = ChangedFiles::from([("svc/app/pos.py".to_string(), ChangedLines::Whole)]);
        let filtered = filter_report(report, &changed, &repo.join("svc"), repo);
        assert_eq!(filtered.findings.len(), 1);
    }
}
