//! Public scan entry: extract facts, then grade with the builtin sink table.
//!
//! Warm path: when the whole-scan input fingerprint matches a cached report,
//! skip ty extract/analyze and return the prior [`Report`] unchanged.

use std::path::Path;
use std::time::Instant;

use async_doctor_core::{AnalyzeOptions, Report, analyze_with_options};
use async_doctor_rules::BuiltinSinkTable;

use crate::cache::effective_cache_root;
use crate::extract::{
    DEFAULT_FOLLOW_BUDGET, absolute_utf8, discover_python_and_stub_files, extract_discovered_files,
    extract_path_with_cache_budget,
};
use crate::scan_cache::{load_scan_report, save_scan_report, scan_input_fingerprint};
use crate::timing::PhaseTiming;

/// Scan a project directory or a single `.py` file with default options.
pub fn scan_path(path: &Path) -> anyhow::Result<Report> {
    scan_path_with_options(path, AnalyzeOptions::default())
}

/// Scan with explicit grading/reporting options (CLI flags).
pub fn scan_path_with_options(path: &Path, options: AnalyzeOptions) -> anyhow::Result<Report> {
    scan_path_with_options_and_cache(path, options, None)
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
    scan_path_with_options_cache_budget(path, options, cache_dir, DEFAULT_FOLLOW_BUDGET)
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
    scan_path_with_options_cache_budget(path, AnalyzeOptions::default(), cache_dir, follow_budget)
}

fn scan_path_with_options_cache_budget(
    path: &Path,
    options: AnalyzeOptions,
    cache_dir: Option<&Path>,
    follow_budget: usize,
) -> anyhow::Result<Report> {
    let mut timing = PhaseTiming::from_env();
    let cache_root = effective_cache_root(cache_dir);

    let t_disc = Instant::now();
    let abs = absolute_utf8(path)?;
    let (fingerprint_root, fingerprint_files) = if abs.as_std_path().is_file() {
        let parent = abs
            .parent()
            .map(|p| p.as_std_path().to_path_buf())
            .unwrap_or_else(|| Path::new(".").to_path_buf());
        (parent, vec![abs.as_std_path().to_path_buf()])
    } else {
        let root = abs.as_std_path().to_path_buf();
        // Fingerprint .py and .pyi in a single walk (same ignore rules).
        // Extract still walks only .py; stubs affect ty resolution via the filesystem.
        let files = discover_python_and_stub_files(&root)?;
        (root, files)
    };
    timing.span("discovery", t_disc);

    let t_fp = Instant::now();
    let fingerprint = scan_input_fingerprint(
        &fingerprint_root,
        &fingerprint_files,
        options,
        follow_budget,
    );
    timing.span("fingerprint", t_fp);

    let t_load = Instant::now();
    if let Some(mut report) = load_scan_report(&cache_root, &fingerprint) {
        timing.span("scan_cache_load", t_load);
        // Recompute live origin/prefix/interpreter so VIRTUAL_ENV vs local
        // `.venv` is not sticky from a prior cache write; keep third-party
        // counts from the cached analysis and refresh the advisory note.
        refresh_environment_on_cache_hit(&mut report, &fingerprint_root);
        timing.finish("scan_cache=hit");
        return Ok(report);
    }
    timing.span("scan_cache_miss", t_load);

    let t_extract = Instant::now();
    let facts = if abs.as_std_path().is_file() {
        extract_path_with_cache_budget(path, cache_dir, follow_budget)?
    } else {
        let py_files: Vec<std::path::PathBuf> = fingerprint_files
            .into_iter()
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("py"))
            .collect();
        extract_discovered_files(&abs, py_files, cache_dir, follow_budget)?
    };
    timing.span("extract", t_extract);

    let t_analyze = Instant::now();
    let report = analyze_with_options(&facts, &BuiltinSinkTable::new(), options);
    timing.span("analyze", t_analyze);

    let t_save = Instant::now();
    let _ = save_scan_report(&cache_root, &fingerprint, &report);
    timing.span("scan_cache_save", t_save);
    timing.finish("scan_cache=miss");
    Ok(report)
}

fn refresh_environment_on_cache_hit(report: &mut Report, scan_root: &Path) {
    let Ok(root) = ruff_db::system::SystemPathBuf::from_path_buf(scan_root.to_path_buf()) else {
        return;
    };
    let Ok(mut live) = crate::env_info::discover_scan_environment(&root) else {
        return;
    };
    live.unresolved_third_party = report.environment.unresolved_third_party;
    live.resolved_third_party = report.environment.resolved_third_party;
    live.refresh_note();
    report.environment = live;
}
