//! Report types and stable finding ids. This crate must not depend on ty/ruff.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// JSON schema version. Bump only on breaking field changes.
pub const SCHEMA_VERSION: u32 = 1;

/// Exit code: no findings.
pub const EXIT_OK: i32 = 0;
/// Exit code: one or more findings (candidates do not affect this).
pub const EXIT_FINDINGS: i32 = 1;
/// Exit code: tool error only. Incomplete analysis uses `complete=false`, not exit 2.
pub const EXIT_ERROR: i32 = 2;

/// How an async entry was inferred. Labels affect ordering/presentation only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl Report {
    pub fn empty_complete() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            complete: true,
            findings: Vec::new(),
            candidates: Vec::new(),
            skipped: Vec::new(),
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

/// Stable finding id: sha256(rule | entry | sink) hex-truncated.
pub fn finding_id(rule: &str, entry_file: &str, entry_name: &str, sink: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(rule.as_bytes());
    hasher.update(b"\0");
    hasher.update(entry_file.as_bytes());
    hasher.update(b"\0");
    hasher.update(entry_name.as_bytes());
    hasher.update(b"\0");
    hasher.update(sink.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..16])
}

pub fn fix_group_id(rule: &str, sink: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(rule.as_bytes());
    hasher.update(b"\0");
    hasher.update(sink.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..12])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finding_id_is_stable() {
        let a = finding_id("blocking-sleep", "a.py", "f", "time.sleep");
        let b = finding_id("blocking-sleep", "a.py", "f", "time.sleep");
        assert_eq!(a, b);
        assert_ne!(a, finding_id("blocking-sleep", "a.py", "g", "time.sleep"));
    }

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
