//! JSON/text report types (schemaVersion 1).

use serde::{Deserialize, Serialize};

/// JSON schema version. Bump only on breaking field changes.
pub const SCHEMA_VERSION: u32 = 1;

/// Exit code: no findings.
pub const EXIT_OK: i32 = 0;
/// Exit code: one or more findings (candidates do not affect this).
pub const EXIT_FINDINGS: i32 = 1;
/// Exit code: tool error only. Incomplete analysis uses `complete=false`, not exit 2.
pub const EXIT_ERROR: i32 = 2;

/// How an async entry was inferred. Labels affect ordering/presentation only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryLabel {
    Route,
    Lifecycle,
    Task,
    Other,
}

impl EntryLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Route => "route",
            Self::Lifecycle => "lifecycle",
            Self::Task => "task",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainStep {
    pub file: String,
    pub line: u32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub file: String,
    pub line: u32,
    /// Qualified name when nested in a class (`Class.method`), otherwise the function name.
    pub name: String,
    pub label: EntryLabel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fix {
    pub hint: String,
    pub fix_group_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub rule: String,
    pub entry: Entry,
    pub chain: Vec<ChainStep>,
    pub conditions: Vec<String>,
    pub fix: Fix,
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub rule: String,
    pub entry: Entry,
    pub chain: Vec<ChainStep>,
    pub conditions: Vec<String>,
    pub reason: String,
    pub fix: Fix,
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub schema_version: u32,
    pub complete: bool,
    pub findings: Vec<Finding>,
    pub candidates: Vec<Candidate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
    /// Async entries omitted because they live under tests/docs/examples/… (zero-config).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub omitted_non_production_entries: u32,
    /// `unresolved` candidates omitted unless `--show-unresolved`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub hidden_unresolved_candidates: u32,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

impl Report {
    pub fn empty_complete() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            complete: true,
            findings: Vec::new(),
            candidates: Vec::new(),
            skipped: Vec::new(),
            omitted_non_production_entries: 0,
            hidden_unresolved_candidates: 0,
        }
    }

    pub fn exit_code(&self) -> i32 {
        if self.findings.is_empty() {
            EXIT_OK
        } else {
            EXIT_FINDINGS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_ignores_candidates() {
        let mut report = Report::empty_complete();
        assert_eq!(report.exit_code(), EXIT_OK);
        report.findings.push(Finding {
            id: "x".into(),
            rule: "blocking-sleep".into(),
            entry: Entry {
                file: "a.py".into(),
                line: 1,
                name: "f".into(),
                label: EntryLabel::Other,
            },
            chain: vec![],
            conditions: vec![],
            fix: Fix {
                hint: "x".into(),
                fix_group_id: "g".into(),
            },
            why: "async-doctor why a.py:1".into(),
        });
        assert_eq!(report.exit_code(), EXIT_FINDINGS);
    }
}
