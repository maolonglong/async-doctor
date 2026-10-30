//! Sink-site aggregation for agent-facing reports.
//!
//! One fix group can fan out across thousands of entries while collapsing to a
//! handful of concrete sink lines. Ranking those sites by entry count tells an
//! agent which lines to fix first.

use std::collections::{BTreeMap, BTreeSet};

use crate::report::{Finding, SinkSiteSummary};

/// Build an additive `sinkSites` summary from findings (largest sites first).
pub fn summarize(findings: &[Finding]) -> Vec<SinkSiteSummary> {
    // (fix_group_id, file, line) → aggregate — BTreeMap for stable order.
    let mut sites: BTreeMap<(String, String, u32), SiteAcc> = BTreeMap::new();

    for f in findings {
        let Some(last) = f.chain.last() else {
            continue;
        };
        let key = (f.fix.fix_group_id.clone(), last.file.clone(), last.line);
        let acc = sites.entry(key).or_insert_with(|| SiteAcc {
            rule: f.rule.clone(),
            fix_group_id: f.fix.fix_group_id.clone(),
            file: last.file.clone(),
            line: last.line,
            text: last.text.clone(),
            finding_count: 0,
            entries: BTreeSet::new(),
            intermediates: BTreeMap::new(),
        });
        acc.finding_count += 1;
        acc.entries
            .insert((f.entry.file.clone(), f.entry.name.clone()));
        let n = f.chain.len();
        if n >= 2 {
            for step in &f.chain[..n - 1] {
                let ikey = (step.file.clone(), step.line, step.text.clone());
                acc.intermediates
                    .entry(ikey)
                    .or_default()
                    .insert(f.id.clone());
            }
        }
    }

    let mut out: Vec<SinkSiteSummary> = sites
        .into_values()
        .map(|acc| {
            let choke = dominant_choke(&acc);
            SinkSiteSummary {
                rule: acc.rule,
                fix_group_id: acc.fix_group_id,
                file: acc.file,
                line: acc.line,
                text: acc.text,
                finding_count: acc.finding_count,
                entry_count: acc.entries.len() as u32,
                choke_point_file: choke.as_ref().map(|c| c.0.clone()).unwrap_or_default(),
                choke_point_line: choke.as_ref().map(|c| c.1).unwrap_or(0),
                choke_point_text: choke.as_ref().map(|c| c.2.clone()).unwrap_or_default(),
                choke_point_finding_count: choke.as_ref().map(|c| c.3).unwrap_or(0),
            }
        })
        .collect();

    out.sort_by(|a, b| {
        b.entry_count
            .cmp(&a.entry_count)
            .then(b.finding_count.cmp(&a.finding_count))
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });
    out
}

struct SiteAcc {
    rule: String,
    fix_group_id: String,
    file: String,
    line: u32,
    text: String,
    finding_count: u32,
    entries: BTreeSet<(String, String)>,
    intermediates: BTreeMap<(String, u32, String), BTreeSet<String>>,
}

/// An intermediate hop dominates when it covers ≥50% of findings at the site
/// and at least 2 findings (avoid noise on tiny sites).
fn dominant_choke(acc: &SiteAcc) -> Option<(String, u32, String, u32)> {
    if acc.finding_count < 2 {
        return None;
    }
    let mut best: Option<(String, u32, String, u32)> = None;
    for ((file, line, text), ids) in &acc.intermediates {
        let n = ids.len() as u32;
        let better = match &best {
            None => true,
            Some((bf, bl, bt, bn)) => n > *bn || (n == *bn && (file, line, text) < (bf, bl, bt)),
        };
        if better {
            best = Some((file.clone(), *line, text.clone(), n));
        }
    }
    let (file, line, text, n) = best?;
    if n * 2 >= acc.finding_count {
        Some((file, line, text, n))
    } else {
        None
    }
}

/// Group-level choke point across all findings in a fix group.
pub fn group_choke_point(findings: &[&Finding]) -> Option<(String, u32, String, u32)> {
    if findings.len() < 2 {
        return None;
    }
    let mut counts: BTreeMap<(String, u32, String), u32> = BTreeMap::new();
    for f in findings {
        let n = f.chain.len();
        if n < 2 {
            continue;
        }
        for step in &f.chain[..n - 1] {
            *counts
                .entry((step.file.clone(), step.line, step.text.clone()))
                .or_default() += 1;
        }
    }
    let total = findings.len() as u32;
    let mut best: Option<(String, u32, String, u32)> = None;
    for ((file, line, text), n) in counts {
        let better = match &best {
            None => true,
            Some((bf, bl, bt, bn)) => n > *bn || (n == *bn && (&file, line, &text) < (bf, bl, bt)),
        };
        if better {
            best = Some((file, line, text, n));
        }
    }
    let (file, line, text, n) = best?;
    if n * 2 >= total {
        Some((file, line, text, n))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{ChainStep, Entry, EntryLabel, Fix};

    fn finding(id: &str, entry: &str, hops: &[(&str, u32, &str)]) -> Finding {
        Finding {
            id: id.into(),
            rule: "blocking-file".into(),
            entry: Entry {
                file: format!("{entry}.py"),
                line: 1,
                name: entry.into(),
                label: EntryLabel::Other,
            },
            chain: hops
                .iter()
                .map(|(f, l, t)| ChainStep {
                    file: (*f).into(),
                    line: *l,
                    text: (*t).into(),
                    third_party: false,
                    package: None,
                    resolution: String::new(),
                })
                .collect(),
            conditions: vec![],
            fix: Fix {
                hint: "fix".into(),
                fix_group_id: "blocking-file:open".into(),
            },
            why: String::new(),
            docs_anchor: String::new(),
            docs_url: String::new(),
        }
    }

    #[test]
    fn ranks_sink_sites_by_entry_count() {
        let findings = vec![
            finding(
                "1",
                "a",
                &[("mid.py", 1, "helper()"), ("sink.py", 10, "open()")],
            ),
            finding(
                "2",
                "b",
                &[("mid.py", 1, "helper()"), ("sink.py", 10, "open()")],
            ),
            finding(
                "3",
                "c",
                &[("other.py", 2, "x()"), ("sink.py", 20, "read()")],
            ),
        ];
        let sites = summarize(&findings);
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0].line, 10);
        assert_eq!(sites[0].entry_count, 2);
        assert_eq!(sites[0].finding_count, 2);
        assert_eq!(sites[0].choke_point_file, "mid.py");
        assert_eq!(sites[0].choke_point_finding_count, 2);
        assert_eq!(sites[1].line, 20);
        assert_eq!(sites[1].entry_count, 1);
    }
}
