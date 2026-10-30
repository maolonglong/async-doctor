//! SARIF 2.1.0 emission + schema validation.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use jsonschema::Draft;
use serde_json::Value;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_async-doctor"))
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("fixtures")
        .join(name)
        .canonicalize()
        .expect("fixture")
}

fn schema_validator() -> &'static jsonschema::Validator {
    static V: OnceLock<jsonschema::Validator> = OnceLock::new();
    V.get_or_init(|| {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/sarif-schema-2.1.0.json");
        let schema: Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("schema file"))
                .expect("schema json");
        jsonschema::options()
            .with_draft(Draft::Draft4)
            .build(&schema)
            .expect("compile SARIF schema")
    })
}

#[test]
fn sarif_output_validates_against_2_1_0_schema() {
    let dir = fixture("m1_multihop");
    let out = bin()
        .args(["--format", "sarif"])
        .arg(&dir)
        .output()
        .expect("run");
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: Value = serde_json::from_slice(&out.stdout).expect("sarif json");
    assert_eq!(doc["version"], "2.1.0");
    let driver = &doc["runs"][0]["tool"]["driver"];
    assert_eq!(driver["name"], "async-doctor");
    assert!(
        !driver["version"].as_str().unwrap_or("").is_empty(),
        "driver.version must be set"
    );
    let rules = driver["rules"].as_array().expect("rules");
    let sleep_rule = rules
        .iter()
        .find(|r| r["id"] == "blocking-sleep")
        .expect("blocking-sleep rule");
    let rule_name = sleep_rule["name"].as_str().unwrap();
    assert_ne!(rule_name, "blocking-sleep", "name must differ from id");
    assert!(!rule_name.is_empty());

    let src = doc["runs"][0]["originalUriBaseIds"]["%SRCROOT%"]["uri"]
        .as_str()
        .expect("%SRCROOT% uri");
    assert!(
        src.starts_with("file://") && src.ends_with('/'),
        "%SRCROOT% must be absolute file URI with trailing slash, got {src}"
    );
    assert_ne!(src, "./", "SARIF1004: %SRCROOT% must not be ./");

    let results = doc["runs"][0]["results"].as_array().expect("results");
    assert!(!results.is_empty());
    assert_eq!(results[0]["ruleId"], "blocking-sleep");
    assert!(results[0]["locations"][0]["physicalLocation"]["region"]["startLine"].is_number());
    let uri = results[0]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
        .as_str()
        .unwrap();
    assert_eq!(
        results[0]["locations"][0]["physicalLocation"]["artifactLocation"]["uriBaseId"],
        "%SRCROOT%"
    );
    assert!(
        !uri.starts_with('/') && !uri.starts_with("file:"),
        "artifact uri should be relative to %SRCROOT%, got {uri}"
    );
    assert!(
        results[0]["codeFlows"][0]["threadFlows"][0]["locations"]
            .as_array()
            .unwrap()
            .len()
            >= 2
    );
    assert!(
        results[0]["partialFingerprints"]["asyncDoctor/findingId"]
            .as_str()
            .unwrap()
            .len()
            == 32
    );

    let validator = schema_validator();
    if let Err(err) = validator.validate(&doc) {
        let errors: Vec<String> = validator
            .iter_errors(&doc)
            .map(|e| format!("{}: {e}", e.instance_path))
            .take(20)
            .collect();
        panic!(
            "SARIF failed schema validation ({err}):\n{}",
            errors.join("\n")
        );
    }
}

#[test]
fn sarif_uris_are_repo_root_relative_when_scanning_subdir() {
    // Scan a subdirectory of this git repo; URIs must include the path from
    // the repo toplevel (not just the basename relative to the scan root).
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root");
    let scan = repo.join("fixtures/m1_multihop");
    let out = bin()
        .args(["--format", "sarif"])
        .arg(&scan)
        .output()
        .expect("run");
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: Value = serde_json::from_slice(&out.stdout).expect("sarif json");
    let uri = doc["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["artifactLocation"]
        ["uri"]
        .as_str()
        .unwrap();
    assert!(
        uri.starts_with("fixtures/"),
        "expected repo-relative uri under fixtures/, got {uri}"
    );
    let src = doc["runs"][0]["originalUriBaseIds"]["%SRCROOT%"]["uri"]
        .as_str()
        .unwrap();
    let repo_uri = format!("file://{}/", repo.to_string_lossy().replace('\\', "/"));
    assert_eq!(src, repo_uri, "%SRCROOT% should point at git toplevel");
}

#[test]
fn sarif_clean_project_has_empty_results() {
    let file = fixture("negative_sync.py");
    let out = bin()
        .args(["--format", "sarif"])
        .arg(&file)
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0));
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(doc["runs"][0]["results"].as_array().unwrap().is_empty());
    schema_validator()
        .validate(&doc)
        .expect("empty SARIF must validate");
}
