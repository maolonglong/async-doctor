//! GitHub-Flavored Markdown report output (`--format markdown`).
//!
//! Designed for `$GITHUB_STEP_SUMMARY` and pull-request comments without
//! requiring GitHub Advanced Security (SARIF).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::time::Duration;

use async_doctor_core::{EnvironmentKind, Finding, Report, summarize_sink_sites};

use crate::paths::display_fs_path;
use crate::text::{entry_headline, sink_label};

pub struct MarkdownOptions {
    pub suppressed_by_baseline: u32,
    pub suppressed_by_ignore: u32,
    pub elapsed: Option<Duration>,
}

pub fn write_markdown(report: &Report, opts: MarkdownOptions) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::with_capacity(64 * 1024, stdout.lock());
    write_markdown_to(&mut out, report, &opts)?;
    out.flush()?;
    Ok(())
}

fn write_markdown_to(
    out: &mut impl Write,
    report: &Report,
    opts: &MarkdownOptions,
) -> anyhow::Result<()> {
    writeln!(out, "# async-doctor\n")?;
    write_env_line(out, report)?;

    if let Some(msg) = &report.message {
        writeln!(out, "{msg}")?;
        return Ok(());
    }

    writeln!(
        out,
        "| Metric | Value |\n| --- | --- |\n| **Score** | **{} / 100** ({}) |\n| **Findings** | {} |\n| **Advice** | {} |\n",
        report.score,
        report.score_label(),
        report.findings.len(),
        report.advice.len(),
    )?;

    if report.findings.is_empty() {
        write!(out, "**No findings.**")?;
        let mut parts = Vec::new();
        if opts.suppressed_by_baseline > 0 {
            parts.push(format!(
                "{} suppressed by baseline",
                opts.suppressed_by_baseline
            ));
        }
        if opts.suppressed_by_ignore > 0 {
            parts.push(format!(
                "{} suppressed by inline ignore",
                opts.suppressed_by_ignore
            ));
        }
        if !parts.is_empty() {
            write!(out, " ({})", parts.join(", "))?;
        }
        writeln!(out, "\n")?;
    } else {
        write_findings_section(out, &report.findings)?;
    }

    if !report.advice.is_empty() {
        writeln!(
            out,
            "## Advice ({}) — does not fail the run\n",
            report.advice.len()
        )?;
        for a in &report.advice {
            writeln!(
                out,
                "- **`[advice:{}]`** `{}` (`{}:{}`) — {}",
                a.rule, a.entry.name, a.entry.file, a.entry.line, a.fix.hint
            )?;
        }
        writeln!(out)?;
    }

    if !report.unused_suppressions.is_empty() {
        writeln!(
            out,
            "## Unused suppressions ({})\n",
            report.unused_suppressions.len()
        )?;
        for u in &report.unused_suppressions {
            writeln!(
                out,
                "- `{}:{}` `ignore[{}]` — {}",
                u.file, u.line, u.rule, u.reason
            )?;
        }
        writeln!(out)?;
    }

    if !report.environment.note.is_empty() {
        writeln!(out, "> **Note:** {}\n", report.environment.note)?;
    }

    if report.omitted_findings + report.omitted_candidates + report.omitted_advice > 0 {
        writeln!(
            out,
            "_omitted {} finding(s), {} candidate(s), {} advice (--max-findings)_",
            report.omitted_findings, report.omitted_candidates, report.omitted_advice
        )?;
    }
    let files = report.file_count;
    let entries = report.async_entry_count;
    let file_s = if files == 1 { "" } else { "s" };
    let entry_s = if entries == 1 { "y" } else { "ies" };
    match opts.elapsed {
        Some(d) => writeln!(
            out,
            "_Scanned {files} file{file_s} · {entries} async entr{entry_s} in {}ms_",
            d.as_millis()
        )?,
        None => writeln!(
            out,
            "_Scanned {files} file{file_s} · {entries} async entr{entry_s}_"
        )?,
    }
    Ok(())
}

fn write_env_line(out: &mut impl Write, report: &Report) -> anyhow::Result<()> {
    let env = &report.environment;
    match env.kind {
        EnvironmentKind::None => {
            writeln!(out, "**Environment:** none (no interpreter/venv found)\n")?;
        }
        EnvironmentKind::Venv | EnvironmentKind::System => {
            let mut parts = vec![format!("{} ({})", env.kind.as_str(), env.origin)];
            if let Some(prefix) = &env.prefix {
                parts.push(format!("prefix `{}`", display_fs_path(prefix)));
            }
            if !env.frameworks.is_empty() {
                parts.push(format!("frameworks {}", env.frameworks.join(", ")));
            }
            writeln!(out, "**Environment:** {}\n", parts.join(" · "))?;
        }
    }
    Ok(())
}

fn write_findings_section(out: &mut impl Write, findings: &[Finding]) -> anyhow::Result<()> {
    let sites = summarize_sink_sites(findings);
    let mut groups: BTreeMap<&str, Vec<&Finding>> = BTreeMap::new();
    let mut entries: BTreeSet<(&str, &str)> = BTreeSet::new();
    for f in findings {
        groups
            .entry(f.fix.fix_group_id.as_str())
            .or_default()
            .push(f);
        entries.insert((f.entry.file.as_str(), f.entry.name.as_str()));
    }

    let entry_s = if entries.len() == 1 { "y" } else { "ies" };
    let site_s = if sites.len() == 1 { "" } else { "s" };
    let finding_s = if findings.len() == 1 { "" } else { "s" };
    let group_s = if groups.len() == 1 { "" } else { "s" };
    writeln!(
        out,
        "Found **{}** sink site{} · **{}** finding{} · **{}** entr{} · **{}** fix group{}\n",
        sites.len(),
        site_s,
        findings.len(),
        finding_s,
        entries.len(),
        entry_s,
        groups.len(),
        group_s
    )?;

    writeln!(
        out,
        "| Rule | Sink site | Call | Findings | Entries |\n| --- | --- | --- | ---: | ---: |"
    )?;
    for s in sites.iter().take(15) {
        let call = s
            .text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace('|', "\\|");
        writeln!(
            out,
            "| `{}` | `{}:{}` | `{}` | {} | {} |",
            s.rule, s.file, s.line, call, s.finding_count, s.entry_count
        )?;
    }
    writeln!(out)?;

    let mut ordered: Vec<_> = groups.into_iter().collect();
    ordered.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(b.0)));

    for (_, group) in ordered {
        let rep = group[0];
        let sink = sink_label(rep).unwrap_or("sink");
        writeln!(
            out,
            "<details>\n<summary><code>[{}]</code> <b>{}</b> — {} finding(s)</summary>\n",
            rep.rule,
            sink,
            group.len()
        )?;
        writeln!(out, "- **Fix:** {}", rep.fix.hint)?;
        if !rep.docs_url.is_empty() {
            writeln!(out, "- **Docs:** {}", rep.docs_url)?;
        }
        writeln!(out, "- **Why:** `{}`\n", rep.why)?;
        writeln!(out, "```text")?;
        writeln!(out, "{}", entry_headline(rep))?;
        for (i, step) in rep.chain.iter().enumerate() {
            let pipe = if i + 1 == rep.chain.len() {
                "└─"
            } else {
                "├─"
            };
            writeln!(out, "  {pipe} {}:{}  {}", step.file, step.line, step.text)?;
        }
        writeln!(out, "```\n</details>\n")?;
    }
    Ok(())
}
