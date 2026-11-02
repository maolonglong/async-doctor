//! --max-findings caps the rendered lists only; score, exit code and
//! sinkSites are computed from the full report.

use std::path::PathBuf;
use std::process::Command;

fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_async-doctor"));
    cmd.env("NO_COLOR", "1");
    cmd
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("fixtures")
        .join(name)
        .canonicalize()
        .expect("fixture path")
}

fn json(dir: &PathBuf, args: &[&str]) -> serde_json::Value {
    let out = bin()
        .args(args)
        .arg("--format")
        .arg("json")
        .arg(dir)
        .output()
        .expect("run async-doctor");
    serde_json::from_slice(&out.stdout).expect("json")
}

#[test]
fn max_findings_caps_lists_only_and_covers_every_site() {
    let dir = fixture("m4_max_findings");
    let full = json(&dir, &[]);
    let capped = json(&dir, &["--max-findings", "2"]);

    // Full report: 2 sink sites, 3 findings.
    assert_eq!(full["sinkSites"].as_array().unwrap().len(), 2);
    assert_eq!(full["findings"].as_array().unwrap().len(), 3);

    // Score, exit-relevant data and sinkSites unchanged by the cap.
    assert_eq!(full["score"], capped["score"]);
    assert_eq!(full["sinkSites"], capped["sinkSites"]);
    assert_eq!(capped["findings"].as_array().unwrap().len(), 2);
    assert_eq!(capped["omittedFindings"], 1);
    assert_eq!(full["omittedFindings"], 0);

    // Every sink site keeps >=1 finding when N >= #sites (round-robin).
    let sites: Vec<(String, u64)> = capped["sinkSites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["file"].as_str().unwrap().to_string(),
                s["line"].as_u64().unwrap(),
            )
        })
        .collect();
    let covered: Vec<(String, u64)> = capped["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            let last = f["chain"].as_array().unwrap().last().unwrap();
            (
                last["file"].as_str().unwrap().to_string(),
                last["line"].as_u64().unwrap(),
            )
        })
        .collect();
    for site in &sites {
        assert!(covered.contains(site), "site {site:?} lost its finding");
    }

    // Exit code identical with and without the cap.
    let code_of = |args: &[&str]| {
        bin()
            .args(args)
            .arg(dir.to_str().unwrap())
            .status()
            .unwrap()
            .code()
    };
    assert_eq!(code_of(&[]), code_of(&["--max-findings", "2"]));
    assert_eq!(code_of(&["--max-findings", "1"]), code_of(&[]));
}

#[test]
fn text_footer_prints_omitted_counts() {
    let dir = fixture("m4_max_findings");
    let out = bin()
        .arg("--max-findings")
        .arg("2")
        .arg(&dir)
        .output()
        .expect("run");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("omitted 1 finding"),
        "text footer must print omitted count:\n{text}"
    );
}
