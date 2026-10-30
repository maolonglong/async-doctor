//! SARIF 2.1.0 emitter for GitHub code scanning and other SARIF consumers.

use async_doctor_core::Report;
use serde_json::{Value, json};

const SARIF_SCHEMA: &str = "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/master/sarif-2.1/schema/sarif-schema-2.1.0.json";

/// Build a SARIF 2.1.0 document from a report.
pub fn report_to_sarif(report: &Report) -> Value {
    let mut rules = Vec::new();
    let mut seen_rules = std::collections::BTreeSet::new();
    for f in &report.findings {
        if seen_rules.insert(f.rule.clone()) {
            rules.push(json!({
                "id": f.rule,
                "name": f.rule,
                "shortDescription": { "text": format!("Blocking IO: {}", f.rule) },
                "fullDescription": {
                    "text": f.fix.hint,
                },
                "helpUri": format!(
                    "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md{}",
                    f.docs_anchor
                ),
                "help": {
                    "text": format!(
                        "{}\n\nExplain: async-doctor explain {}\nDocs: docs/rules.md{}\n{}",
                        f.fix.hint, f.rule, f.docs_anchor, f.why
                    )
                },
                "defaultConfiguration": { "level": "error" },
            }));
        }
    }

    let mut results = Vec::new();
    for f in &report.findings {
        let (prim_file, prim_line) = f
            .chain
            .first()
            .map(|s| (s.file.as_str(), s.line))
            .unwrap_or((f.entry.file.as_str(), f.entry.line));

        let mut thread_locations = Vec::new();
        // Entry first, then each chain hop — full evidence for code scanning UI.
        thread_locations.push(json!({
            "location": physical_location(
                &f.entry.file,
                f.entry.line,
                &format!(
                    "async entry `{}` ({})",
                    f.entry.name,
                    f.entry.label.as_str()
                ),
            )
        }));
        for step in &f.chain {
            thread_locations.push(json!({
                "location": physical_location(&step.file, step.line, &step.text)
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
                    f.entry.file,
                    f.entry.line,
                    f.fix.hint
                )
            },
            "locations": [physical_location(prim_file, prim_line, &f.fix.hint)],
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

    json!({
        "$schema": SARIF_SCHEMA,
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "async-doctor",
                    "informationUri": "https://github.com/maolonglong/async-doctor",
                    "rules": rules,
                }
            },
            "originalUriBaseIds": {
                "%SRCROOT%": {
                    "uri": "./",
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

fn physical_location(file: &str, line: u32, message: &str) -> Value {
    json!({
        "physicalLocation": {
            "artifactLocation": {
                "uri": file.replace('\\', "/"),
                "uriBaseId": "%SRCROOT%",
            },
            "region": {
                "startLine": line.max(1),
            }
        },
        "message": { "text": message },
    })
}
