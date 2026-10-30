//! Public scan entry: extract facts, then grade with the stdlib sink table.

use std::path::Path;

use async_doctor_core::{AnalyzeOptions, Report, analyze_with_options};
use async_doctor_rules::StdlibSinkTable;

use crate::extract::extract_path;

/// Scan a project directory or a single `.py` file with default options.
pub fn scan_path(path: &Path) -> anyhow::Result<Report> {
    scan_path_with_options(path, AnalyzeOptions::default())
}

/// Scan with explicit grading/reporting options (CLI flags).
pub fn scan_path_with_options(path: &Path, options: AnalyzeOptions) -> anyhow::Result<Report> {
    let facts = extract_path(path)?;
    Ok(analyze_with_options(
        &facts,
        &StdlibSinkTable::new(),
        options,
    ))
}
