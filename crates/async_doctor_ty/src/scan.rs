//! Public scan entry: extract facts, then grade with the builtin sink table.
//!
//! Warm path: when the whole-scan input fingerprint matches a cached report,
//! skip ty extract/analyze and return the prior [`Report`] unchanged.

use std::path::{Path, PathBuf};
use std::time::Instant;

use async_doctor_core::{AnalyzeOptions, Report, analyze_owned};
use async_doctor_rules::BuiltinSinkTable;

use crate::cache::effective_cache_root;
use crate::extract::{
    DEFAULT_FOLLOW_BUDGET, absolute_utf8, discover_python_and_stub_files, extract_discovered_files,
    extract_path_with_cache_budget,
};
use crate::scan_cache::{load_scan_report, save_scan_report, scan_input_fingerprint_with_python};
use crate::timing::PhaseTiming;

/// Runtime options for scanning a Python file or project directory.
#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    pub analyze: AnalyzeOptions,
    /// Explicit Python interpreter or environment prefix (`--python <PATH>`).
    pub python: Option<PathBuf>,
    /// Explicit third-party and whole-scan cache directory override.
    pub cache_dir: Option<PathBuf>,
}

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
    scan_path_with_options_cache_budget(path, options, None, cache_dir, DEFAULT_FOLLOW_BUDGET)
}

/// Scan with unified [`ScanOptions`] (including `--python` and optional `cache_dir`).
pub fn scan_path_with_scan_options(path: &Path, options: &ScanOptions) -> anyhow::Result<Report> {
    scan_path_with_options_cache_budget(
        path,
        options.analyze,
        options.python.as_deref(),
        options.cache_dir.as_deref(),
        DEFAULT_FOLLOW_BUDGET,
    )
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
    scan_path_with_options_cache_budget(
        path,
        AnalyzeOptions::default(),
        None,
        cache_dir,
        follow_budget,
    )
}

/// Test seam: count `fresh_empty` calls for one scan under the hermetic env
/// lock so parallel `cargo test` threads cannot race the process-global
/// counter.
#[cfg(test)]
pub(crate) fn scan_path_counting_fresh_empty(path: &Path) -> anyhow::Result<(Report, usize)> {
    crate::hermetic_env::with_clean_venv_env(|| {
        crate::db::AsyncDoctorDb::reset_fresh_empty_count();
        let report = scan_path_with_options_cache_budget_inner(
            path,
            AnalyzeOptions::default(),
            None,
            None,
            DEFAULT_FOLLOW_BUDGET,
        )?;
        Ok((report, crate::db::AsyncDoctorDb::fresh_empty_count()))
    })
}

fn scan_path_with_options_cache_budget(
    path: &Path,
    options: AnalyzeOptions,
    python: Option<&Path>,
    cache_dir: Option<&Path>,
    follow_budget: usize,
) -> anyhow::Result<Report> {
    #[cfg(test)]
    {
        crate::hermetic_env::with_clean_venv_env(|| {
            scan_path_with_options_cache_budget_inner(
                path,
                options,
                python,
                cache_dir,
                follow_budget,
            )
        })
    }
    #[cfg(not(test))]
    {
        scan_path_with_options_cache_budget_inner(path, options, python, cache_dir, follow_budget)
    }
}

fn scan_path_with_options_cache_budget_inner(
    path: &Path,
    options: AnalyzeOptions,
    python: Option<&Path>,
    cache_dir: Option<&Path>,
    follow_budget: usize,
) -> anyhow::Result<Report> {
    let mut timing = PhaseTiming::from_env();
    let cache_root = effective_cache_root(cache_dir);

    let t_disc = Instant::now();
    let abs = absolute_utf8(path)?;
    if abs.as_std_path().is_file() {
        timing.span("discovery", t_disc);
        let t_extract = Instant::now();
        let facts = extract_path_with_cache_budget(path, python, cache_dir, follow_budget)?;
        timing.span("extract", t_extract);
        let t_analyze = Instant::now();
        let report = analyze_owned(facts, &BuiltinSinkTable::new(), options);
        timing.span("analyze", t_analyze);
        timing.finish("scan_cache=bypass_single_file");
        return Ok(report);
    }

    let fingerprint_root = abs.as_std_path().to_path_buf();
    // Fingerprint .py and .pyi in a single walk (same ignore rules).
    // Extract still walks only .py; stubs affect ty resolution via the filesystem.
    let fingerprint_files = discover_python_and_stub_files(&fingerprint_root)?;
    timing.span("discovery", t_disc);

    let t_fp = Instant::now();
    let fp = scan_input_fingerprint_with_python(
        &fingerprint_root,
        &fingerprint_files,
        options,
        follow_budget,
        python,
    );
    timing.span("fingerprint", t_fp);

    let t_load = Instant::now();
    if let Some(mut report) = load_scan_report(&cache_root, &fp.key) {
        timing.span("scan_cache_load", t_load);
        // Recompute live origin/prefix/interpreter so VIRTUAL_ENV vs local
        // `.venv` is not sticky from a prior cache write; keep third-party
        // counts from the cached analysis and refresh the advisory note.
        refresh_environment_on_cache_hit(&mut report, &fingerprint_root, python)?;
        report.ignore_candidate_files = Some(fp.ignore_candidate_files);
        timing.finish("scan_cache=hit");
        return Ok(report);
    }
    timing.span("scan_cache_miss", t_load);

    let t_extract = Instant::now();
    let py_files: Vec<std::path::PathBuf> = fingerprint_files
        .into_iter()
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("py"))
        .collect();
    let facts = extract_discovered_files(&abs, py_files, python, cache_dir, follow_budget)?;
    timing.span("extract", t_extract);

    let t_analyze = Instant::now();
    let mut report = analyze_owned(facts, &BuiltinSinkTable::new(), options);
    timing.span("analyze", t_analyze);

    if report.complete {
        let t_save = Instant::now();
        let _ = save_scan_report(&cache_root, &fp.key, &report);
        timing.span("scan_cache_save", t_save);
    }
    report.ignore_candidate_files = Some(fp.ignore_candidate_files);
    timing.finish("scan_cache=miss");
    Ok(report)
}

fn refresh_environment_on_cache_hit(
    report: &mut Report,
    scan_root: &Path,
    python: Option<&Path>,
) -> anyhow::Result<()> {
    let Ok(root) = ruff_db::system::SystemPathBuf::from_path_buf(scan_root.to_path_buf()) else {
        return Ok(());
    };
    let sys_python = python.map(absolute_utf8).transpose()?;
    let live_res =
        crate::env_info::discover_scan_environment_with_python(&root, sys_python.as_deref());
    let mut live = match live_res {
        Ok(live) => live,
        Err(err) if python.is_some() => return Err(err),
        Err(_) => return Ok(()),
    };
    live.unresolved_third_party = report.environment.unresolved_third_party;
    live.resolved_third_party = report.environment.resolved_third_party;
    live.unresolved_third_party_modules = report.environment.unresolved_third_party_modules.clone();
    live.follow_budget_skipped = report.environment.follow_budget_skipped;
    live.refresh_note();
    report.environment = live;
    Ok(())
}
