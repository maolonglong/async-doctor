//! Sink-site-first human grouping for the default text report.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use async_doctor_core::{Finding, SinkSiteSummary, group_choke_point, summarize_sink_sites};

use super::{sink_label, write_finding_indented};

const TOP_SITES_SHOWN: usize = 8;

pub(super) fn write_grouped(out: &mut impl Write, findings: &[Finding]) -> anyhow::Result<()> {
    let sites = summarize_sink_sites(findings);
    let mut groups: BTreeMap<&str, Vec<&Finding>> = BTreeMap::new();
    let mut entries: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut sink_labels: BTreeSet<&str> = BTreeSet::new();
    for f in findings {
        groups
            .entry(f.fix.fix_group_id.as_str())
            .or_default()
            .push(f);
        entries.insert((f.entry.file.as_str(), f.entry.name.as_str()));
        if let Some(sink) = sink_label(f) {
            sink_labels.insert(sink);
        }
    }

    let unique_sites = sites.len();
    writeln!(
        out,
        "Found {} finding(s) · {} entries · {} sink sites · {} fix groups\n",
        findings.len(),
        entries.len(),
        unique_sites,
        groups.len()
    )?;
    writeln!(
        out,
        "(sink sites ranked by entries cleared; pass --expand for every chain)\n"
    )?;

    let mut sites_by_group: BTreeMap<&str, Vec<&SinkSiteSummary>> = BTreeMap::new();
    for s in &sites {
        sites_by_group
            .entry(s.fix_group_id.as_str())
            .or_default()
            .push(s);
    }

    let mut ordered: Vec<_> = groups.into_iter().collect();
    ordered.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(b.0)));

    for (gid, group) in ordered {
        let rep = group[0];
        let group_sites = sites_by_group.remove(gid).unwrap_or_default();
        let mut group_entries: BTreeSet<&str> = BTreeSet::new();
        let mut group_sink_labels: BTreeSet<&str> = BTreeSet::new();
        for f in &group {
            group_entries.insert(f.entry.name.as_str());
            if let Some(s) = sink_label(f) {
                group_sink_labels.insert(s);
            }
        }
        let sink_summary = group_sink_labels
            .iter()
            .copied()
            .take(3)
            .collect::<Vec<_>>();
        let sink_extra = group_sink_labels.len().saturating_sub(sink_summary.len());
        let sink_text = if sink_extra > 0 {
            format!("{} (+{sink_extra} more)", sink_summary.join(", "))
        } else {
            sink_summary.join(", ")
        };

        writeln!(
            out,
            "[{}] {} — {} finding(s), {} entr{}, {} sink site{}",
            rep.rule,
            if sink_text.is_empty() {
                "sink".to_string()
            } else {
                sink_text
            },
            group.len(),
            group_entries.len(),
            if group_entries.len() == 1 { "y" } else { "ies" },
            group_sites.len(),
            if group_sites.len() == 1 { "" } else { "s" }
        )?;
        writeln!(out, "  fix: {}", rep.fix.hint)?;
        if !rep.docs_url.is_empty() {
            writeln!(out, "  docs: {}", rep.docs_url)?;
        } else if !rep.docs_anchor.is_empty() {
            writeln!(out, "  explain: async-doctor explain {}", rep.rule)?;
        }

        if let Some((file, line, text, n)) = group_choke_point(&group) {
            let short = truncate_call(&text, 72);
            writeln!(
                out,
                "  choke point: {file}:{line}  {short}  (in {n}/{} findings)",
                group.len()
            )?;
        }

        writeln!(
            out,
            "  top sinks (fix one line → clear N findings / M entries):"
        )?;
        for (i, site) in group_sites.iter().take(TOP_SITES_SHOWN).enumerate() {
            let short = truncate_call(&site.text, 64);
            writeln!(
                out,
                "    {}. {}:{}  {short}  — {} finding(s), {} entr{}",
                i + 1,
                site.file,
                site.line,
                site.finding_count,
                site.entry_count,
                if site.entry_count == 1 { "y" } else { "ies" }
            )?;
        }
        if group_sites.len() > TOP_SITES_SHOWN {
            writeln!(
                out,
                "    … {} more sink site(s)",
                group_sites.len() - TOP_SITES_SHOWN
            )?;
        }

        // Representative chain: prefer a finding that hits the top sink site.
        let example = group_sites
            .first()
            .and_then(|top| {
                group.iter().find(|f| {
                    f.chain
                        .last()
                        .is_some_and(|s| s.file == top.file && s.line == top.line)
                })
            })
            .copied()
            .unwrap_or(rep);
        writeln!(out, "  example:")?;
        write_finding_indented(out, example, "    ")?;
        if group.len() > 1 {
            writeln!(
                out,
                "  (+{} more; pass --expand to list all)\n",
                group.len() - 1
            )?;
        } else {
            writeln!(out)?;
        }
    }
    Ok(())
}

fn truncate_call(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        one_line
    } else {
        let mut s: String = one_line.chars().take(max.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}
