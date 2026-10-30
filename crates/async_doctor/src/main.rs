//! async-doctor CLI — Your agent blocks the event loop. This catches it.

mod args;
mod baseline;
mod changed;
mod help_cmd;
mod suppress;
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
    let mut report = async_doctor_ty::scan_path_with_options(&cli.path, options)
        .with_context(|| format!("failed to scan `{}`", cli.path.display()))?;

    if !report.complete {
        for skipped in &report.skipped {
            eprintln!("warning: skipped {skipped}");
        }
        eprintln!("warning: analysis incomplete (complete=false)");
    }

    // Inline suppressions (always on — zero-config escape hatch).
    let scan_root = if cli.path.is_file() {
        cli.path
            .parent()
            .unwrap_or(cli.path.as_path())
            .to_path_buf()
    } else {
        cli.path.clone()
    };
    let scan_root = scan_root.canonicalize().unwrap_or(scan_root);
    report = suppress::apply(report, &scan_root)?.report;

    // --changed: full analysis, filtered reporting.
    if let Some(ref_spec) = &cli.changed {
        let git_ref = if ref_spec.is_empty() || ref_spec == "AUTO" {
            None
        } else {
            Some(ref_spec.as_str())
        };
        let repo = changed::repo_root_for(&cli.path)
            .with_context(|| format!("--changed requires a git repo at {}", cli.path.display()))?;
        let files = changed::changed_files(&repo, git_ref)?;
        report = changed::filter_report(report, &files);
    }

    // Write baseline from the post-suppression / post-changed set, before
    // --baseline filtering (so refreshing a baseline is not a no-op).
    if let Some(path) = &cli.write_baseline {
        baseline::write_baseline(path, &report)?;
        eprintln!(
            "wrote baseline ({} id(s)) to {}",
            report.findings.len(),
            path.display()
        );
    }

    // --baseline: suppress known stable ids; only new findings fail.
    if let Some(path) = &cli.baseline {
        let bl = baseline::read_baseline(path)?;
        report = baseline::filter_report(report, &bl);
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
