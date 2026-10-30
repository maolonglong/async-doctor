//! async-doctor CLI — Your agent blocks the event loop. This catches it.

mod args;
mod help_cmd;
mod text;

use std::io::Write;
use std::process::ExitCode;

use anyhow::Context;
use async_doctor_core::{AnalyzeOptions, EXIT_ERROR};
use clap::Parser;

use args::{Cli, Command, Format};

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
    let report = async_doctor_ty::scan_path_with_options(&cli.path, options)
        .with_context(|| format!("failed to scan `{}`", cli.path.display()))?;

    if !report.complete {
        for skipped in &report.skipped {
            eprintln!("warning: skipped {skipped}");
        }
        eprintln!("warning: analysis incomplete (complete=false)");
    }

    match cli.format {
        Format::Text => text::write_report(&report)?,
        Format::Json => {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            serde_json::to_writer_pretty(&mut out, &report)?;
            writeln!(out)?;
        }
    }

    Ok(ExitCode::from(report.exit_code() as u8))
}
