//! Compact human-readable report output.
//!
//! Default view groups by fix group (rule + sink) with counts and one
//! representative chain. Pass `--expand` to list every finding.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use async_doctor_core::{Candidate, EntryLabel, Finding, Report};

#[derive(Default)]
pub struct TextOptions {
    /// When true, print every finding with its full chain (JSON-parity volume).
    pub expand: bool,
    /// Findings suppressed by `--baseline`.
    pub suppressed_by_baseline: u32,
    /// Findings suppressed by inline `# async-doctor: ignore[...]`.
    pub suppressed_by_ignore: u32,
}

pub fn write_report(report: &Report, opts: TextOptions) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    if report.findings.is_empty() {
        write!(out, "No findings.")?;
        let mut parts = Vec::new();
        if opts.suppressed_by_baseline > 0 {
            parts.push(format!(
                "{} suppressed by baseline",
                opts.suppressed_by_baseline
            ));
        }
        if opts.suppressed_by_ignore > 0 {
            parts.push(format!(
                "{} suppressed by inline ignore",
                opts.suppressed_by_ignore
            ));
        }
        if !parts.is_empty() {
            write!(out, " ({})", parts.join(", "))?;
        }
        writeln!(out)?;
    } else if opts.expand {
        writeln!(out, "Found {} finding(s):\n", report.findings.len())?;
        for finding in &report.findings {
            write_finding(&mut out, finding)?;
        }
    } else {
        write_grouped(&mut out, &report.findings)?;
    }

    if !report.candidates.is_empty() {
        writeln!(
            out,
            "({} candidate(s) omitted from exit status)",
            report.candidates.len()
        )?;
    }
    if report.omitted_non_production_entries > 0 {
        writeln!(
            out,
            "(skipped {} non-production async entr{}, use --include-non-production to include)",
            report.omitted_non_production_entries,
            if report.omitted_non_production_entries == 1 {
                "y"
            } else {
                "ies"
            }
        )?;
    }
    if report.hidden_unresolved_candidates > 0 {
        writeln!(
            out,
            "(hid {} unresolved candidate{}, use --show-unresolved to include)",
            report.hidden_unresolved_candidates,
            if report.hidden_unresolved_candidates == 1 {
                ""
            } else {
                "s"
            }
        )?;
    }
    if !report.unresolved_summary.is_empty() {
        writeln!(out, "\nUnresolved patterns (by entries affected):")?;
        for pat in &report.unresolved_summary {
            writeln!(
                out,
                "  {} — {} entr{}, {} site{}",
                pat.pattern,
                pat.entries_affected,
                if pat.entries_affected == 1 {
                    "y"
                } else {
                    "ies"
                },
                pat.sites,
                if pat.sites == 1 { "" } else { "s" }
            )?;
        }
    }
    if !report.unused_suppressions.is_empty() {
        writeln!(
            out,
            "\nUnused suppressions ({}):",
            report.unused_suppressions.len()
        )?;
        for u in &report.unused_suppressions {
            writeln!(
                out,
                "  {}:{}  ignore[{}] — {}",
                u.file, u.line, u.rule, u.reason
            )?;
        }
    }
    Ok(())
}

fn write_grouped(out: &mut impl Write, findings: &[Finding]) -> anyhow::Result<()> {
    let mut groups: BTreeMap<&str, Vec<&Finding>> = BTreeMap::new();
    let mut entries: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut sinks: BTreeSet<&str> = BTreeSet::new();
    for f in findings {
        groups
            .entry(f.fix.fix_group_id.as_str())
            .or_default()
            .push(f);
        entries.insert((f.entry.file.as_str(), f.entry.name.as_str()));
        if let Some(sink) = sink_label(f) {
            sinks.insert(sink);
        }
    }

    writeln!(
        out,
        "Found {} finding(s) · {} entries · {} sinks · {} fix groups\n",
        findings.len(),
        entries.len(),
        sinks.len(),
        groups.len()
    )?;
    writeln!(
        out,
        "(grouped by fix; pass --expand to list every finding)\n"
    )?;

    // Largest groups first for signal.
    let mut ordered: Vec<_> = groups.into_iter().collect();
    ordered.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(b.0)));

    for (_gid, group) in ordered {
        let rep = group[0];
        let mut group_entries: BTreeSet<&str> = BTreeSet::new();
        let mut group_sinks: BTreeSet<&str> = BTreeSet::new();
        for f in &group {
            group_entries.insert(f.entry.name.as_str());
            if let Some(s) = sink_label(f) {
                group_sinks.insert(s);
            }
        }
        let sink_summary = group_sinks.iter().copied().take(3).collect::<Vec<_>>();
        let sink_extra = group_sinks.len().saturating_sub(sink_summary.len());
        let sink_text = if sink_extra > 0 {
            format!("{} (+{sink_extra} more)", sink_summary.join(", "))
        } else {
            sink_summary.join(", ")
        };

        writeln!(
            out,
            "[{}] {} — {} finding(s), {} entr{}",
            rep.rule,
            if sink_text.is_empty() {
                "sink".to_string()
            } else {
                sink_text
            },
            group.len(),
            group_entries.len(),
            if group_entries.len() == 1 { "y" } else { "ies" }
        )?;
        writeln!(out, "  fix: {}", rep.fix.hint)?;
        if !rep.docs_url.is_empty() {
            writeln!(out, "  docs: {}", rep.docs_url)?;
        } else if !rep.docs_anchor.is_empty() {
            writeln!(out, "  explain: async-doctor explain {}", rep.rule)?;
        }
        writeln!(out, "  example:")?;
        write_finding_indented(out, rep, "    ")?;
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

fn sink_label(f: &Finding) -> Option<&str> {
    f.chain.last().map(|s| {
        let t = s.text.trim();
        t.split('(').next().unwrap_or(t).trim()
    })
}

pub fn entry_headline(finding: &Finding) -> String {
    let kind = match finding.entry.label {
        EntryLabel::Other => String::new(),
        other => format!("{} ", other.as_str()),
    };
    format!(
        "{kind}async def {}  {}:{}",
        finding.entry.name, finding.entry.file, finding.entry.line
    )
}

pub fn write_finding(out: &mut impl Write, finding: &Finding) -> anyhow::Result<()> {
    writeln!(out, "[{}] {}", finding.rule, entry_headline(finding))?;
    write_chain(out, &finding.chain, "  ", true)?;
    if !finding.conditions.is_empty() {
        writeln!(out, "  conditions: {}", finding.conditions.join(", "))?;
    }
    writeln!(out, "  fix: {}", finding.fix.hint)?;
    if !finding.docs_url.is_empty() {
        writeln!(out, "  docs: {}", finding.docs_url)?;
    }
    writeln!(out, "  explain: async-doctor explain {}", finding.rule)?;
    writeln!(out, "  why: {}", finding.why)?;
    writeln!(out)?;
    Ok(())
}

fn write_finding_indented(
    out: &mut impl Write,
    finding: &Finding,
    indent: &str,
) -> anyhow::Result<()> {
    writeln!(out, "{indent}{}", entry_headline(finding))?;
    write_chain(out, &finding.chain, indent, false)?;
    Ok(())
}

pub fn write_candidate(out: &mut impl Write, candidate: &Candidate) -> anyhow::Result<()> {
    writeln!(
        out,
        "[{}] candidate ({})  async def {}  {}:{}",
        candidate.rule,
        candidate.reason,
        candidate.entry.name,
        candidate.entry.file,
        candidate.entry.line
    )?;
    write_chain(out, &candidate.chain, "  ", true)?;
    writeln!(out, "  fix: {}", candidate.fix.hint)?;
    if !candidate.docs_url.is_empty() {
        writeln!(out, "  docs: {}", candidate.docs_url)?;
    }
    writeln!(out, "  explain: async-doctor explain {}", candidate.rule)?;
    writeln!(out, "  why: {}", candidate.why)?;
    writeln!(out)?;
    Ok(())
}

fn write_chain(
    out: &mut impl Write,
    chain: &[async_doctor_core::ChainStep],
    indent: &str,
    show_resolution: bool,
) -> anyhow::Result<()> {
    for (i, step) in chain.iter().enumerate() {
        let pipe = if i + 1 == chain.len() {
            "└─"
        } else {
            "├─"
        };
        let mut suffix = String::new();
        if step.third_party {
            match &step.package {
                Some(pkg) => suffix.push_str(&format!("  [third-party:{pkg}]")),
                None => suffix.push_str("  [third-party]"),
            }
        }
        if show_resolution && !step.resolution.is_empty() {
            suffix.push_str(&format!("  ({})", step.resolution));
        }
        writeln!(
            out,
            "{indent}{pipe} {}:{}  {}{suffix}",
            step.file, step.line, step.text
        )?;
    }
    Ok(())
}
