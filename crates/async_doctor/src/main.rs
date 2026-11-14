//! async-doctor CLI — Your agent blocks the event loop. This catches it.

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// Short page decay returns freed Salsa wave pages to the OS promptly between waves.
#[cfg(not(target_env = "msvc"))]
#[allow(non_upper_case_globals)]
#[unsafe(export_name = "_rjem_malloc_conf")]
pub static malloc_conf: Option<&'static u8> =
    Some(&b"dirty_decay_ms:1000,muzzy_decay_ms:1000\0"[0]);

mod args;
mod baseline;
mod changed;
mod help_cmd;
mod markdown;
mod paths;
mod sarif;
mod skill_cmd;
mod suppress;
mod text;

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Context, bail};
use async_doctor_core::{AnalyzeOptions, EXIT_ERROR, EXIT_OK, Report};
use clap::Parser;

use args::{Cli, Command, FailOn, Format};
use changed::ChangedSet;
use paths::{cli_path_arg, resolve_cli_path};

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
            python,
            format,
            expand,
            include_non_production,
            show_unresolved,
        }) => help_cmd::run_why(
            &location,
            &path,
            python.as_deref(),
            format,
            expand,
            include_non_production,
            show_unresolved,
        ),
        Some(Command::InstallSkill { path, force }) => skill_cmd::run_install_skill(&path, force),
        None => run_scan(cli),
    }
}

fn run_scan(cli: Cli) -> anyhow::Result<ExitCode> {
    if cli.write_baseline.is_some() && cli.changed.is_some() {
        bail!(
            "--write-baseline cannot be combined with --changed / --diff \
             (would write an incomplete baseline); omit --changed, or write \
             the baseline from a full scan first"
        );
    }

    let options = AnalyzeOptions {
        include_non_production: cli.include_non_production,
        show_unresolved: cli.show_unresolved,
    };
    let resolved_python = cli.python.as_deref().map(resolve_cli_path);
    let scan_opts = async_doctor_ty::ScanOptions {
        analyze: options,
        python: resolved_python,
        cache_dir: None,
    };
    let t_start = Instant::now();
    // Absolute so bare `file.py` is not treated as having an empty parent.
    // Keep the original CLI path for `why --path` so relative inputs (and
    // symlink leaves) stay short and runnable instead of long `../` climbs.
    let path = resolve_cli_path(&cli.path);
    std::fs::metadata(&path).with_context(|| format!("failed to scan `{}`", path.display()))?;

    let scan_root = if path.is_file() {
        path.parent().unwrap_or(path.as_path()).to_path_buf()
    } else {
        path.clone()
    };
    let scan_root = dunce::canonicalize(&scan_root).unwrap_or(scan_root);
    let single_file = path.is_file().then_some(path.as_path());

    // Prefer git toplevel for SARIF %SRCROOT%; fall back to scan root.
    let mut repo_root_for_sarif = if matches!(cli.format, Format::Sarif) {
        changed::repo_root_for(&path).ok()
    } else {
        None
    };

    let pre_changed = cli.changed.as_ref().map(|ref_spec| {
        let git_ref = if ref_spec.is_empty() || ref_spec == "AUTO" {
            None
        } else {
            Some(ref_spec.as_str())
        };
        changed::resolve_changed(&path, git_ref)
    });

    let short_circuit_clean = match &pre_changed {
        Some(ChangedSet::Filter {
            files,
            repo_root,
            note,
        }) => {
            if let Some(note) = note {
                eprintln!("note: {note}");
            }
            repo_root_for_sarif = Some(repo_root.clone());
            !changed::has_python_or_config_changes(files)
        }
        Some(ChangedSet::FullScan { note }) => {
            eprintln!("note: {note}");
            false
        }
        None => false,
    };

    let (mut report, suppressed_by_ignore, elapsed) = if short_circuit_clean {
        if let Some(py) = &scan_opts.python {
            std::fs::metadata(py)
                .with_context(|| format!("invalid Python environment `{}`", py.display()))?;
        }
        (Report::empty_complete(), 0u32, t_start.elapsed())
    } else {
        let mut report = async_doctor_ty::scan_path_with_scan_options(&path, &scan_opts)
            .with_context(|| format!("failed to scan `{}`", path.display()))?;
        let elapsed = t_start.elapsed();

        rewrite_why_commands(&mut report, &cli.path, cli.python.as_deref());

        if !report.complete {
            for skipped in &report.skipped {
                eprintln!("warning: skipped {skipped}");
            }
            eprintln!("warning: analysis incomplete (complete=false)");
        }

        let suppress_out = suppress::apply_scoped(report, &scan_root, single_file)?;
        for m in &suppress_out.missing_reason {
            eprintln!(
                "warning: {}:{}: `# async-doctor: ignore[{}]` requires a reason \
                 (`# async-doctor: ignore[{}]: <why>`)",
                m.file, m.line, m.rule, m.rule
            );
        }
        let suppressed_by_ignore = suppress_out.suppressed_findings;
        let mut report = suppress_out.report;

        if let Some(ChangedSet::Filter {
            files, repo_root, ..
        }) = &pre_changed
        {
            report = changed::filter_report(report, files, &scan_root, repo_root);
        }
        (report, suppressed_by_ignore, elapsed)
    };

    let writing_baseline = cli.write_baseline.is_some();
    if let Some(path) = &cli.write_baseline {
        baseline::write_baseline(path, &report)?;
        eprintln!(
            "wrote baseline ({} id(s)) to {}",
            report.findings.len(),
            path.display()
        );
    }

    let mut suppressed_by_baseline = 0u32;
    if let Some(path) = &cli.baseline {
        let bl = baseline::read_baseline(path)?;
        let (filtered, n) = baseline::filter_report(report, &bl);
        report = filtered;
        suppressed_by_baseline = n;
    }

    if suppressed_by_ignore > 0
        || !report.unused_suppressions.is_empty()
        || cli.changed.is_some()
        || suppressed_by_baseline > 0
    {
        report.refresh_sink_sites();
    } else {
        report.refresh_score();
    }

    // Output-only cap: score, exit code and sinkSites stay full-report.
    let exit_code = report.exit_code();
    if let Some(n) = cli.max_findings {
        report.apply_max_findings(n);
    }

    if cli.score {
        println!("{}", report.score);
    } else {
        match cli.format {
            Format::Text => text::write_report(
                &report,
                text::TextOptions {
                    expand: cli.expand,
                    suppressed_by_baseline,
                    suppressed_by_ignore,
                    elapsed: Some(elapsed),
                },
            )?,
            Format::Markdown => markdown::write_markdown(
                &report,
                markdown::MarkdownOptions {
                    expand: cli.expand,
                    suppressed_by_baseline,
                    suppressed_by_ignore,
                    elapsed: Some(elapsed),
                },
            )?,
            Format::Json => write_json_stdout(&report)?,
            Format::Sarif => {
                let src_root = repo_root_for_sarif
                    .as_deref()
                    .unwrap_or(scan_root.as_path());
                let doc = sarif::report_to_sarif(&report, &scan_root, src_root);
                let stdout = std::io::stdout();
                let mut out = std::io::BufWriter::with_capacity(64 * 1024, stdout.lock());
                serde_json::to_writer_pretty(&mut out, &doc)?;
                writeln!(out)?;
                out.flush()?;
            }
        }
    }

    if writing_baseline || matches!(cli.fail_on, FailOn::None) {
        return Ok(ExitCode::from(EXIT_OK as u8));
    }

    Ok(ExitCode::from(exit_code as u8))
}

pub(crate) fn write_json_stdout<T: serde::Serialize>(value: &T) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let is_tty = stdout.is_terminal();
    let mut out = std::io::BufWriter::with_capacity(64 * 1024, stdout.lock());
    if is_tty {
        serde_json::to_writer_pretty(&mut out, value)?;
    } else {
        serde_json::to_writer(&mut out, value)?;
    }
    writeln!(out)?;
    out.flush()?;
    Ok(())
}

pub(crate) fn extract_why_locator(why: &str) -> Option<&str> {
    let rest = why.strip_prefix("async-doctor why ")?.trim();
    let loc = rest.split_once(" --path ").map(|(l, _)| l).unwrap_or(rest);
    let loc = loc
        .split_once(" --python ")
        .map(|(l, _)| l)
        .unwrap_or(loc)
        .trim();
    let loc = loc
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| loc.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(loc);
    (!loc.is_empty() && loc.contains(':')).then_some(loc)
}

pub(crate) fn rewrite_why_commands(report: &mut Report, scan_path: &Path, python: Option<&Path>) {
    let path_arg = cli_path_arg(scan_path);
    let python_arg = python.map(cli_path_arg);
    let rewrite = |why: &mut String| {
        let Some(loc) = extract_why_locator(why) else {
            return;
        };
        let loc_arg = if loc.chars().any(|c| c.is_whitespace()) {
            format!("\"{loc}\"")
        } else {
            loc.to_string()
        };
        *why = match &python_arg {
            Some(py) => format!("async-doctor why {loc_arg} --path {path_arg} --python {py}"),
            None => format!("async-doctor why {loc_arg} --path {path_arg}"),
        };
    };
    for f in &mut report.findings {
        rewrite(&mut f.why);
    }
    for c in &mut report.candidates {
        rewrite(&mut c.why);
    }
    // locationLessCandidates intentionally leave `why` empty (no file:line).
    for a in &mut report.advice {
        rewrite(&mut a.why);
    }
}
