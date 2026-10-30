//! async-doctor CLI — Your agent blocks the event loop. This catches it.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use async_doctor_core::{AnalyzeOptions, EXIT_ERROR, Report};
use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
}

#[derive(Parser, Debug)]
#[command(
    name = "async-doctor",
    version,
    about = "Your agent blocks the event loop. This catches it.",
    long_about = "Detect sync/blocking IO reachable from Python async def code.\n\
                  Zero-config: point at a project directory and run."
)]
struct Cli {
    /// Project directory or a single `.py` file to scan (default: current directory).
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,

    /// Include entries under tests/, docs/, examples/, scripts/, and hidden dirs.
    #[arg(long)]
    include_non_production: bool,

    /// Include `reason=unresolved` candidates in the report (hidden by default).
    #[arg(long)]
    show_unresolved: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(EXIT_ERROR as u8)
        }
    }
}

fn run() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse();
    let options = AnalyzeOptions {
        include_non_production: cli.include_non_production,
        show_unresolved: cli.show_unresolved,
    };
    let report = async_doctor_ty::scan_path_with_options(&cli.path, options)
        .with_context(|| format!("failed to scan `{}`", cli.path.display()))?;

    if !report.complete {
        for skipped in &report.skipped {
            eprintln!("warning: skipped {skipped}");
        }
        eprintln!("warning: analysis incomplete (complete=false)");
    }

    match cli.format {
        Format::Text => write_text(&report)?,
        Format::Json => {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            serde_json::to_writer_pretty(&mut out, &report)?;
            writeln!(out)?;
        }
    }

    Ok(ExitCode::from(report.exit_code() as u8))
}

fn write_text(report: &Report) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    if report.findings.is_empty() {
        writeln!(out, "No findings.")?;
    } else {
        writeln!(out, "Found {} finding(s):\n", report.findings.len())?;
        for finding in &report.findings {
            writeln!(
                out,
                "[{}] reachable from {} {} {}:{}",
                finding.rule,
                finding.entry.label.as_str(),
                finding.entry.name,
                finding.entry.file,
                finding.entry.line
            )?;
            for (i, step) in finding.chain.iter().enumerate() {
                let pipe = if i + 1 == finding.chain.len() {
                    "└─"
                } else {
                    "├─"
                };
                writeln!(out, "  {pipe} {}:{}  {}", step.file, step.line, step.text)?;
            }
            if !finding.conditions.is_empty() {
                writeln!(out, "  conditions: {}", finding.conditions.join(", "))?;
            }
            writeln!(out, "  fix: {}", finding.fix.hint)?;
            writeln!(out, "  why: {}", finding.why)?;
            writeln!(out)?;
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
    Ok(())
}
