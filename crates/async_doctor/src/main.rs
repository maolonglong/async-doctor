//! async-doctor CLI — Your agent blocks the event loop. This catches it.

mod args;
mod baseline;
mod changed;
mod help_cmd;
mod paths;
mod suppress;
mod text;

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, bail};
use async_doctor_core::{AnalyzeOptions, EXIT_ERROR, EXIT_OK, Report};
use clap::Parser;

use args::{Cli, Command, Format};
use changed::ChangedSet;
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
    if cli.write_baseline.is_some() && cli.changed.is_some() {
        bail!(
            "--write-baseline cannot be combined with --changed \
             (would write an incomplete baseline); omit --changed, or write \
             the baseline from a full scan first"
        );
    }

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

    let scan_root = if cli.path.is_file() {
        cli.path
            .parent()
            .unwrap_or(cli.path.as_path())
            .to_path_buf()
    } else {
        cli.path.clone()
    };
    let scan_root = scan_root.canonicalize().unwrap_or(scan_root);

    // Inline suppressions (always on — zero-config escape hatch).
    let suppress_out = suppress::apply(report, &scan_root)?;
    for m in &suppress_out.missing_reason {
        eprintln!(
            "warning: {}:{}: `# async-doctor: ignore[{}]` requires a reason \
             (`# async-doctor: ignore[{}]: <why>`)",
            m.file, m.line, m.rule, m.rule
        );
    }
    let suppressed_by_ignore = suppress_out.suppressed_findings;
    let mut report = suppress_out.report;

    // --changed: full analysis, filtered reporting (graceful fallback).
    if let Some(ref_spec) = &cli.changed {
        let git_ref = if ref_spec.is_empty() || ref_spec == "AUTO" {
            None
        } else {
            Some(ref_spec.as_str())
        };
        match changed::resolve_changed(&cli.path, git_ref) {
            ChangedSet::Filter {
                files,
                repo_root,
                note,
            } => {
                if let Some(note) = note {
                    eprintln!("note: {note}");
                }
                report = changed::filter_report(report, &files, &scan_root, &repo_root);
            }
            ChangedSet::FullScan { note } => {
                eprintln!("note: {note}");
            }
        }
    }

    // Write baseline from the post-suppression set (never after --changed).
    let writing_baseline = cli.write_baseline.is_some();
    if let Some(path) = &cli.write_baseline {
        baseline::write_baseline(path, &report)?;
        eprintln!(
            "wrote baseline ({} id(s)) to {}",
            report.findings.len(),
            path.display()
        );
    }

    // --baseline: suppress known stable ids; only new findings fail.
    let mut suppressed_by_baseline = 0u32;
    if let Some(path) = &cli.baseline {
        let bl = baseline::read_baseline(path)?;
        let (filtered, n) = baseline::filter_report(report, &bl);
        report = filtered;
        suppressed_by_baseline = n;
    }

    match cli.format {
        Format::Text => text::write_report(
            &report,
            text::TextOptions {
                expand: cli.expand,
                suppressed_by_baseline,
                suppressed_by_ignore,
            },
        )?,
        Format::Json => {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            serde_json::to_writer_pretty(&mut out, &report)?;
            writeln!(out)?;
        }
    }

    // Recording a baseline is a bookkeeping action — always exit 0 on success.
    if writing_baseline {
        return Ok(ExitCode::from(EXIT_OK as u8));
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
