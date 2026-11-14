//! `explain` and `why` subcommands — agent-facing long-form help.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, bail};
use async_doctor_core::{
    Advice, AnalyzeOptions, Candidate, ChainStep, EXIT_FINDINGS, EXIT_OK, Finding, Report,
    SCHEMA_VERSION, primary_sink_step,
};
use async_doctor_rules::rule_doc;
use serde_json::json;

use crate::args::WhyFormat;
use crate::paths::{normalize_path, paths_equal, resolve_cli_path};
use crate::text::{write_advice, write_candidate, write_finding};

const WHY_GROUP_CAP_THRESHOLD: usize = 5;
const WHY_GROUP_SHOWN: usize = 3;

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
    python: Option<&Path>,
    format: WhyFormat,
    expand: bool,
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
    let resolved_python = python.map(resolve_cli_path);
    let scan_opts = async_doctor_ty::ScanOptions {
        analyze: options,
        python: resolved_python,
        cache_dir: None,
    };
    let mut report = async_doctor_ty::scan_path_with_scan_options(&abs_path, &scan_opts)
        .with_context(|| format!("failed to scan `{}`", abs_path.display()))?;
    // Same rewrite and inline-suppression pass the default scan applies.
    crate::rewrite_why_commands(&mut report, path, python);
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
    let mut report = crate::suppress::apply_scoped(report, &scan_root, single_file)?.report;
    report.refresh_sink_sites();

    let matches = findings_at(&report, &file, line);
    let code = if matches.findings.is_empty() {
        EXIT_OK
    } else {
        EXIT_FINDINGS
    };

    let mut rule_corrections = BTreeMap::new();
    for (rule, fix) in matches
        .findings
        .iter()
        .map(|f| (&f.rule, &f.fix))
        .chain(matches.candidates.iter().map(|c| (&c.rule, &c.fix)))
        .chain(matches.advice.iter().map(|a| (&a.rule, &a.fix)))
    {
        *rule_corrections.entry(rule.as_str()).or_insert(false) |= !fix.hint.is_empty();
    }

    if matches!(format, WhyFormat::Json) {
        let rules: Vec<_> = rule_corrections
            .iter()
            .map(|(id, has_correction)| {
                if let Some(doc) = rule_doc(id) {
                    json!({
                        "id": doc.id,
                        "title": doc.title,
                        "summary": doc.summary,
                        "body": doc.body,
                        "fix": if *has_correction { doc.fix } else { "" },
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
        crate::write_json_stdout(&doc)?;
        return Ok(ExitCode::from(code as u8));
    }

    let mut out = std::io::stdout().lock();
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

    let mut distinct_docs = Vec::new();
    let mut seen_rule_ids: HashSet<&'static str> = HashSet::new();
    let mut record_rule = |r: &str| {
        if let Some(doc) = rule_doc(r)
            && seen_rule_ids.insert(doc.id)
        {
            let mut doc = *doc;
            if !rule_corrections[doc.id] {
                doc.fix = "";
            }
            distinct_docs.push(doc);
        }
    };

    for group in group_by_sink_and_rule(&matches.findings, finding_sink_key) {
        record_rule(&group[0].rule);
        let (_, sink_file, sink_line) = finding_sink_key(group[0]);
        let shown = if !expand && group.len() > WHY_GROUP_CAP_THRESHOLD {
            WHY_GROUP_SHOWN
        } else {
            group.len()
        };
        for f in group.iter().take(shown) {
            write_finding(&mut out, f)?;
            write_hop_resolution(&mut out, &f.chain)?;
        }
        if group.len() > shown {
            writeln!(
                out,
                "... and {} more entries (pass --expand to list all)\n",
                group.len() - shown
            )?;
        }
        if let Some(site) = report.sink_sites.iter().find(|s| {
            s.fix_group_id == group[0].fix.fix_group_id
                && s.file == sink_file
                && s.line == sink_line
        }) && site.sync_caller_count > 0
        {
            let caller_s = if site.sync_caller_count == 1 { "" } else { "s" };
            let reach_verb = if site.sync_caller_count == 1 {
                "reaches"
            } else {
                "reach"
            };
            match site.sample_sync_caller.as_deref() {
                Some(sample) if !sample.is_empty() => writeln!(
                    out,
                    "Sync callers: {} observed sync caller{caller_s} also {reach_verb} this helper (e.g. {sample}).\n",
                    site.sync_caller_count
                )?,
                _ => writeln!(
                    out,
                    "Sync callers: {} observed sync caller{caller_s} also {reach_verb} this helper.\n",
                    site.sync_caller_count
                )?,
            }
        }
    }

    for group in group_by_sink_and_rule(&matches.candidates, candidate_sink_key) {
        record_rule(&group[0].rule);
        let shown = if !expand && group.len() > WHY_GROUP_CAP_THRESHOLD {
            WHY_GROUP_SHOWN
        } else {
            group.len()
        };
        for c in group.iter().take(shown) {
            write_candidate(&mut out, c)?;
            write_hop_resolution(&mut out, &c.chain)?;
        }
        if group.len() > shown {
            writeln!(
                out,
                "... and {} more entries (pass --expand to list all)\n",
                group.len() - shown
            )?;
        }
    }

    for group in group_by_sink_and_rule(&matches.advice, advice_sink_key) {
        record_rule(&group[0].rule);
        let shown = if !expand && group.len() > WHY_GROUP_CAP_THRESHOLD {
            WHY_GROUP_SHOWN
        } else {
            group.len()
        };
        for a in group.iter().take(shown) {
            write_advice(&mut out, a)?;
            write_hop_resolution(&mut out, &a.chain)?;
        }
        if group.len() > shown {
            writeln!(
                out,
                "... and {} more entries (pass --expand to list all)\n",
                group.len() - shown
            )?;
        }
    }

    for doc in distinct_docs {
        writeln!(out, "{}", doc.explain_text())?;
    }

    Ok(ExitCode::from(code as u8))
}

fn write_hop_resolution(out: &mut impl Write, chain: &[ChainStep]) -> anyhow::Result<()> {
    writeln!(out, "Hop resolution:")?;
    for (i, step) in chain.iter().enumerate() {
        let res = if step.resolution.is_empty() {
            "(unspecified)"
        } else {
            step.resolution.as_str()
        };
        writeln!(out, "  {}. {}:{} — {res}", i + 1, step.file, step.line)?;
    }
    writeln!(out)?;
    Ok(())
}

fn finding_sink_key(f: &Finding) -> (&str, &str, u32) {
    let (file, line) = primary_sink_step(f)
        .map(|(_, s)| (s.file.as_str(), s.line))
        .unwrap_or((f.entry.file.as_str(), f.entry.line));
    (f.rule.as_str(), file, line)
}

fn candidate_sink_key(c: &Candidate) -> (&str, &str, u32) {
    let (file, line) = c
        .chain
        .last()
        .map(|s| (s.file.as_str(), s.line))
        .unwrap_or((c.entry.file.as_str(), c.entry.line));
    (c.rule.as_str(), file, line)
}

fn advice_sink_key(a: &Advice) -> (&str, &str, u32) {
    let (file, line) = a
        .chain
        .last()
        .map(|s| (s.file.as_str(), s.line))
        .unwrap_or((a.entry.file.as_str(), a.entry.line));
    (a.rule.as_str(), file, line)
}

fn group_by_sink_and_rule<'a, T>(
    items: &'a [T],
    key_fn: fn(&'a T) -> (&'a str, &'a str, u32),
) -> Vec<Vec<&'a T>> {
    let mut keys: Vec<(&'a str, &'a str, u32)> = Vec::new();
    let mut groups: Vec<Vec<&'a T>> = Vec::new();
    for item in items {
        let key = key_fn(item);
        if let Some(pos) = keys.iter().position(|k| *k == key) {
            groups[pos].push(item);
        } else {
            keys.push(key);
            groups.push(vec![item]);
        }
    }
    groups
}

struct SiteMatches {
    findings: Vec<Finding>,
    candidates: Vec<Candidate>,
    advice: Vec<Advice>,
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
    // Allow accidental `--path …` or `--python …` suffix if the user pasted the full why line.
    let location = location
        .split_once(" --path ")
        .map(|(l, _)| l)
        .unwrap_or(location);
    let location = location
        .split_once(" --python ")
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
