//! SARIF 2.1.0 emitter for GitHub code scanning and other SARIF consumers.
//!
//! Artifact URIs are repo-root-relative (via git toplevel when available) so
//! scanning a subdirectory still maps correctly in GitHub code scanning.
//! `%SRCROOT%` is an absolute `file:` URI (SARIF1004).
//! Third-party hops use the real absolute `file://` path of the resolved
//! site-packages file (never `file://<scan-root>/site-packages/...` fakes).

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
    let site_roots = discover_site_packages_roots(&scan_root);

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
        let entry_loc = in_repo_location(&f.entry.file, false, &scan_root, &src_root, &site_roots);
        let (prim_file, prim_line, prim_third) = f
            .chain
            .first()
            .map(|s| (s.file.as_str(), s.line, s.third_party))
            .unwrap_or((f.entry.file.as_str(), f.entry.line, false));
        let Some(prim_loc) =
            in_repo_location(prim_file, prim_third, &scan_root, &src_root, &site_roots)
        else {
            // Should not happen for first-party primary locations.
            continue;
        };

        let mut thread_locations = Vec::new();
        if let Some(entry_loc) = entry_loc {
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
        }
        for step in &f.chain {
            if let Some(loc) = step_location(step, &scan_root, &src_root, &site_roots) {
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

fn step_location(
    step: &ChainStep,
    scan_root: &Path,
    src_root: &Path,
    site_roots: &[PathBuf],
) -> Option<ArtifactUri> {
    in_repo_location(
        &step.file,
        step.third_party,
        scan_root,
        src_root,
        site_roots,
    )
}

fn in_repo_location(
    path: &str,
    third_party: bool,
    scan_root: &Path,
    src_root: &Path,
    site_roots: &[PathBuf],
) -> Option<ArtifactUri> {
    if third_party || is_synthetic_site_packages(path) {
        return resolve_third_party_abs(path, scan_root, site_roots)
            .map(|abs| ArtifactUri::AbsoluteFile(file_uri_file(&abs)));
    }

    let abs = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        scan_root.join(path)
    };
    let abs = abs.canonicalize().unwrap_or(abs);

    match abs.strip_prefix(src_root) {
        Ok(rel) => {
            let rel = normalize_path(&rel.to_string_lossy());
            if rel.contains("site-packages/") || rel.contains("dist-packages/") {
                Some(ArtifactUri::AbsoluteFile(file_uri_file(&abs)))
            } else {
                Some(ArtifactUri::RepoRelative(rel))
            }
        }
        Err(_) => Some(ArtifactUri::AbsoluteFile(file_uri_file(&abs))),
    }
}

fn is_synthetic_site_packages(path: &str) -> bool {
    let p = path.replace('\\', "/");
    p.starts_with("site-packages/") || p.starts_with("dist-packages/")
}

/// Map a synthetic `site-packages/pkg/mod.py` report path to a real file.
fn resolve_third_party_abs(
    path: &str,
    scan_root: &Path,
    site_roots: &[PathBuf],
) -> Option<PathBuf> {
    let norm = path.replace('\\', "/");
    if Path::new(&norm).is_absolute() {
        let p = PathBuf::from(&norm);
        return p.is_file().then_some(p.canonicalize().unwrap_or(p));
    }

    // Fixture layout: <scan_root>/site-packages/...
    let direct = scan_root.join(&norm);
    if direct.is_file() {
        return Some(direct.canonicalize().unwrap_or(direct));
    }

    let rest = norm
        .strip_prefix("site-packages/")
        .or_else(|| norm.strip_prefix("dist-packages/"))?;
    for root in site_roots {
        let candidate = root.join(rest);
        if candidate.is_file() {
            return Some(candidate.canonicalize().unwrap_or(candidate));
        }
    }
    None
}

/// Discover `site-packages` / `dist-packages` directories near the scan root.
fn discover_site_packages_roots(scan_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |p: PathBuf| {
        if p.is_dir() {
            let key = p.canonicalize().unwrap_or_else(|_| p.clone());
            if seen.insert(key.clone()) {
                out.push(key);
            }
        }
    };

    push(scan_root.join("site-packages"));
    push(scan_root.join("dist-packages"));

    if let Ok(venv) = std::env::var("VIRTUAL_ENV") {
        let venv = PathBuf::from(venv);
        if let Ok(lib) = std::fs::read_dir(venv.join("lib")) {
            for ent in lib.flatten() {
                let name = ent.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("python") {
                    push(ent.path().join("site-packages"));
                    push(ent.path().join("dist-packages"));
                }
            }
        }
    }

    // Walk scan_root (and a few parents) for .venv/venv layouts.
    let mut cursor = Some(scan_root.to_path_buf());
    for _ in 0..4 {
        let Some(dir) = cursor else { break };
        for venv_name in [".venv", "venv"] {
            let lib = dir.join(venv_name).join("lib");
            if let Ok(rd) = std::fs::read_dir(&lib) {
                for ent in rd.flatten() {
                    let name = ent.file_name();
                    let name = name.to_string_lossy();
                    if name.starts_with("python") {
                        push(ent.path().join("site-packages"));
                        push(ent.path().join("dist-packages"));
                    }
                }
            }
        }
        // Also a project-local site-packages (fixtures).
        push(dir.join("site-packages"));
        push(dir.join("dist-packages"));
        cursor = dir.parent().map(Path::to_path_buf);
    }

    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_fixture_site_packages_under_scan_root() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/m2_cache_multipkg")
            .canonicalize()
            .unwrap();
        let roots = discover_site_packages_roots(&fixture);
        let abs = resolve_third_party_abs("site-packages/libx/core.py", &fixture, &roots).unwrap();
        assert!(abs.is_file(), "{abs:?}");
        assert!(abs.ends_with("libx/core.py"), "{abs:?}");
        let uri = file_uri_file(&abs);
        assert!(uri.starts_with("file://"), "{uri}");
        // Must not invent a non-existent path under an unrelated prefix.
        assert!(
            !uri.ends_with("/site-packages/site-packages/libx/core.py"),
            "{uri}"
        );
    }
}
