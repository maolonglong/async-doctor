//! async-doctor CLI — Your agent blocks the event loop. This catches it.

mod args;
mod help_cmd;
mod paths;
mod text;

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use anyhow::Context;
use async_doctor_core::{AnalyzeOptions, EXIT_ERROR, Report};
use clap::Parser;

use args::{Cli, Command, Format};
use paths::cli_path_arg;

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
    match cli.command {
        Some(Command::Explain { rule }) => help_cmd::run_explain(&rule),
        Some(Command::Why {
            location,
            path,
            include_non_production,
            show_unresolved,
        }) => help_cmd::run_why(&location, &path, include_non_production, show_unresolved),
        None => run_scan(cli),
    }
}

fn run_scan(cli: Cli) -> anyhow::Result<ExitCode> {
    let options = AnalyzeOptions {
        include_non_production: cli.include_non_production,
        show_unresolved: cli.show_unresolved,
    };
    let mut report = async_doctor_ty::scan_path_with_options(&cli.path, options)
        .with_context(|| format!("failed to scan `{}`", cli.path.display()))?;

    rewrite_why_commands(&mut report, &cli.path);

    if !report.complete {
        for skipped in &report.skipped {
            eprintln!("warning: skipped {skipped}");
        }
        eprintln!("warning: analysis incomplete (complete=false)");
    }

    match cli.format {
        Format::Text => text::write_report(&report, text::TextOptions { expand: cli.expand })?,
        Format::Json => {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            serde_json::to_writer_pretty(&mut out, &report)?;
            writeln!(out)?;
        }
    }

    Ok(ExitCode::from(report.exit_code() as u8))
}

/// Make each finding's `why` field runnable as-is from any cwd.
fn rewrite_why_commands(report: &mut Report, scan_path: &Path) {
    let path_arg = cli_path_arg(scan_path);
    let rewrite = |why: &mut String| {
        if let Some(rest) = why.strip_prefix("async-doctor why ") {
            let loc = rest.split_whitespace().next().unwrap_or(rest);
            *why = format!("async-doctor why {loc} --path {path_arg}");
        }
    };
    for f in &mut report.findings {
        rewrite(&mut f.why);
    }
    for c in &mut report.candidates {
        rewrite(&mut c.why);
    }
}
