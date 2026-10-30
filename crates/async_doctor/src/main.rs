//! async-doctor CLI — Your agent blocks the event loop. This catches it.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use async_doctor_core::{EXIT_ERROR, Report};
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
    let report = async_doctor_ty::scan_path(&cli.path)
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
        if !report.candidates.is_empty() {
            writeln!(
                out,
                "({} candidate(s) omitted from exit status)",
                report.candidates.len()
            )?;
        }
        return Ok(());
    }

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
    Ok(())
}
