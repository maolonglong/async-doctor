//! Public scan entry: extract facts, then grade with the builtin sink table.

use std::path::Path;

use async_doctor_core::{AnalyzeOptions, Report, analyze_with_options};
use async_doctor_rules::BuiltinSinkTable;

#[cfg(test)]
use crate::extract::extract_path_with_cache_budget;
use crate::extract::{extract_path, extract_path_with_cache};

/// Scan a project directory or a single `.py` file with default options.
pub fn scan_path(path: &Path) -> anyhow::Result<Report> {
    scan_path_with_options(path, AnalyzeOptions::default())
}

/// Scan with explicit grading/reporting options (CLI flags).
pub fn scan_path_with_options(path: &Path, options: AnalyzeOptions) -> anyhow::Result<Report> {
    let facts = extract_path(path)?;
    Ok(analyze_with_options(
        &facts,
        &BuiltinSinkTable::new(),
        options,
    ))
}

/// Scan with an explicit third-party package cache directory.
///
/// Tests must pass an isolated `cache_dir` instead of mutating process-wide
/// `XDG_CACHE_HOME` (races under parallel `cargo test`).
pub fn scan_path_with_cache(path: &Path, cache_dir: Option<&Path>) -> anyhow::Result<Report> {
    scan_path_with_options_and_cache(path, AnalyzeOptions::default(), cache_dir)
}

/// Scan with grading options and an explicit cache directory.
pub fn scan_path_with_options_and_cache(
    path: &Path,
    options: AnalyzeOptions,
    cache_dir: Option<&Path>,
) -> anyhow::Result<Report> {
    let facts = extract_path_with_cache(path, cache_dir)?;
    Ok(analyze_with_options(
        &facts,
        &BuiltinSinkTable::new(),
        options,
    ))
}

/// Scan with an explicit cache directory and injectable follow budget.
///
/// Test seam only — production callers use [`scan_path_with_cache`] (default
/// budget). Avoids process-global env mutation under parallel `cargo test`.
#[cfg(test)]
pub(crate) fn scan_path_with_cache_budget(
    path: &Path,
    cache_dir: Option<&Path>,
    follow_budget: usize,
) -> anyhow::Result<Report> {
    let facts = extract_path_with_cache_budget(path, cache_dir, follow_budget)?;
    Ok(analyze_with_options(
        &facts,
        &BuiltinSinkTable::new(),
        AnalyzeOptions::default(),
    ))
}
