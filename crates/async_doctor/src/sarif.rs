//! SARIF 2.1.0 emitter for GitHub code scanning and other SARIF consumers.
//!
//! Artifact URIs are repo-root-relative (via git toplevel when available) so
//! scanning a subdirectory still maps correctly in GitHub code scanning.
//! `%SRCROOT%` is an absolute `file:` URI (SARIF1004).
//! Third-party / outside-repo hops use absolute `file://` URIs with no
//! `uriBaseId` (never `site-packages/...` under `%SRCROOT%`).

use std::path::{Path, PathBuf};

use async_doctor_core::{ChainStep, Report};
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
        let entry_loc = in_repo_location(&f.entry.file, false, &scan_root, &src_root);
        let (prim_file, prim_line, prim_third) = f
            .chain
            .first()
            .map(|s| (s.file.as_str(), s.line, s.third_party))
            .unwrap_or((f.entry.file.as_str(), f.entry.line, false));
        let prim_loc = in_repo_location(prim_file, prim_third, &scan_root, &src_root);

        let mut thread_locations = Vec::new();
        thread_locations.push(json!({
            "location": physical_location(
                &entry_loc,
                f.entry.line,
                &format!(
                    "async entry `{}` ({})",
                    f.entry.name,
                    f.entry.label.as_str()
                ),
            )
        }));
        for step in &f.chain {
            if let Some(loc) = step_location(step, &scan_root, &src_root) {
                thread_locations.push(json!({
                    "location": physical_location(&loc, step.line, &step.text)
                }));
            }
        }

        results.push(json!({
            "ruleId": f.rule,
            "level": "error",
            "message": {
                "text": format!(
                    "[{}] reachable from {} — {}",
                    f.rule,
                    f.entry.name,
                    f.fix.hint
                )
            },
            "locations": [physical_location(&prim_loc, prim_line, &f.fix.hint)],
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

enum ArtifactUri {
    /// Relative to `%SRCROOT%`.
    RepoRelative(String),
    /// Absolute `file://` URI (third-party / outside the repo).
    AbsoluteFile(String),
}

fn step_location(step: &ChainStep, scan_root: &Path, src_root: &Path) -> Option<ArtifactUri> {
    Some(in_repo_location(
        &step.file,
        step.third_party,
        scan_root,
        src_root,
    ))
}

fn in_repo_location(
    path: &str,
    third_party: bool,
    scan_root: &Path,
    src_root: &Path,
) -> ArtifactUri {
    let abs = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        scan_root.join(path)
    };
    let abs = abs.canonicalize().unwrap_or(abs);

    if third_party {
        return ArtifactUri::AbsoluteFile(file_uri_file(&abs));
    }
    match abs.strip_prefix(src_root) {
        Ok(rel) => {
            let rel = normalize_path(&rel.to_string_lossy());
            // site-packages under a venv inside the repo still must not use SRCROOT
            // as if it were project source — prefer absolute file://.
            if rel.contains("site-packages/") || rel.contains("dist-packages/") {
                ArtifactUri::AbsoluteFile(file_uri_file(&abs))
            } else {
                ArtifactUri::RepoRelative(rel)
            }
        }
        Err(_) => ArtifactUri::AbsoluteFile(file_uri_file(&abs)),
    }
}

fn file_uri_dir(path: &Path) -> String {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut s = abs.to_string_lossy().replace('\\', "/");
    if !s.starts_with('/') {
        s = format!("/{s}");
    }
    if !s.ends_with('/') {
        s.push('/');
    }
    format!("file://{s}")
}

fn file_uri_file(path: &Path) -> String {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut s = abs.to_string_lossy().replace('\\', "/");
    if !s.starts_with('/') {
        s = format!("/{s}");
    }
    format!("file://{s}")
}

fn physical_location(uri: &ArtifactUri, line: u32, message: &str) -> Value {
    let artifact = match uri {
        ArtifactUri::RepoRelative(rel) => json!({
            "uri": rel,
            "uriBaseId": "%SRCROOT%",
        }),
        ArtifactUri::AbsoluteFile(file) => json!({
            "uri": file,
        }),
    };
    json!({
        "physicalLocation": {
            "artifactLocation": artifact,
            "region": {
                "startLine": line.max(1),
            }
        },
        "message": { "text": message },
    })
}
