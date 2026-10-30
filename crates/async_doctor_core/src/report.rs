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
#[serde(rename_all = "camelCase")]
pub struct ChainStep {
    pub file: String,
    pub line: u32,
    pub text: String,
    /// True when this hop is inside a followed third-party (site-packages) function.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub third_party: bool,
    /// Distribution name when `third_party` (e.g. `urllib3`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// How this hop was resolved (typed callee, project call, third-party follow, sink).
    /// Additive in schema v1 — for `why` evidence and agents.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resolution: String,
}

impl ChainStep {
    pub fn project(file: String, line: u32, text: String) -> Self {
        Self {
            file,
            line,
            text,
            third_party: false,
            package: None,
            resolution: String::new(),
        }
    }

    pub fn third_party(file: String, line: u32, text: String, package: Option<String>) -> Self {
        Self {
            file,
            line,
            text,
            third_party: true,
            package,
            resolution: String::new(),
        }
    }

    pub fn with_resolution(mut self, resolution: impl Into<String>) -> Self {
        self.resolution = resolution.into();
        self
    }
}

/// Aggregated unresolved-callee pattern (shown with `--show-unresolved`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedPattern {
    /// Callee pattern key (e.g. reason string or snipped call text).
    pub pattern: String,
    /// Number of distinct async entries affected.
    pub entries_affected: u32,
    /// Number of unresolved call sites matching this pattern.
    pub sites: u32,
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
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub id: String,
    pub rule: String,
    pub entry: Entry,
    pub chain: Vec<ChainStep>,
    pub conditions: Vec<String>,
    pub fix: Fix,
    /// Runnable locator help: `async-doctor why file:line --path <scan-root>`.
    pub why: String,
    /// Fragment into `docs/rules.md` (e.g. `#blocking-sleep`). Additive in schema v1.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs_anchor: String,
    /// Absolute docs URL for installed binaries. Additive in schema v1.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub id: String,
    pub rule: String,
    pub entry: Entry,
    pub chain: Vec<ChainStep>,
    pub conditions: Vec<String>,
    pub reason: String,
    pub fix: Fix,
    pub why: String,
    /// Fragment into `docs/rules.md` (e.g. `#blocking-sleep`). Additive in schema v1.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs_anchor: String,
    /// Absolute docs URL for installed binaries. Additive in schema v1.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs_url: String,
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
    /// Aggregated unresolved patterns (populated when `--show-unresolved`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_summary: Vec<UnresolvedPattern>,
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
            unresolved_summary: Vec::new(),
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
            why: "async-doctor why a.py:1 --path .".into(),
            docs_anchor: "#blocking-sleep".into(),
            docs_url:
                "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md#blocking-sleep"
                    .into(),
        });
        assert_eq!(report.exit_code(), EXIT_FINDINGS);
    }
}
