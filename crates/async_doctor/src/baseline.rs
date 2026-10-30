//! Baseline read/write — suppress known findings by stable id.
//!
//! Fingerprints are finding `id` values (hash of rule + symbol path + sink +
//! chain qualnames), so line shifts and unrelated renames do not churn the
//! baseline.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::Context;
use async_doctor_core::{Report, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};

/// On-disk baseline format (schemaVersion 1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaselineFile {
    pub schema_version: u32,
    /// Stable finding ids. Line numbers are intentionally absent.
    pub ids: Vec<String>,
    /// Optional human-oriented rows (ignored when matching).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<BaselineFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaselineFinding {
    pub id: String,
    pub rule: String,
    pub entry: String,
}

impl BaselineFile {
    pub fn from_report(report: &Report) -> Self {
        let mut findings: Vec<BaselineFinding> = report
            .findings
            .iter()
            .map(|f| BaselineFinding {
                id: f.id.clone(),
                rule: f.rule.clone(),
                entry: format!("{}:{}", f.entry.name, f.entry.file),
            })
            .collect();
        findings.sort_by(|a, b| a.id.cmp(&b.id));
        let ids = findings.iter().map(|f| f.id.clone()).collect();
        Self {
            schema_version: SCHEMA_VERSION,
            ids,
            findings,
        }
    }

    pub fn id_set(&self) -> HashSet<String> {
        self.ids.iter().cloned().collect()
    }
}

pub fn write_baseline(path: &Path, report: &Report) -> anyhow::Result<()> {
    let baseline = BaselineFile::from_report(report);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create baseline dir {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(&baseline)?;
    let mut f =
        fs::File::create(path).with_context(|| format!("create baseline {}", path.display()))?;
    f.write_all(json.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}

pub fn read_baseline(path: &Path) -> anyhow::Result<BaselineFile> {
    let text =
        fs::read_to_string(path).with_context(|| format!("read baseline {}", path.display()))?;
    let baseline: BaselineFile = serde_json::from_str(&text)
        .with_context(|| format!("parse baseline {}", path.display()))?;
    Ok(baseline)
}

/// Drop findings whose stable id appears in the baseline. Candidates unchanged.
pub fn filter_report(mut report: Report, baseline: &BaselineFile) -> Report {
    let ids = baseline.id_set();
    report.findings.retain(|f| !ids.contains(&f.id));
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_doctor_core::{Entry, EntryLabel, Finding, Fix};

    fn finding(id: &str, rule: &str) -> Finding {
        Finding {
            id: id.into(),
            rule: rule.into(),
            entry: Entry {
                file: "a.py".into(),
                line: 10,
                name: "handler".into(),
                label: EntryLabel::Other,
            },
            chain: vec![],
            conditions: vec![],
            fix: Fix {
                hint: "x".into(),
                fix_group_id: "g".into(),
            },
            why: "async-doctor why a.py:10".into(),
            docs_anchor: format!("#{rule}"),
        }
    }

    #[test]
    fn filter_drops_baselined_ids_only() {
        let mut report = Report::empty_complete();
        report.findings = vec![
            finding("aaa", "blocking-sleep"),
            finding("bbb", "blocking-http"),
        ];
        let baseline = BaselineFile {
            schema_version: 1,
            ids: vec!["aaa".into()],
            findings: vec![],
        };
        let filtered = filter_report(report, &baseline);
        assert_eq!(filtered.findings.len(), 1);
        assert_eq!(filtered.findings[0].id, "bbb");
    }

    #[test]
    fn roundtrip_ignores_line_numbers() {
        let mut report = Report::empty_complete();
        report.findings = vec![finding("stable", "blocking-sleep")];
        let bl = BaselineFile::from_report(&report);
        assert_eq!(bl.ids, vec!["stable"]);
        // Shift the line; id unchanged → still suppressed.
        report.findings[0].entry.line = 999;
        let filtered = filter_report(report, &bl);
        assert!(filtered.findings.is_empty());
    }
}
