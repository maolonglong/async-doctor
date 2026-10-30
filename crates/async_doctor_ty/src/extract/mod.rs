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
mod path;

use std::path::Path;

use async_doctor_core::ProjectFacts;
use ruff_db::system::{SystemPath, SystemPathBuf};

use crate::db::AsyncDoctorDb;
use crate::sitepkgs::clear_package_meta_memo;

use files::extract_files;
use path::{absolute_utf8, discover_python_files};

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
    clear_package_meta_memo();
    let path = absolute_utf8(path)?;
    if path.as_std_path().is_file() {
        if path.extension() != Some("py") {
            anyhow::bail!("`{path}` is not a Python (`.py`) file");
        }
        let parent = path
            .parent()
            .map(SystemPath::to_path_buf)
            .unwrap_or_else(|| SystemPathBuf::from("."));
        let db = AsyncDoctorDb::open(&parent, None)?;
        return extract_files(&db, vec![path.as_std_path().to_path_buf()], cache_dir);
    }

    let db = AsyncDoctorDb::open(&path, None)?;
    let files = discover_python_files(db.project_root().as_std_path())?;
    extract_files(&db, files, cache_dir)
}
