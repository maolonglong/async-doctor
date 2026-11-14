//! Sink-site-first human grouping for the default text report.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use async_doctor_core::{
    Finding, Report, SinkSiteSummary, group_choke_point, summarize_sink_sites,
};

use super::style::Palette;
use super::{sink_label, write_finding_indented};

const TOP_CLUSTERS_SHOWN: usize = 8;
const TOP_GROUPS_SHOWN: usize = 10;

pub(super) fn write_grouped(
    out: &mut impl Write,
    report: &Report,
    pal: &Palette,
) -> anyhow::Result<()> {
    let findings = &report.findings;
    let computed_sites;
    let sites: &[SinkSiteSummary] = if !report.sink_sites.is_empty() {
        &report.sink_sites
    } else {
        computed_sites = summarize_sink_sites(findings);
        &computed_sites
    };
    let mut groups: BTreeMap<&str, Vec<&Finding>> = BTreeMap::new();
    let mut all_group_ids: BTreeSet<&str> = sites.iter().map(|s| s.fix_group_id.as_str()).collect();
    let mut entries: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut sink_labels: BTreeSet<&str> = BTreeSet::new();
    for f in findings {
        groups
            .entry(f.fix.fix_group_id.as_str())
            .or_default()
            .push(f);
        all_group_ids.insert(f.fix.fix_group_id.as_str());
        entries.insert((f.entry.file.as_str(), f.entry.name.as_str()));
        if let Some(sink) = sink_label(f) {
            sink_labels.insert(sink);
        }
    }

    let unique_sites = sites.len();
    let total_findings = findings.len() + report.omitted_findings as usize;
    let max_site_entries = sites
        .iter()
        .map(|s| s.entry_count as usize)
        .max()
        .unwrap_or(0);
    let total_entries = entries.len().max(max_site_entries);
    let total_groups = all_group_ids.len();
    let b = pal.bold;
    let dim = pal.dim;
    let ck = pal.choke;
    // Lead with distinct sink sites (the actionable unit), then findings
    // (entry × sink paths), so totals are not mistaken for site counts.
    let entry_s = if total_entries == 1 { "y" } else { "ies" };
    let site_s = if unique_sites == 1 { "" } else { "s" };
    let finding_s = if total_findings == 1 { "" } else { "s" };
    let group_s = if total_groups == 1 { "" } else { "s" };
    writeln!(
        out,
        "{b}Found {} sink site{} · {} finding{} · {} entr{} · {} diagnostic group{}{b:#}",
        unique_sites,
        site_s,
        total_findings,
        finding_s,
        total_entries,
        entry_s,
        total_groups,
        group_s
    )?;
    if let Some(top) = sites.first().filter(|s| s.finding_count > 1) {
        write_top_site_callout(out, top, ck)?;
    }
    writeln!(out)?;
    writeln!(
        out,
        "{dim}(sink sites ranked by entries; shared entry sets are clustered; pass --expand for every chain){dim:#}\n"
    )?;

    let mut sites_by_group: BTreeMap<&str, Vec<&SinkSiteSummary>> = BTreeMap::new();
    for s in sites {
        sites_by_group
            .entry(s.fix_group_id.as_str())
            .or_default()
            .push(s);
    }

    let mut ordered: Vec<_> = groups.into_iter().collect();
    ordered.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(b.0)));

    let omitted_groups = ordered.len().saturating_sub(TOP_GROUPS_SHOWN);
    let omitted_group_findings: usize = if omitted_groups > 0 {
        ordered[TOP_GROUPS_SHOWN..]
            .iter()
            .map(|(_, g)| g.len())
            .sum()
    } else {
        0
    };

    let r = pal.rule_finding;
    let fx = pal.fix;
    for (gid, group) in ordered.into_iter().take(TOP_GROUPS_SHOWN) {
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
            "{r}[{}]{r:#} {b}{}{b:#} — {} finding(s), {} entr{}, {} sink site{}",
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
        if !rep.fix.hint.is_empty() {
            writeln!(out, "  {fx}fix:{fx:#} {}", rep.fix.hint)?;
        }
        if !rep.docs_url.is_empty() {
            writeln!(out, "  docs: {}", rep.docs_url)?;
        } else if !rep.docs_anchor.is_empty() {
            writeln!(out, "  explain: async-doctor explain {}", rep.rule)?;
        }

        if let Some((file, line, text, n)) = group_choke_point(&group) {
            let short = truncate_call(&text, 72);
            writeln!(
                out,
                "  {ck}choke point:{ck:#} {file}:{line}  {short}  (in {n}/{} findings)",
                group.len()
            )?;
        }

        write_top_sink_clusters(out, &group_sites)?;

        // Representative chain: prefer a finding that hits the top sink site.
        let example = group_sites
            .first()
            .and_then(|top| {
                group.iter().find(|f| {
                    async_doctor_core::primary_sink_step(f)
                        .is_some_and(|(_, s)| s.file == top.file && s.line == top.line)
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

    if omitted_groups > 0 {
        writeln!(
            out,
            "{dim}… and {omitted_groups} more diagnostic group(s) ({omitted_group_findings} finding(s)); pass --expand to list all{dim:#}\n"
        )?;
    }
    Ok(())
}

fn format_sync_callers_note(site: &SinkSiteSummary) -> String {
    if site.sync_caller_count == 0 {
        return String::new();
    }
    let caller_s = if site.sync_caller_count == 1 { "" } else { "s" };
    match site.sample_sync_caller.as_deref() {
        Some(sample) if !sample.is_empty() => {
            format!(
                "; {} sync caller{caller_s}, e.g. {sample}",
                site.sync_caller_count
            )
        }
        _ => format!("; {} sync caller{caller_s}", site.sync_caller_count),
    }
}

fn write_top_site_callout(
    out: &mut impl Write,
    top: &SinkSiteSummary,
    ck: anstyle::Style,
) -> anyhow::Result<()> {
    let entry_s = if top.entry_count == 1 { "y" } else { "ies" };
    let sync_note = format_sync_callers_note(top);
    if top.choke_point_finding_count > 1 && !top.choke_point_file.is_empty() {
        let choke_call = truncate_call(&top.choke_point_text, 48);
        if top.choke_point_finding_count == top.finding_count {
            writeln!(
                out,
                "{ck}Top site:{ck:#} {}:{}  {choke_call} → {}:{}  (reached by {} findings across {} entr{entry_s}{sync_note})",
                top.choke_point_file,
                top.choke_point_line,
                top.file,
                top.line,
                top.finding_count,
                top.entry_count,
            )?;
        } else {
            writeln!(
                out,
                "{ck}Top site:{ck:#} {}:{}  {}  (reached by {} findings across {} entr{entry_s}; choke {}:{} in {}{sync_note})",
                top.file,
                top.line,
                truncate_call(&top.text, 48),
                top.finding_count,
                top.entry_count,
                top.choke_point_file,
                top.choke_point_line,
                top.choke_point_finding_count,
            )?;
        }
    } else {
        let call = truncate_call(&top.text, 56);
        writeln!(
            out,
            "{ck}Top site:{ck:#} {}:{}  {call}  (reached by {} findings across {} entr{entry_s}{sync_note})",
            top.file, top.line, top.finding_count, top.entry_count,
        )?;
    }
    Ok(())
}

/// Emit sink sites, clustering those that share an identical entry set.
fn write_top_sink_clusters(
    out: &mut impl Write,
    group_sites: &[&SinkSiteSummary],
) -> anyhow::Result<()> {
    writeln!(
        out,
        "  top sinks (ranked by entries; shared entry sets are clustered):"
    )?;

    let mut emitted_set_ids = BTreeSet::new();
    let mut clusters_shown = 0usize;
    let mut sites_covered = 0usize;

    for site in group_sites {
        if clusters_shown >= TOP_CLUSTERS_SHOWN {
            break;
        }
        if site.shared_entry_site_count > 1 && !site.entry_set_id.is_empty() {
            if !emitted_set_ids.insert(site.entry_set_id.as_str()) {
                continue;
            }
            let cluster: Vec<&SinkSiteSummary> = group_sites
                .iter()
                .copied()
                .filter(|s| s.entry_set_id == site.entry_set_id)
                .collect();
            sites_covered += cluster.len();
            clusters_shown += 1;
            let entries = site.entry_count;
            let findings_n: u32 = cluster.iter().map(|s| s.finding_count).sum();
            writeln!(
                out,
                "    {clusters_shown}. {} sites — same {entries} entr{}; all must be fixed \
                 (fixing one line does not clear the others):",
                cluster.len(),
                if entries == 1 { "y" } else { "ies" }
            )?;
            for s in &cluster {
                let short = truncate_call(&s.text, 64);
                let sync_note = format_sync_callers_note(s);
                writeln!(
                    out,
                    "        {}:{}  {short}  — {} finding(s){sync_note}",
                    s.file, s.line, s.finding_count
                )?;
            }
            writeln!(
                out,
                "      → together: {findings_n} finding(s), {entries} entr{}",
                if entries == 1 { "y" } else { "ies" }
            )?;
        } else {
            sites_covered += 1;
            clusters_shown += 1;
            let short = truncate_call(&site.text, 64);
            let sync_note = format_sync_callers_note(site);
            writeln!(
                out,
                "    {clusters_shown}. {}:{}  {short}  — {} finding(s), {} entr{}{sync_note}",
                site.file,
                site.line,
                site.finding_count,
                site.entry_count,
                if site.entry_count == 1 { "y" } else { "ies" }
            )?;
        }
    }

    if sites_covered < group_sites.len() {
        writeln!(
            out,
            "    … {} more sink site(s)",
            group_sites.len() - sites_covered
        )?;
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
