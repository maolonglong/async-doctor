//! Deterministic 0–100 health score computation and band labels for [`Report`].

use std::collections::{BTreeMap, BTreeSet};

use super::Report;

pub(super) fn default_score() -> u32 {
    100
}

fn rule_severity_weight(rule: &str) -> f64 {
    match rule {
        "nested-event-loop"
        | "sync-with-async-cm"
        | "sync-for-async-iterable"
        | "eager-offload-call"
        | "unawaited-coroutine"
        | "offload-coroutine" => 1.5,
        "blocking-sleep" | "blocking-subprocess" => 1.35,
        "blocking-http" | "blocking-db" | "blocking-socket" => 1.2,
        "blocking-lock" => 1.1,
        "blocking-file" => 0.85,
        _ => 1.0,
    }
}

impl Report {
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
            let mut unique_sites = BTreeMap::new();
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
            let mut entries = BTreeSet::new();
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
}

#[cfg(test)]
mod tests {
    use crate::report::{
        Advice, AdviceCategory, ChainStep, EXIT_FINDINGS, EXIT_OK, Entry, EntryLabel, Finding, Fix,
        Report, UnusedSuppression,
    };

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
