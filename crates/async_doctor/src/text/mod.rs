//! Compact human-readable report output.
//!
//! Default view groups by fix group with sink sites ranked by entry count.
//! Pass `--expand` to list every finding.

mod grouped;
pub mod style;

use std::io::Write;
use std::time::Duration;

use async_doctor_core::{
    Advice, Candidate, EntryLabel, EnvironmentKind, Finding, Report, ScanEnvironment,
};

use crate::paths::display_fs_path;
use style::Palette;

/// Default number of advice items shown before `--expand`.
pub(crate) const TOP_ADVICE_SHOWN: usize = 10;

#[derive(Default)]
pub struct TextOptions {
    /// When true, print every finding with its full chain (JSON-parity volume).
    pub expand: bool,
    /// Findings suppressed by `--baseline`.
    pub suppressed_by_baseline: u32,
    /// Findings suppressed by inline `# async-doctor: ignore[...]`.
    pub suppressed_by_ignore: u32,
    /// Total wall-clock scan duration when invoked from the CLI.
    pub elapsed: Option<Duration>,
}

pub fn write_report(report: &Report, opts: TextOptions) -> anyhow::Result<()> {
    let pal = Palette::stdout();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::with_capacity(64 * 1024, stdout.lock());
    write_environment_banner(&mut out, &report.environment, &pal)?;
    if !report.complete {
        let warn = pal.rule_candidate;
        if report.skipped.is_empty() {
            writeln!(
                out,
                "{warn}warning: incomplete analysis (complete=false) — results may omit findings{warn:#}"
            )?;
        } else {
            writeln!(
                out,
                "{warn}warning: incomplete analysis (complete=false; {} skipped) — results may omit findings{warn:#}",
                report.skipped.len()
            )?;
        }
    }
    if let Some(msg) = &report.message {
        writeln!(out, "{msg}")?;
    } else if report.findings.is_empty() {
        let ok = pal.ok;
        write!(out, "{ok}No findings.{ok:#}")?;
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
        let b = pal.bold;
        writeln!(out, "{b}Found {} finding(s):{b:#}\n", report.findings.len())?;
        for finding in &report.findings {
            write_finding_styled(&mut out, finding, &pal)?;
        }
    } else {
        grouped::write_grouped(&mut out, report, &pal)?;
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
    // Location-less unresolved-import aggregates are covered by `environment.note`
    // when present; avoid printing the same message twice in text.
    let note = &report.environment.note;
    let print_location_less = report
        .location_less_candidates
        .iter()
        .any(|c| !(c.rule == "unresolved-import" && !note.is_empty()));
    if print_location_less {
        let extras: Vec<_> = report
            .location_less_candidates
            .iter()
            .filter(|c| !(c.rule == "unresolved-import" && !note.is_empty()))
            .collect();
        if !extras.is_empty() {
            writeln!(out)?;
            for c in extras {
                writeln!(out, "[{}] {}", c.rule, c.reason)?;
                writeln!(out, "  fix: {}", c.fix.hint)?;
            }
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
    if !note.is_empty() {
        if report.environment.unresolved_third_party_modules.is_empty() {
            writeln!(out, "\nnote: {note}")?;
        } else {
            writeln!(
                out,
                "\nnote: {note} (missing: {})",
                report.environment.unresolved_third_party_modules.join(", ")
            )?;
        }
    }
    let skipped = report.environment.follow_budget_skipped;
    if skipped > 0 {
        writeln!(
            out,
            "note: third-party follow budget reached ({skipped} callees not followed); \
             deep library call chains may be missed"
        )?;
    }
    if report.message.is_none() {
        write_scan_footer(&mut out, report, opts.elapsed, &pal)?;
    }
    out.flush()?;
    Ok(())
}

fn write_scan_footer(
    out: &mut impl Write,
    report: &Report,
    elapsed: Option<Duration>,
    pal: &Palette,
) -> anyhow::Result<()> {
    let dim = pal.dim;
    if report.omitted_findings + report.omitted_candidates + report.omitted_advice > 0 {
        writeln!(
            out,
            "{dim}omitted {} finding(s), {} candidate(s), {} advice (--max-findings){dim:#}",
            report.omitted_findings, report.omitted_candidates, report.omitted_advice
        )?;
    }
    let files = report.file_count;
    let entries = report.async_entry_count;
    let file_s = if files == 1 { "" } else { "s" };
    let entry_s = if entries == 1 { "y" } else { "ies" };
    let score = report.score;
    let label = report.score_label();
    match elapsed {
        Some(d) => writeln!(
            out,
            "{dim}Scanned {files} file{file_s} · {entries} async entr{entry_s} in {}ms · Score: {score}/100 ({label}){dim:#}",
            d.as_millis()
        )?,
        None => writeln!(
            out,
            "{dim}Scanned {files} file{file_s} · {entries} async entr{entry_s} · Score: {score}/100 ({label}){dim:#}"
        )?,
    }
    Ok(())
}

/// Print detected interpreter/venv like react-doctor prints framework/project.
fn write_environment_banner(
    out: &mut impl Write,
    env: &ScanEnvironment,
    pal: &Palette,
) -> anyhow::Result<()> {
    let dim = pal.dim;
    match env.kind {
        EnvironmentKind::None => {
            writeln!(
                out,
                "{dim}Environment: none (no interpreter/venv found){dim:#}"
            )?;
        }
        EnvironmentKind::Venv | EnvironmentKind::System => {
            let kind = env.kind.as_str();
            let mut parts = vec![format!("{kind} ({})", env.origin)];
            if let Some(prefix) = &env.prefix {
                parts.push(format!("prefix {}", display_fs_path(prefix)));
            }
            if let Some(interp) = &env.interpreter {
                parts.push(format!("interpreter {}", display_fs_path(interp)));
            }
            if let Some(sp) = env.site_packages.first() {
                let extra = env.site_packages.len().saturating_sub(1);
                if extra > 0 {
                    parts.push(format!(
                        "site-packages {} (+{extra} more)",
                        display_fs_path(sp)
                    ));
                } else {
                    parts.push(format!("site-packages {}", display_fs_path(sp)));
                }
            }
            if !env.frameworks.is_empty() {
                parts.push(format!("frameworks {}", env.frameworks.join(", ")));
            }
            writeln!(out, "{dim}Environment: {}{dim:#}", parts.join(" · "))?;
        }
    }
    Ok(())
}

/// Canonical sink qualname for group headers: `blocking sink \`X\`` on the
/// primary sink hop. Falls back to the call text when absent.
pub(crate) fn sink_label(f: &Finding) -> Option<&str> {
    async_doctor_core::primary_sink_step(f).map(|(_, s)| {
        s.resolution
            .strip_prefix("blocking sink `")
            .and_then(|r| r.strip_suffix('`'))
            .unwrap_or_else(|| {
                let t = s.text.trim();
                t.split('(').next().unwrap_or(t).trim()
            })
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
    write_finding_styled(out, finding, &Palette::stdout())
}

pub fn write_finding_styled(
    out: &mut impl Write,
    finding: &Finding,
    pal: &Palette,
) -> anyhow::Result<()> {
    let r = pal.rule_finding;
    let fx = pal.fix;
    writeln!(
        out,
        "{r}[{}]{r:#} {}",
        finding.rule,
        entry_headline(finding)
    )?;
    write_chain(out, &finding.chain, "  ", true)?;
    if !finding.conditions.is_empty() {
        writeln!(out, "  conditions: {}", finding.conditions.join(", "))?;
    }
    writeln!(out, "  {fx}fix:{fx:#} {}", finding.fix.hint)?;
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
        // Already sorted by offload site file/line in the report.
        for a in &report.advice {
            write_advice(out, a)?;
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
    let pal = Palette::stdout();
    let r = pal.rule_advice;
    let fx = pal.fix;
    writeln!(
        out,
        "{r}[advice:{}]{r:#} ({})  async def {}  {}:{}",
        advice.rule,
        advice.category.as_str(),
        advice.entry.name,
        advice.entry.file,
        advice.entry.line
    )?;
    write_chain(out, &advice.chain, "  ", true)?;
    writeln!(out, "  {fx}fix:{fx:#} {}", advice.fix.hint)?;
    if !advice.docs_url.is_empty() {
        writeln!(out, "  docs: {}", advice.docs_url)?;
    }
    writeln!(out, "  explain: async-doctor explain {}", advice.rule)?;
    writeln!(out, "  why: {}", advice.why)?;
    writeln!(out)?;
    Ok(())
}

pub fn write_candidate(out: &mut impl Write, candidate: &Candidate) -> anyhow::Result<()> {
    let pal = Palette::stdout();
    let r = pal.rule_candidate;
    let fx = pal.fix;
    writeln!(
        out,
        "{r}[{}]{r:#} candidate ({})  async def {}  {}:{}",
        candidate.rule,
        candidate.reason,
        candidate.entry.name,
        candidate.entry.file,
        candidate.entry.line
    )?;
    write_chain(out, &candidate.chain, "  ", true)?;
    writeln!(out, "  {fx}fix:{fx:#} {}", candidate.fix.hint)?;
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
