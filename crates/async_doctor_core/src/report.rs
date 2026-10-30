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
    /// Plain `async def` with no more specific kind. Serialized as `async_def`
    /// so JSON matches the human `async def …` headline (not the opaque `other`).
    #[serde(rename = "async_def")]
    Other,
}

impl EntryLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Route => "route",
            Self::Lifecycle => "lifecycle",
            Self::Task => "task",
            Self::Other => "async_def",
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

/// An `# async-doctor: ignore[...]` comment that matched no finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnusedSuppression {
    pub file: String,
    pub line: u32,
    pub rule: String,
    pub reason: String,
}

/// One concrete sink line and how many findings/entries reach it.
///
/// Additive in schema v1 — agents use this to pick the few lines that clear
/// the most findings. Optional choke-point fields name a shared intermediate
/// hop that dominates paths to this sink (`chokePointFindingCount == 0` means none).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SinkSiteSummary {
    pub rule: String,
    pub fix_group_id: String,
    pub file: String,
    pub line: u32,
    pub text: String,
    pub finding_count: u32,
    pub entry_count: u32,
    /// Stable id of the entry set that reaches this sink (within the fix group).
    /// Sites that share the same id reach the exact same entries — fixing one
    /// line does not clear the others.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub entry_set_id: String,
    /// How many sink sites in this fix group share `entry_set_id`. `1` = unique.
    #[serde(default, skip_serializing_if = "is_lte_one")]
    pub shared_entry_site_count: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub choke_point_file: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub choke_point_line: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub choke_point_text: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub choke_point_finding_count: u32,
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
    /// Inline ignore comments that did not suppress anything. Additive in schema v1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unused_suppressions: Vec<UnusedSuppression>,
    /// Sink sites ranked by entry count. Additive in schema v1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sink_sites: Vec<SinkSiteSummary>,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

fn is_lte_one(v: &u32) -> bool {
    *v <= 1
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
            unused_suppressions: Vec::new(),
            sink_sites: Vec::new(),
        }
    }

    /// Refresh the additive `sinkSites` summary from current findings.
    pub fn refresh_sink_sites(&mut self) {
        self.sink_sites = crate::sink_sites::summarize(&self.findings);
    }

    /// Exit 1 when findings or unused suppressions remain; candidates ignored.
    pub fn exit_code(&self) -> i32 {
        if self.findings.is_empty() && self.unused_suppressions.is_empty() {
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
