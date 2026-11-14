//! Extract first-party call-graph facts using ty's semantic model.
//!
//! All ty/ruff usage stays in this crate. Core sees only plain [`ProjectFacts`].

mod collector;
mod constants;
mod ctx;
mod files;
pub(crate) mod follow;
mod functions;
mod helpers;
mod panic_util;
pub(crate) mod pass2;
mod path;

use std::path::Path;

use async_doctor_core::ProjectFacts;
use ruff_db::system::{SystemPath, SystemPathBuf};

use files::extract_files;

pub use path::discover_python_files;

pub(crate) use follow::DEFAULT_FOLLOW_BUDGET;
pub(crate) use path::{absolute_utf8, discover_python_and_stub_files};

/// Default third-party follow budget (test seam — not public API).
#[cfg(test)]
pub(crate) const FOLLOW_BUDGET_DEFAULT: usize = DEFAULT_FOLLOW_BUDGET;

/// Scan a project directory or a single `.py` file into graded facts.
pub fn extract_path(path: &Path) -> anyhow::Result<ProjectFacts> {
    extract_path_with_cache(path, None)
}

/// Extract call-graph facts, writing/reading the third-party package cache under
/// `cache_dir` when provided (otherwise the user default cache root).
pub fn extract_path_with_cache(
    path: &Path,
    cache_dir: Option<&Path>,
) -> anyhow::Result<ProjectFacts> {
    extract_path_with_cache_budget(path, None, cache_dir, DEFAULT_FOLLOW_BUDGET)
}

/// Like [`extract_path_with_cache`], but with an injectable third-party follow
/// budget and optional `--python` path (test/internal seam).
pub(crate) fn extract_path_with_cache_budget(
    path: &Path,
    python: Option<&Path>,
    cache_dir: Option<&Path>,
    follow_budget: usize,
) -> anyhow::Result<ProjectFacts> {
    let path = absolute_utf8(path)?;
    if path.as_std_path().is_file() {
        if path.extension() != Some("py") {
            anyhow::bail!("`{path}` is not a Python (`.py`) file");
        }
        let parent = path
            .parent()
            .map(SystemPath::to_path_buf)
            .unwrap_or_else(|| SystemPathBuf::from("."));
        return extract_files(
            &parent,
            vec![path.as_std_path().to_path_buf()],
            python,
            cache_dir,
            follow_budget,
        );
    }

    // Only walk the scan root for first-party entries. Outside editable roots
    // stay on the search path and are followed like site-packages when called.
    let files = discover_python_files(path.as_std_path())?;
    extract_discovered_files(&path, files, python, cache_dir, follow_budget)
}

pub(crate) fn extract_discovered_files(
    path: &SystemPath,
    files: Vec<std::path::PathBuf>,
    python: Option<&Path>,
    cache_dir: Option<&Path>,
    follow_budget: usize,
) -> anyhow::Result<ProjectFacts> {
    if files.is_empty() {
        let sys_python = python
            .map(crate::env_discovery::absolute_python_path)
            .transpose()?;
        // Discover before returning so cold empty scans match warm cache hits
        // (which refresh `environment` via `discover_scan_environment_with_python`).
        let environment =
            crate::env_info::discover_scan_environment_with_python(path, sys_python.as_deref())?;
        return Ok(ProjectFacts {
            complete: true,
            no_python_files: true,
            environment,
            ..ProjectFacts::default()
        });
    }
    extract_files(path, files, python, cache_dir, follow_budget)
}
