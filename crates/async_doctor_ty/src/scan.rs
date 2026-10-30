//! Public scan entry: extract facts, then grade with the stdlib sink table.

use std::path::Path;

use async_doctor_core::{Report, analyze};
use async_doctor_rules::StdlibSinkTable;

use crate::extract::extract_path;

/// Scan a project directory or a single `.py` file.
pub fn scan_path(path: &Path) -> anyhow::Result<Report> {
    let facts = extract_path(path)?;
    Ok(analyze(&facts, &StdlibSinkTable::new()))
}
