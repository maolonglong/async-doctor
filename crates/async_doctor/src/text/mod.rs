//! Compact human-readable report output.
//!
//! Default view groups by fix group with sink sites ranked by entry count.
//! Pass `--expand` to list every finding.

mod grouped;

use std::io::Write;

use async_doctor_core::{Advice, Candidate, EntryLabel, Finding, Report};

/// Default number of advice items shown before `--expand`.
const TOP_ADVICE_SHOWN: usize = 10;

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
        grouped::write_grouped(&mut out, &report.findings)?;
    }

    write_advice_section(&mut out, report, opts.expand)?;

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

pub(crate) fn sink_label(f: &Finding) -> Option<&str> {
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

pub(crate) fn write_finding_indented(
    out: &mut impl Write,
    finding: &Finding,
    indent: &str,
) -> anyhow::Result<()> {
    writeln!(out, "{indent}{}", entry_headline(finding))?;
    write_chain(out, &finding.chain, indent, false)?;
    Ok(())
}

fn write_advice_section(out: &mut impl Write, report: &Report, expand: bool) -> anyhow::Result<()> {
    if report.advice.is_empty() {
        return Ok(());
    }
    let total = report.advice.len();
    writeln!(out)?;
    if expand || total <= TOP_ADVICE_SHOWN {
        writeln!(out, "Advice ({total}) — does not fail the run:\n")?;
        // Group by fix_group_id (offload site + twin).
        let mut groups: std::collections::BTreeMap<&str, Vec<&Advice>> =
            std::collections::BTreeMap::new();
        for a in &report.advice {
            groups
                .entry(a.fix.fix_group_id.as_str())
                .or_default()
                .push(a);
        }
        for (_gid, items) in groups {
            for a in items {
                write_advice(out, a)?;
            }
        }
    } else {
        writeln!(
            out,
            "Advice ({total}) — showing top {TOP_ADVICE_SHOWN} (pass --expand for all); does not fail the run:\n"
        )?;
        for a in report.advice.iter().take(TOP_ADVICE_SHOWN) {
            write_advice(out, a)?;
        }
    }
    Ok(())
}

pub fn write_advice(out: &mut impl Write, advice: &Advice) -> anyhow::Result<()> {
    writeln!(
        out,
        "[advice:{}] ({})  async def {}  {}:{}",
        advice.rule,
        advice.category.as_str(),
        advice.entry.name,
        advice.entry.file,
        advice.entry.line
    )?;
    write_chain(out, &advice.chain, "  ", true)?;
    writeln!(out, "  fix: {}", advice.fix.hint)?;
    if !advice.docs_url.is_empty() {
        writeln!(out, "  docs: {}", advice.docs_url)?;
    }
    writeln!(out, "  explain: async-doctor explain {}", advice.rule)?;
    writeln!(out, "  why: {}", advice.why)?;
    writeln!(out)?;
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
