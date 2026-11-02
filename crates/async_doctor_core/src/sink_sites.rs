//! Sink-site aggregation for agent-facing reports.
//!
//! One fix group can fan out across thousands of entries while collapsing to a
//! handful of concrete sink lines. Ranking those sites by entry count tells an
//! agent which lines to fix first. When several sites share the exact same
//! entry set, they are marked so agents do not think fixing one clears them all.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use crate::report::{ChainStep, Finding, SinkSiteSummary};

/// Return the `(index, step)` of the primary sink hop in `f.chain`.
///
/// For `with open(...) as f: f.write(...)`, `merge_file_io_findings` appends the
/// `read`/`write` step after the `open` step while `f.why` and `f.id` remain
/// anchored at the `open` hop. Prefer the step matching `f.why` when present so
/// `sinkSites`, `f.why`, and `async-doctor why` agree on the primary sink line.
pub fn primary_sink_step(f: &Finding) -> Option<(usize, &ChainStep)> {
    let last_idx = f.chain.len().checked_sub(1)?;
    if let Some((why_file, why_line)) = why_file_line(&f.why)
        && let Some((idx, step)) = f
            .chain
            .iter()
            .enumerate()
            .rfind(|(_, s)| s.line == why_line && s.file == why_file)
    {
        return Some((idx, step));
    }
    Some((last_idx, &f.chain[last_idx]))
}

fn why_file_line(why: &str) -> Option<(&str, u32)> {
    let rest = why.strip_prefix("async-doctor why ")?;
    let loc = rest.split_whitespace().next()?;
    let (file, line_str) = loc.rsplit_once(':')?;
    let line = line_str.parse::<u32>().ok()?;
    Some((file, line))
}

/// Build an additive `sinkSites` summary from findings (largest sites first).
pub fn summarize(findings: &[Finding]) -> Vec<SinkSiteSummary> {
    // (fix_group_id, file, line) → aggregate — BTreeMap for stable order.
    let mut sites: BTreeMap<(&str, &str, u32), SiteAcc<'_>> = BTreeMap::new();

    for (f_idx, f) in findings.iter().enumerate() {
        let Some((sink_idx, sink_step)) = primary_sink_step(f) else {
            continue;
        };
        let key = (
            f.fix.fix_group_id.as_str(),
            sink_step.file.as_str(),
            sink_step.line,
        );
        let acc = sites.entry(key).or_insert_with(|| SiteAcc {
            rule: f.rule.as_str(),
            fix_group_id: f.fix.fix_group_id.as_str(),
            file: sink_step.file.as_str(),
            line: sink_step.line,
            text: sink_step.text.as_str(),
            finding_count: 0,
            entries: BTreeSet::new(),
            intermediates: BTreeMap::new(),
        });
        acc.finding_count += 1;
        acc.entries
            .insert((f.entry.file.as_str(), f.entry.name.as_str()));
        if sink_idx >= 1 {
            for step in &f.chain[..sink_idx] {
                let ikey = (step.file.as_str(), step.line, step.text.as_str());
                let entry = acc.intermediates.entry(ikey).or_insert((0, usize::MAX));
                if entry.1 != f_idx {
                    entry.0 += 1;
                    entry.1 = f_idx;
                }
            }
        }
    }

    // Within each fix group, count how many sites share each entry set.
    let mut set_counts: BTreeMap<(&str, String), u32> = BTreeMap::new();
    let mut site_set_ids: BTreeMap<(&str, &str, u32), String> = BTreeMap::new();
    for (&(gid, file, line), acc) in &sites {
        let set_id = entry_set_id(&acc.entries);
        *set_counts.entry((gid, set_id.clone())).or_default() += 1;
        site_set_ids.insert((gid, file, line), set_id);
    }

    let mut out: Vec<SinkSiteSummary> = sites
        .into_iter()
        .map(|(key, acc)| {
            let set_id = site_set_ids.remove(&key).unwrap_or_default();
            let shared = set_counts
                .get(&(acc.fix_group_id, set_id.clone()))
                .copied()
                .unwrap_or(1);
            let choke = dominant_choke(&acc);
            SinkSiteSummary {
                rule: acc.rule.to_string(),
                fix_group_id: acc.fix_group_id.to_string(),
                file: acc.file.to_string(),
                line: acc.line,
                text: acc.text.to_string(),
                finding_count: acc.finding_count,
                entry_count: acc.entries.len() as u32,
                entry_set_id: set_id,
                shared_entry_site_count: shared,
                choke_point_file: choke.as_ref().map(|c| c.0.to_string()).unwrap_or_default(),
                choke_point_line: choke.as_ref().map(|c| c.1).unwrap_or(0),
                choke_point_text: choke.as_ref().map(|c| c.2.to_string()).unwrap_or_default(),
                choke_point_finding_count: choke.as_ref().map(|c| c.3).unwrap_or(0),
            }
        })
        .collect();

    out.sort_by(|a, b| {
        b.entry_count
            .cmp(&a.entry_count)
            .then(b.finding_count.cmp(&a.finding_count))
            .then(a.entry_set_id.cmp(&b.entry_set_id))
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });
    out
}

fn entry_set_id(entries: &BTreeSet<(&str, &str)>) -> String {
    let mut h = Sha256::new();
    for (f, n) in entries {
        h.update(f.as_bytes());
        h.update([0]);
        h.update(n.as_bytes());
        h.update([0]);
    }
    format!("{:x}", h.finalize())[..16].to_string()
}

struct SiteAcc<'a> {
    rule: &'a str,
    fix_group_id: &'a str,
    file: &'a str,
    line: u32,
    text: &'a str,
    finding_count: u32,
    entries: BTreeSet<(&'a str, &'a str)>,
    intermediates: BTreeMap<(&'a str, u32, &'a str), (u32, usize)>,
}

/// An intermediate hop dominates when it covers ≥50% of findings at the site
/// and at least 2 findings (avoid noise on tiny sites).
fn dominant_choke<'a>(acc: &SiteAcc<'a>) -> Option<(&'a str, u32, &'a str, u32)> {
    if acc.finding_count < 2 {
        return None;
    }
    let mut best: Option<(&'a str, u32, &'a str, u32)> = None;
    for (&(file, line, text), &(n, _)) in &acc.intermediates {
        let better = match best {
            None => true,
            Some((bf, bl, bt, bn)) => n > bn || (n == bn && (file, line, text) < (bf, bl, bt)),
        };
        if better {
            best = Some((file, line, text, n));
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
        let Some((sink_idx, _)) = primary_sink_step(f) else {
            continue;
        };
        if sink_idx < 1 {
            continue;
        }
        for step in &f.chain[..sink_idx] {
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
            Some((bf, bl, bt, bn)) => n > *bn || (n == *bn && (&file, &line, &text) < (bf, bl, bt)),
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
        assert_eq!(sites[0].shared_entry_site_count, 1);
        assert_eq!(sites[1].line, 20);
        assert_eq!(sites[1].entry_count, 1);
    }

    #[test]
    fn marks_sites_that_share_the_same_entry_set() {
        // Same entries a,b reach two different sink lines — fixing one is not enough.
        let findings = vec![
            finding("1", "a", &[("sink.py", 10, "open()")]),
            finding("2", "b", &[("sink.py", 10, "open()")]),
            finding("3", "a", &[("sink.py", 20, "read()")]),
            finding("4", "b", &[("sink.py", 20, "read()")]),
            finding("5", "c", &[("other.py", 1, "unlink()")]),
        ];
        let sites = summarize(&findings);
        let shared: Vec<_> = sites
            .iter()
            .filter(|s| s.shared_entry_site_count > 1)
            .collect();
        assert_eq!(shared.len(), 2);
        assert!(shared.iter().all(|s| s.shared_entry_site_count == 2));
        assert_eq!(shared[0].entry_set_id, shared[1].entry_set_id);
        let uniq = sites.iter().find(|s| s.line == 1).unwrap();
        assert!(uniq.shared_entry_site_count <= 1);
    }
}
