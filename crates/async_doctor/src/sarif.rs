//! SARIF 2.1.0 emitter for GitHub code scanning and other SARIF consumers.
//!
//! Artifact URIs are repo-root-relative (via git toplevel when available) so
//! scanning a subdirectory still maps correctly in GitHub code scanning.
//! `%SRCROOT%` is an absolute `file:` URI (SARIF1004).

use std::path::{Path, PathBuf};

use async_doctor_core::Report;
use async_doctor_rules::rule_doc;
use serde_json::{Value, json};

use crate::paths::normalize_path;

const SARIF_SCHEMA: &str = "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/master/sarif-2.1/schema/sarif-schema-2.1.0.json";

/// Build a SARIF 2.1.0 document from a report.
///
/// `scan_root` is the directory finding paths are relative to.
/// `src_root` is the repository root used for `%SRCROOT%` and URI rewriting.
pub fn report_to_sarif(report: &Report, scan_root: &Path, src_root: &Path) -> Value {
    let scan_root = scan_root
        .canonicalize()
        .unwrap_or_else(|_| scan_root.to_path_buf());
    let src_root = src_root
        .canonicalize()
        .unwrap_or_else(|_| src_root.to_path_buf());

    let mut rules = Vec::new();
    let mut seen_rules = std::collections::BTreeSet::new();
    for f in &report.findings {
        if seen_rules.insert(f.rule.clone()) {
            let (name, short) = rule_display(&f.rule);
            let help_uri = if f.docs_url.is_empty() {
                format!(
                    "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md{}",
                    f.docs_anchor
                )
            } else {
                f.docs_url.clone()
            };
            rules.push(json!({
                "id": f.rule,
                "name": name,
                "shortDescription": { "text": short },
                "fullDescription": {
                    "text": f.fix.hint,
                },
                "helpUri": help_uri,
                "help": {
                    "text": format!(
                        "{}\n\nExplain: async-doctor explain {}\n{}",
                        f.fix.hint, f.rule, f.why
                    )
                },
                "defaultConfiguration": { "level": "error" },
            }));
        }
    }

    let mut results = Vec::new();
    for f in &report.findings {
        let entry_uri = repo_uri(&f.entry.file, &scan_root, &src_root);
        let (prim_file, prim_line) = f
            .chain
            .first()
            .map(|s| (s.file.as_str(), s.line))
            .unwrap_or((f.entry.file.as_str(), f.entry.line));
        let prim_uri = repo_uri(prim_file, &scan_root, &src_root);

        let mut thread_locations = Vec::new();
        thread_locations.push(json!({
            "location": physical_location(
                &entry_uri,
                f.entry.line,
                &format!(
                    "async entry `{}` ({})",
                    f.entry.name,
                    f.entry.label.as_str()
                ),
            )
        }));
        for step in &f.chain {
            let uri = repo_uri(&step.file, &scan_root, &src_root);
            thread_locations.push(json!({
                "location": physical_location(&uri, step.line, &step.text)
            }));
        }

        results.push(json!({
            "ruleId": f.rule,
            "level": "error",
            "message": {
                "text": format!(
                    "[{}] reachable from {} {}:{} — {}",
                    f.rule,
                    f.entry.name,
                    entry_uri,
                    f.entry.line,
                    f.fix.hint
                )
            },
            "locations": [physical_location(&prim_uri, prim_line, &f.fix.hint)],
            "partialFingerprints": {
                "asyncDoctor/findingId": f.id,
            },
            "codeFlows": [{
                "threadFlows": [{
                    "locations": thread_locations,
                }]
            }],
        }));
    }

    let src_uri = file_uri_dir(&src_root);

    json!({
        "$schema": SARIF_SCHEMA,
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "async-doctor",
                    "version": env!("CARGO_PKG_VERSION"),
                    "informationUri": "https://github.com/maolonglong/async-doctor",
                    "rules": rules,
                }
            },
            "originalUriBaseIds": {
                "%SRCROOT%": {
                    "uri": src_uri,
                }
            },
            "results": results,
            "invocations": [{
                "executionSuccessful": report.complete,
                "exitCode": report.exit_code(),
            }],
        }]
    })
}

fn rule_display(rule: &str) -> (String, String) {
    if let Some(doc) = rule_doc(rule) {
        (doc.title.to_string(), doc.summary.to_string())
    } else {
        (
            format!("Blocking IO ({rule})"),
            format!("Blocking IO: {rule}"),
        )
    }
}

fn repo_uri(path: &str, scan_root: &Path, src_root: &Path) -> String {
    let abs = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        scan_root.join(path)
    };
    let abs = abs.canonicalize().unwrap_or(abs);
    match abs.strip_prefix(src_root) {
        Ok(rel) => normalize_path(&rel.to_string_lossy()),
        Err(_) => normalize_path(path),
    }
}

fn file_uri_dir(path: &Path) -> String {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut s = abs.to_string_lossy().replace('\\', "/");
    if !s.starts_with('/') {
        // Windows drive paths → /C:/...
        s = format!("/{s}");
    }
    if !s.ends_with('/') {
        s.push('/');
    }
    format!("file://{s}")
}

fn physical_location(uri: &str, line: u32, message: &str) -> Value {
    json!({
        "physicalLocation": {
            "artifactLocation": {
                "uri": uri,
                "uriBaseId": "%SRCROOT%",
            },
            "region": {
                "startLine": line.max(1),
            }
        },
        "message": { "text": message },
    })
}
