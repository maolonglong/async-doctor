//! `explain` and `why` subcommands — agent-facing long-form help.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, bail};
use async_doctor_core::{AnalyzeOptions, EXIT_FINDINGS, EXIT_OK, Report, SCHEMA_VERSION};
use async_doctor_rules::rule_doc;
use serde_json::json;

use crate::args::WhyFormat;
use crate::paths::{normalize_path, paths_equal, resolve_cli_path};
use crate::text::{write_advice, write_candidate, write_finding};

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
    format: WhyFormat,
    include_non_production: bool,
    show_unresolved: bool,
) -> anyhow::Result<ExitCode> {
    let (file, line) = parse_location(location)
        .with_context(|| format!("invalid location `{location}` (expected file:line)"))?;

    let options = AnalyzeOptions {
        include_non_production,
        show_unresolved,
    };
    let abs_path = resolve_cli_path(path);
    let mut report = async_doctor_ty::scan_path_with_options(&abs_path, options)
        .with_context(|| format!("failed to scan `{}`", abs_path.display()))?;
    // Same rewrite and inline-suppression pass the default scan applies.
    crate::rewrite_why_commands(&mut report, path);
    let scan_root = if abs_path.is_file() {
        abs_path
            .parent()
            .unwrap_or(abs_path.as_path())
            .to_path_buf()
    } else {
        abs_path.clone()
    };
    let scan_root = dunce::canonicalize(&scan_root).unwrap_or(scan_root);
    let single_file = abs_path.is_file().then_some(abs_path.as_path());
    let report = crate::suppress::apply_scoped(report, &scan_root, single_file)?.report;

    let matches = findings_at(&report, &file, line);
    let code = if matches.findings.is_empty() {
        EXIT_OK
    } else {
        EXIT_FINDINGS
    };

    let mut out = std::io::stdout().lock();
    if matches!(format, WhyFormat::Json) {
        let mut seen_rules = BTreeSet::new();
        for f in &matches.findings {
            seen_rules.insert(f.rule.clone());
        }
        for c in &matches.candidates {
            seen_rules.insert(c.rule.clone());
        }
        for a in &matches.advice {
            seen_rules.insert(a.rule.clone());
        }
        let rules: Vec<_> = seen_rules
            .into_iter()
            .map(|id| {
                if let Some(doc) = rule_doc(&id) {
                    json!({
                        "id": doc.id,
                        "title": doc.title,
                        "summary": doc.summary,
                        "body": doc.body,
                        "fix": doc.fix,
                        "docsAnchor": doc.docs_anchor(),
                        "docsUrl": doc.docs_url(),
                    })
                } else {
                    json!({
                        "id": id,
                        "docsAnchor": format!("#{id}"),
                    })
                }
            })
            .collect();
        let doc = json!({
            "schemaVersion": SCHEMA_VERSION,
            "location": {
                "file": file,
                "line": line,
            },
            "findings": matches.findings,
            "candidates": matches.candidates,
            "advice": matches.advice,
            "rules": rules,
        });
        serde_json::to_writer_pretty(&mut out, &doc)?;
        writeln!(out)?;
        return Ok(ExitCode::from(code as u8));
    }

    if matches.findings.is_empty() && matches.candidates.is_empty() && matches.advice.is_empty() {
        writeln!(
            out,
            "No findings, candidates, or advice touch {file}:{line} under {}.",
            abs_path.display()
        )?;
        writeln!(
            out,
            "Hint: paths are reported relative to the scan root; try the path from JSON output."
        )?;
        return Ok(ExitCode::from(EXIT_OK as u8));
    }

    writeln!(
        out,
        "Evidence for {file}:{line} ({} finding(s), {} candidate(s), {} advice):\n",
        matches.findings.len(),
        matches.candidates.len(),
        matches.advice.len()
    )?;
    for f in &matches.findings {
        write_finding(&mut out, f)?;
        writeln!(out, "Hop resolution:")?;
        for (i, step) in f.chain.iter().enumerate() {
            let res = if step.resolution.is_empty() {
                "(unspecified)"
            } else {
                step.resolution.as_str()
            };
            writeln!(out, "  {}. {}:{} — {res}", i + 1, step.file, step.line)?;
        }
        writeln!(out)?;
        if let Some(doc) = rule_doc(&f.rule) {
            writeln!(out, "{}", doc.explain_text())?;
        }
    }
    for c in &matches.candidates {
        write_candidate(&mut out, c)?;
        writeln!(out, "Hop resolution:")?;
        for (i, step) in c.chain.iter().enumerate() {
            let res = if step.resolution.is_empty() {
                "(unspecified)"
            } else {
                step.resolution.as_str()
            };
            writeln!(out, "  {}. {}:{} — {res}", i + 1, step.file, step.line)?;
        }
        writeln!(out)?;
        if let Some(doc) = rule_doc(&c.rule) {
            writeln!(out, "{}", doc.explain_text())?;
        }
    }
    for a in &matches.advice {
        write_advice(&mut out, a)?;
        writeln!(out, "Hop resolution:")?;
        for (i, step) in a.chain.iter().enumerate() {
            let res = if step.resolution.is_empty() {
                "(unspecified)"
            } else {
                step.resolution.as_str()
            };
            writeln!(out, "  {}. {}:{} — {res}", i + 1, step.file, step.line)?;
        }
        writeln!(out)?;
        if let Some(doc) = rule_doc(&a.rule) {
            writeln!(out, "{}", doc.explain_text())?;
        }
    }

    Ok(ExitCode::from(code as u8))
}

struct SiteMatches {
    findings: Vec<async_doctor_core::Finding>,
    candidates: Vec<async_doctor_core::Candidate>,
    advice: Vec<async_doctor_core::Advice>,
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
    let advice = report
        .advice
        .iter()
        .filter(|a| {
            touch(&a.entry.file, a.entry.line)
                || a.chain.iter().any(|s| touch(&s.file, s.line))
                || why_matches(&a.why, &file_norm, line)
        })
        .cloned()
        .collect();
    SiteMatches {
        findings,
        candidates,
        advice,
    }
}

fn why_matches(why: &str, file: &str, line: u32) -> bool {
    let Some(loc) = crate::extract_why_locator(why) else {
        return false;
    };
    parse_location(loc)
        .map(|(f, l)| l == line && paths_equal(&f, file))
        .unwrap_or(false)
}

fn parse_location(location: &str) -> anyhow::Result<(String, u32)> {
    let location = location.trim();
    // Allow accidental `--path …` suffix if the user pasted the full why line.
    let location = location
        .split_once(" --path ")
        .map(|(l, _)| l)
        .unwrap_or(location)
        .trim();
    let location = location
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            location
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        })
        .unwrap_or(location)
        .trim();
    let Some((file, line_s)) = location.rsplit_once(':') else {
        bail!("missing :line");
    };
    let line: u32 = line_s.parse().context("line must be an integer")?;
    if file.is_empty() || line == 0 {
        bail!("file and line must be non-empty / positive");
    }
    Ok((normalize_path(file), line))
}
