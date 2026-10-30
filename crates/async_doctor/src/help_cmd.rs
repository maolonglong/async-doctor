//! `explain` and `why` subcommands — agent-facing long-form help.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use async_doctor_core::{AnalyzeOptions, EXIT_FINDINGS, EXIT_OK, Report};
use async_doctor_rules::rule_doc;

use crate::text::{write_candidate, write_finding};

pub fn run_explain(rule: &str) -> anyhow::Result<ExitCode> {
    let Some(doc) = rule_doc(rule) else {
        let known: Vec<&str> = async_doctor_rules::RULE_DOCS.iter().map(|r| r.id).collect();
        bail!("unknown rule `{rule}`.\nKnown rules: {}", known.join(", "));
    };
    let mut out = std::io::stdout().lock();
    write!(out, "{}", doc.explain_text())?;
    Ok(ExitCode::from(EXIT_OK as u8))
}

pub fn run_why(
    location: &str,
    path: &Path,
    include_non_production: bool,
    show_unresolved: bool,
) -> anyhow::Result<ExitCode> {
    let (file, line) = parse_location(location)
        .with_context(|| format!("invalid location `{location}` (expected file:line)"))?;

    let options = AnalyzeOptions {
        include_non_production,
        show_unresolved,
    };
    let report = async_doctor_ty::scan_path_with_options(path, options)
        .with_context(|| format!("failed to scan `{}`", path.display()))?;

    let matches = findings_at(&report, &file, line);
    let mut out = std::io::stdout().lock();
    if matches.findings.is_empty() && matches.candidates.is_empty() {
        writeln!(
            out,
            "No findings or candidates touch {file}:{line} under {}.",
            path.display()
        )?;
        writeln!(
            out,
            "Hint: paths are reported relative to the scan root; try the path from JSON output."
        )?;
        return Ok(ExitCode::from(EXIT_OK as u8));
    }

    writeln!(
        out,
        "Evidence for {file}:{line} ({} finding(s), {} candidate(s)):\n",
        matches.findings.len(),
        matches.candidates.len()
    )?;
    for f in &matches.findings {
        write_finding(&mut out, f)?;
        if let Some(doc) = rule_doc(&f.rule) {
            writeln!(out, "{}", doc.explain_text())?;
        }
    }
    for c in &matches.candidates {
        write_candidate(&mut out, c)?;
        if let Some(doc) = rule_doc(&c.rule) {
            writeln!(out, "{}", doc.explain_text())?;
        }
    }

    let code = if matches.findings.is_empty() {
        EXIT_OK
    } else {
        EXIT_FINDINGS
    };
    Ok(ExitCode::from(code as u8))
}

struct SiteMatches {
    findings: Vec<async_doctor_core::Finding>,
    candidates: Vec<async_doctor_core::Candidate>,
}

fn findings_at(report: &Report, file: &str, line: u32) -> SiteMatches {
    let file_norm = normalize_path(file);
    let touch = |f: &str, l: u32| l == line && paths_equal(f, &file_norm);

    let findings = report
        .findings
        .iter()
        .filter(|f| {
            touch(&f.entry.file, f.entry.line)
                || f.chain.iter().any(|s| touch(&s.file, s.line))
                || why_matches(&f.why, &file_norm, line)
        })
        .cloned()
        .collect();
    let candidates = report
        .candidates
        .iter()
        .filter(|c| {
            touch(&c.entry.file, c.entry.line)
                || c.chain.iter().any(|s| touch(&s.file, s.line))
                || why_matches(&c.why, &file_norm, line)
        })
        .cloned()
        .collect();
    SiteMatches {
        findings,
        candidates,
    }
}

fn why_matches(why: &str, file: &str, line: u32) -> bool {
    // `async-doctor why helpers.py:5`
    let Some(rest) = why.strip_prefix("async-doctor why ") else {
        return false;
    };
    parse_location(rest)
        .map(|(f, l)| l == line && paths_equal(&f, file))
        .unwrap_or(false)
}

fn parse_location(location: &str) -> anyhow::Result<(String, u32)> {
    let location = location.trim();
    let Some((file, line_s)) = location.rsplit_once(':') else {
        bail!("missing :line");
    };
    let line: u32 = line_s.parse().context("line must be an integer")?;
    if file.is_empty() || line == 0 {
        bail!("file and line must be non-empty / positive");
    }
    Ok((normalize_path(file), line))
}

fn normalize_path(p: &str) -> String {
    let p = p.replace('\\', "/");
    PathBuf::from(p.trim_start_matches("./"))
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn paths_equal(a: &str, b: &str) -> bool {
    let a = normalize_path(a);
    let b = normalize_path(b);
    a == b || a.ends_with(&b) || b.ends_with(&a)
}
