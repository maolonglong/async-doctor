//! Extract first-party call-graph facts using ty's semantic model.
//!
//! All ty/ruff usage stays in this crate. Core sees only plain [`ProjectFacts`].

mod collector;
mod constants;
mod ctx;
mod files;
mod follow;
mod functions;
mod helpers;
mod panic_util;
mod pass2;
mod path;

use std::path::Path;

use async_doctor_core::ProjectFacts;
use ruff_db::system::{SystemPath, SystemPathBuf};

use files::extract_files;

pub use path::{discover_python_files, discover_python_stub_files};

pub(crate) use follow::DEFAULT_FOLLOW_BUDGET;
pub(crate) use path::absolute_utf8;

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
    extract_path_with_cache_budget(path, cache_dir, DEFAULT_FOLLOW_BUDGET)
}

/// Like [`extract_path_with_cache`], but with an injectable third-party follow
/// budget (test seam only — not a user-facing config).
pub(crate) fn extract_path_with_cache_budget(
    path: &Path,
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
            cache_dir,
            follow_budget,
        );
    }

    // Only walk the scan root for first-party entries. Outside editable roots
    // stay on the search path and are followed like site-packages when called.
    let files = discover_python_files(path.as_std_path())?;
    if files.is_empty() {
        return Ok(ProjectFacts {
            complete: true,
            no_python_files: true,
            ..ProjectFacts::default()
        });
    }
    extract_files(&path, files, cache_dir, follow_budget)
}
