//! JSON/text report types (schemaVersion 2).

mod score;

use serde::{Deserialize, Serialize};

use crate::environment::ScanEnvironment;
use score::default_score;

/// JSON schema version. Bump only on breaking field changes.
///
/// v2: always emit top-level arrays and zero-valued count fields; add
/// `environment` and `locationLessCandidates`; `why --path` is cwd-relative
/// (never an absolute machine path). Location-less candidates may omit `why`
/// (empty) because they have no `file:line` locator.
pub const SCHEMA_VERSION: u32 = 2;

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
    /// Included for `why` evidence and agents.
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
    /// Empty when analysis does not establish a local correction. Diagnostic
    /// evidence remains in the call chain; this is not a placeholder to fill.
    pub hint: String,
    /// Stable diagnostic grouping key, independent of whether a repair exists.
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
    /// Fragment into `docs/rules.md` (e.g. `#blocking-sleep`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs_anchor: String,
    /// Absolute docs URL for installed binaries.
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
    /// Fragment into `docs/rules.md` (e.g. `#blocking-sleep`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs_anchor: String,
    /// Absolute docs URL for installed binaries.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs_url: String,
}

/// Non-blocking diagnostic tier (schemaVersion 1 additive).
///
/// Advice never affects [`Report::exit_code`] — same contract as candidates, but
/// for non-blocking correctness and performance observations rather than
/// uncertain blocking findings. An observation need not recommend an edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Advice {
    pub id: String,
    pub rule: String,
    pub category: AdviceCategory,
    pub entry: Entry,
    pub chain: Vec<ChainStep>,
    pub fix: Fix,
    pub why: String,
    /// Fragment into `docs/rules.md` (e.g. `#offload-has-async-twin`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs_anchor: String,
    /// Absolute docs URL for installed binaries.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs_url: String,
}

/// Advice category — informational only; does not affect grading severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdviceCategory {
    Performance,
    Correctness,
}

impl AdviceCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Performance => "performance",
            Self::Correctness => "correctness",
        }
    }
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
/// Counts summarize observed reachability, not how many findings an edit will
/// fix. Optional choke-point fields name a shared intermediate hop on paths to
/// this sink (`chokePointFindingCount == 0` means none).
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
    /// Sites that share the same id are reached by the exact same entries.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub entry_set_id: String,
    /// How many sink sites in this fix group share `entry_set_id`. `1` = unique.
    #[serde(default, skip_serializing_if = "is_lte_one")]
    pub shared_entry_site_count: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub choke_point_file: String,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub choke_point_line: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub choke_point_text: String,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub choke_point_finding_count: u32,
    /// Number of sync-only project callers reaching this site's project helper (`0` omitted).
    #[serde(
        rename = "syncCallerCount",
        default,
        skip_serializing_if = "is_zero_u32"
    )]
    pub sync_caller_count: u32,
    /// Sample `"file:line"` of a sync-only project caller when `sync_caller_count > 0`.
    #[serde(
        rename = "sampleSyncCaller",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub sample_sync_caller: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub schema_version: u32,
    pub complete: bool,
    /// Deterministic 0–100 health score (`100` when clean; `< 85` when findings or unused suppressions remain).
    #[serde(default = "default_score")]
    pub score: u32,
    /// Interpreter / venv / site-packages used for this scan (schema v2).
    #[serde(default)]
    pub environment: ScanEnvironment,
    /// Top-level arrays are always emitted (schema v2) — including when empty.
    pub findings: Vec<Finding>,
    pub candidates: Vec<Candidate>,
    /// Non-blocking suggestions (never affect exit code).
    #[serde(default)]
    pub advice: Vec<Advice>,
    /// Candidates with no file location (e.g. aggregated missing third-party).
    #[serde(default)]
    pub location_less_candidates: Vec<Candidate>,
    #[serde(default)]
    pub skipped: Vec<String>,
    /// Human/agent-facing status when findings are empty for a special reason
    /// (e.g. no Python files under the scan root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Async entries omitted because they live under tests/docs/examples/… (zero-config).
    /// Always emitted (including `0`) for a stable JSON contract.
    #[serde(default)]
    pub omitted_non_production_entries: u32,
    /// `unresolved` candidates omitted unless `--show-unresolved`.
    /// Always emitted (including `0`) for a stable JSON contract.
    #[serde(default)]
    pub hidden_unresolved_candidates: u32,
    /// Entries cut by `--max-findings` (always emitted; 0 without the flag).
    #[serde(default)]
    pub omitted_findings: u32,
    #[serde(default)]
    pub omitted_candidates: u32,
    #[serde(default)]
    pub omitted_advice: u32,
    /// Aggregated unresolved patterns (populated when `--show-unresolved`).
    #[serde(default)]
    pub unresolved_summary: Vec<UnresolvedPattern>,
    /// Inline ignore comments that did not suppress anything.
    #[serde(default)]
    pub unused_suppressions: Vec<UnusedSuppression>,
    /// Sink sites ranked by entry count.
    #[serde(default)]
    pub sink_sites: Vec<SinkSiteSummary>,
    /// Number of first-party Python files scanned.
    #[serde(default)]
    pub file_count: u32,
    /// Number of active first-party `async def` entries evaluated.
    #[serde(default)]
    pub async_entry_count: u32,
    /// `.py` files under the scanned directory root that contained `"async-doctor"`
    /// during fingerprinting, avoiding a second full tree walk for inline suppressions
    /// without any process-global side channel.
    #[serde(skip)]
    pub ignore_candidate_files: Option<Vec<std::path::PathBuf>>,
    /// Per-diagnostic `(file, start_line, end_line)` project spans used by `--changed`
    /// to retain cross-file `def` → `async def` and intra-function / `__init__` edits
    /// without altering the public JSON contract.
    #[serde(skip)]
    pub item_spans: std::collections::BTreeMap<String, Vec<(String, u32, u32)>>,
}

fn is_zero_u32(v: &u32) -> bool {
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
            score: 100,
            environment: ScanEnvironment::none(),
            findings: Vec::new(),
            candidates: Vec::new(),
            advice: Vec::new(),
            location_less_candidates: Vec::new(),
            skipped: Vec::new(),
            message: None,
            omitted_non_production_entries: 0,
            hidden_unresolved_candidates: 0,
            omitted_findings: 0,
            omitted_candidates: 0,
            omitted_advice: 0,
            unresolved_summary: Vec::new(),
            unused_suppressions: Vec::new(),
            sink_sites: Vec::new(),
            file_count: 0,
            async_entry_count: 0,
            ignore_candidate_files: None,
            item_spans: std::collections::BTreeMap::new(),
        }
    }

    /// `--max-findings` output cap. Findings are chosen round-robin over
    /// `sink_sites` order so each site keeps an example while `n >= sites`;
    /// emitted order stays the original sort. `sink_sites`, `score` and
    /// `exit_code()` must be read before calling.
    pub fn apply_max_findings(&mut self, n: usize) {
        if self.findings.len() > n {
            let site_index: std::collections::HashMap<(String, String, u32), usize> = self
                .sink_sites
                .iter()
                .enumerate()
                .map(|(i, s)| ((s.rule.clone(), s.file.clone(), s.line), i))
                .collect();
            let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); self.sink_sites.len()];
            let mut rest: Vec<usize> = Vec::new();
            for (i, f) in self.findings.iter().enumerate() {
                let key = crate::sink_sites::primary_sink_step(f)
                    .map(|(_, st)| (f.rule.clone(), st.file.clone(), st.line));
                match key.as_ref().and_then(|k| site_index.get(k)) {
                    Some(&si) => buckets[si].push(i),
                    None => rest.push(i),
                }
            }
            buckets.push(rest);
            let mut picked: Vec<usize> = Vec::with_capacity(n);
            let mut heads = vec![0usize; buckets.len()];
            while picked.len() < n {
                let mut any = false;
                for (b, head) in heads.iter_mut().enumerate() {
                    if *head < buckets[b].len() && picked.len() < n {
                        picked.push(buckets[b][*head]);
                        *head += 1;
                        any = true;
                    }
                }
                if !any {
                    break;
                }
            }
            picked.sort_unstable();
            self.omitted_findings = (self.findings.len() - picked.len()) as u32;
            let old = std::mem::take(&mut self.findings);
            self.findings = picked.into_iter().map(|i| old[i].clone()).collect();
        }
        if self.candidates.len() > n {
            self.omitted_candidates = (self.candidates.len() - n) as u32;
            self.candidates.truncate(n);
        }
        if self.advice.len() > n {
            self.omitted_advice = (self.advice.len() - n) as u32;
            self.advice.truncate(n);
        }
    }

    /// Refresh the additive `sinkSites` summary and `score` from current findings/advice.
    pub fn refresh_sink_sites(&mut self) {
        let prior_sync_callers: std::collections::HashMap<
            (String, String, u32),
            (u32, Option<String>),
        > = self
            .sink_sites
            .iter()
            .filter(|s| s.sync_caller_count > 0)
            .map(|s| {
                (
                    (s.fix_group_id.clone(), s.file.clone(), s.line),
                    (s.sync_caller_count, s.sample_sync_caller.clone()),
                )
            })
            .collect();
        let mut refreshed = crate::sink_sites::summarize(&self.findings);
        for site in &mut refreshed {
            if let Some((count, sample)) =
                prior_sync_callers.get(&(site.fix_group_id.clone(), site.file.clone(), site.line))
            {
                site.sync_caller_count = *count;
                site.sample_sync_caller = sample.clone();
            }
        }
        self.sink_sites = refreshed;
        self.refresh_score();
    }

    /// Exit 1 when findings or unused suppressions remain; candidates and advice ignored.
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

    #[test]
    fn exit_code_ignores_advice() {
        let mut report = Report::empty_complete();
        report.advice.push(Advice {
            id: "a".into(),
            rule: "offload-has-async-twin".into(),
            category: AdviceCategory::Performance,
            entry: Entry {
                file: "a.py".into(),
                line: 1,
                name: "f".into(),
                label: EntryLabel::Other,
            },
            chain: vec![],
            fix: Fix {
                hint: "use AsyncClient".into(),
                fix_group_id: "g".into(),
            },
            why: "async-doctor why a.py:2 --path .".into(),
            docs_anchor: "#offload-has-async-twin".into(),
            docs_url: String::new(),
        });
        assert_eq!(report.exit_code(), EXIT_OK);
    }
}
