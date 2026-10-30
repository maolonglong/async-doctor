//! Compact human-readable report output.

use std::io::Write;

use async_doctor_core::{Candidate, Finding, Report};

pub fn write_report(report: &Report) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    if report.findings.is_empty() {
        writeln!(out, "No findings.")?;
    } else {
        writeln!(out, "Found {} finding(s):\n", report.findings.len())?;
        for finding in &report.findings {
            write_finding(&mut out, finding)?;
        }
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

pub fn write_finding(out: &mut impl Write, finding: &Finding) -> anyhow::Result<()> {
    writeln!(
        out,
        "[{}] {}  {} {}:{}",
        finding.rule,
        finding.entry.label.as_str(),
        finding.entry.name,
        finding.entry.file,
        finding.entry.line
    )?;
    write_chain(out, &finding.chain)?;
    if !finding.conditions.is_empty() {
        writeln!(out, "  conditions: {}", finding.conditions.join(", "))?;
    }
    writeln!(out, "  fix: {}", finding.fix.hint)?;
    if !finding.docs_anchor.is_empty() {
        writeln!(
            out,
            "  explain: async-doctor explain {}  (docs/rules.md{})",
            finding.rule, finding.docs_anchor
        )?;
    }
    writeln!(out, "  why: {}", finding.why)?;
    writeln!(out)?;
    Ok(())
}

pub fn write_candidate(out: &mut impl Write, candidate: &Candidate) -> anyhow::Result<()> {
    writeln!(
        out,
        "[{}] candidate ({})  {} {}:{}",
        candidate.rule,
        candidate.reason,
        candidate.entry.name,
        candidate.entry.file,
        candidate.entry.line
    )?;
    write_chain(out, &candidate.chain)?;
    writeln!(out, "  fix: {}", candidate.fix.hint)?;
    if !candidate.docs_anchor.is_empty() {
        writeln!(
            out,
            "  explain: async-doctor explain {}  (docs/rules.md{})",
            candidate.rule, candidate.docs_anchor
        )?;
    }
    writeln!(out, "  why: {}", candidate.why)?;
    writeln!(out)?;
    Ok(())
}

fn write_chain(out: &mut impl Write, chain: &[async_doctor_core::ChainStep]) -> anyhow::Result<()> {
    for (i, step) in chain.iter().enumerate() {
        let pipe = if i + 1 == chain.len() {
            "└─"
        } else {
            "├─"
        };
        let tp = if step.third_party {
            match &step.package {
                Some(pkg) => format!("  [third-party:{pkg}]"),
                None => "  [third-party]".to_string(),
            }
        } else {
            String::new()
        };
        writeln!(
            out,
            "  {pipe} {}:{}  {}{tp}",
            step.file, step.line, step.text
        )?;
    }
    Ok(())
}
