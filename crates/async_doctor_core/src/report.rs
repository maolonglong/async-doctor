//! JSON/text report types (schemaVersion 2).

use serde::{Deserialize, Serialize};

use crate::environment::ScanEnvironment;

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
/// for high-confidence performance suggestions (e.g. replace an offload with an
/// async twin) rather than uncertain findings.
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
/// Agents use this to pick the few lines that clear the most findings. Optional
/// choke-point fields name a shared intermediate hop that dominates paths to
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
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

fn is_lte_one(v: &u32) -> bool {
    *v <= 1
}

fn default_score() -> u32 {
    100
}

fn rule_severity_weight(rule: &str) -> f64 {
    match rule {
        "nested-event-loop" | "sync-with-async-cm" | "offload-coroutine" => 1.5,
        "blocking-sleep" | "blocking-subprocess" => 1.35,
        "blocking-http" | "blocking-db" | "blocking-socket" => 1.2,
        "blocking-lock" => 1.1,
        "blocking-file" => 0.85,
        _ => 1.0,
    }
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
        self.sink_sites = crate::sink_sites::summarize(&self.findings);
        self.refresh_score();
    }

    /// Recompute `score` (`0..=100`) from current `findings`, `sink_sites`, `unused_suppressions`, and `advice`.
    pub fn refresh_score(&mut self) {
        self.score = self.compute_score();
    }

    /// Deterministic 0–100 health score.
    ///
    /// Invariant: `score == 100` when completely clean, and `score >= 85` ("Great")
    /// holds iff `exit_code() == EXIT_OK` (zero findings and zero unused
    /// suppressions). Advice deducts at most 15 points.
    ///
    /// When findings exist, penalty scales logarithmically with rule-weighted
    /// unique sink sites (`sink_sites`) and with the share of affected async
    /// entries so large repos with a handful of shared helpers do not collapse to
    /// `0` alongside repos where nearly every entry blocks.
    pub fn compute_score(&self) -> u32 {
        let finding_count = self.findings.len() as u32;
        let unused_count = self.unused_suppressions.len() as u32;
        let advice_count = self.advice.len() as u32;
        let advice_penalty = ((6.0 * (1.0 + advice_count as f64).ln()).round() as u32).min(15);

        if finding_count == 0 && unused_count == 0 {
            return 100u32.saturating_sub(advice_penalty);
        }

        let base_error_penalty = 16u32;

        let site_penalty = if finding_count == 0 {
            0u32
        } else if !self.sink_sites.is_empty() {
            let weighted_sites: f64 = self
                .sink_sites
                .iter()
                .map(|s| rule_severity_weight(&s.rule))
                .sum();
            ((9.5 * (1.0 + weighted_sites).ln()).round() as u32).min(45)
        } else {
            let mut unique_sites = std::collections::BTreeMap::new();
            for f in &self.findings {
                let key = crate::sink_sites::primary_sink_step(f)
                    .map(|(_, s)| (s.file.as_str(), s.line))
                    .unwrap_or((f.entry.file.as_str(), f.entry.line));
                let w = rule_severity_weight(&f.rule);
                let entry = unique_sites.entry(key).or_insert(0.0f64);
                if w > *entry {
                    *entry = w;
                }
            }
            let weighted_sites: f64 = unique_sites.values().sum();
            ((9.5 * (1.0 + weighted_sites).ln()).round() as u32).min(45)
        };

        let affected_entries = {
            let mut entries = std::collections::BTreeSet::new();
            for f in &self.findings {
                entries.insert((f.entry.file.as_str(), f.entry.name.as_str()));
            }
            entries.len() as u32
        };

        let spread_penalty = if finding_count == 0 {
            0u32
        } else {
            let total_entries = self.async_entry_count.max(affected_entries).max(1);
            let entry_ratio = (affected_entries as f64) / (total_entries as f64);
            let ratio_pts = 16.0 * entry_ratio.sqrt();
            let fanout_pts = 2.0 * (1.0 + finding_count as f64).ln();
            ((ratio_pts + fanout_pts).round() as u32).min(25)
        };

        let unused_penalty = (unused_count * 4).min(12);

        let total_penalty =
            base_error_penalty + site_penalty + spread_penalty + unused_penalty + advice_penalty;
        100u32.saturating_sub(total_penalty)
    }

    /// Human-readable rating band for `self.score`.
    pub fn score_label(&self) -> &'static str {
        if self.score >= 85 {
            "Great"
        } else if self.score >= 60 {
            "Needs work"
        } else {
            "Critical"
        }
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

    fn make_finding(entry_name: &str, rule: &str, sink_file: &str, sink_line: u32) -> Finding {
        Finding {
            id: format!("{entry_name}-{rule}-{sink_line}"),
            rule: rule.into(),
            entry: Entry {
                file: "app.py".into(),
                line: 1,
                name: entry_name.into(),
                label: EntryLabel::Other,
            },
            chain: vec![ChainStep::project(
                sink_file.into(),
                sink_line,
                "sink()".into(),
            )],
            conditions: vec![],
            fix: Fix {
                hint: "offload".into(),
                fix_group_id: format!("fg-{sink_line}"),
            },
            why: format!("async-doctor why {sink_file}:{sink_line} --path ."),
            docs_anchor: format!("#{rule}"),
            docs_url: String::new(),
        }
    }

    #[test]
    fn score_preserves_band_invariants_and_advice_cap() {
        let mut clean = Report::empty_complete();
        clean.refresh_score();
        assert_eq!(clean.score, 100);
        assert_eq!(clean.score_label(), "Great");

        for i in 0..50 {
            clean.advice.push(Advice {
                id: format!("a-{i}"),
                rule: "offload-has-async-twin".into(),
                category: AdviceCategory::Performance,
                entry: Entry {
                    file: "a.py".into(),
                    line: i,
                    name: format!("f_{i}"),
                    label: EntryLabel::Other,
                },
                chain: vec![],
                fix: Fix {
                    hint: "use async twin".into(),
                    fix_group_id: "g".into(),
                },
                why: String::new(),
                docs_anchor: String::new(),
                docs_url: String::new(),
            });
        }
        clean.refresh_score();
        assert!(
            (85..=100).contains(&clean.score),
            "advice-only report must stay in Great band (>=85), got {}",
            clean.score
        );
        assert_eq!(clean.exit_code(), EXIT_OK);

        let mut unused_only = Report::empty_complete();
        unused_only.unused_suppressions.push(UnusedSuppression {
            file: "a.py".into(),
            line: 1,
            rule: "blocking-sleep".into(),
            reason: "stale".into(),
        });
        unused_only.refresh_score();
        assert!(
            unused_only.score < 85,
            "unused suppression must drop score below 85, got {}",
            unused_only.score
        );
        assert_eq!(unused_only.exit_code(), EXIT_FINDINGS);
    }

    #[test]
    fn score_scales_smoothly_on_large_repo_and_weights_severity() {
        let mut sleep_one = Report::empty_complete();
        sleep_one.async_entry_count = 10;
        sleep_one
            .findings
            .push(make_finding("e1", "blocking-sleep", "a.py", 10));
        sleep_one.refresh_sink_sites();

        let mut file_one = Report::empty_complete();
        file_one.async_entry_count = 10;
        file_one
            .findings
            .push(make_finding("e1", "blocking-file", "a.py", 10));
        file_one.refresh_sink_sites();

        assert!(
            sleep_one.score < file_one.score,
            "blocking-sleep ({}) should penalize more than blocking-file ({})",
            sleep_one.score,
            file_one.score
        );

        // Large repo with 15 shared file-IO helpers fanned out across 150 of 500 async entries
        // (750 findings total) must stay well above 0 and rank above a repo where 450/500
        // entries call 100 distinct blocking-sleep sites.
        let mut shared_helpers = Report::empty_complete();
        shared_helpers.async_entry_count = 500;
        for entry_idx in 0..150 {
            let entry_name = format!("entry_{entry_idx}");
            for site_idx in 0..5 {
                let line = 100 + ((entry_idx + site_idx) % 15) as u32;
                shared_helpers.findings.push(make_finding(
                    &entry_name,
                    "blocking-file",
                    "helpers.py",
                    line,
                ));
            }
        }
        shared_helpers.refresh_sink_sites();
        assert_eq!(shared_helpers.sink_sites.len(), 15);
        assert!(
            (30..=55).contains(&shared_helpers.score),
            "expected shared-helper large repo in 30..=55 band, got {}",
            shared_helpers.score
        );

        let mut widespread = Report::empty_complete();
        widespread.async_entry_count = 500;
        for entry_idx in 0..450 {
            let entry_name = format!("entry_{entry_idx}");
            let line = 200 + (entry_idx % 100) as u32;
            widespread.findings.push(make_finding(
                &entry_name,
                "blocking-sleep",
                "handlers.py",
                line,
            ));
        }
        widespread.refresh_sink_sites();
        assert!(
            widespread.score < shared_helpers.score,
            "widespread blocking-sleep ({}) must score lower than shared file helpers ({})",
            widespread.score,
            shared_helpers.score
        );
    }
}
