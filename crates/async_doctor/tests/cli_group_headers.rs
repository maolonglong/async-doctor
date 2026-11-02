//! Grouped-text headers show canonical sink qualnames, one group per
//! (rule, fix) pair — never two identical titles.

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

#[test]
fn group_headers_use_canonical_sink_qualname_and_never_repeat() {
    let dir = fixture("m4_group_header_db");
    let out = bin().arg(&dir).output().expect("run async-doctor");
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    let headers: Vec<&str> = text.lines().filter(|l| l.starts_with('[')).collect();
    // Canonical qualnames in headers — not call-text fragments.
    assert!(
        headers
            .iter()
            .any(|h| h.contains("sqlite3.Connection.execute")),
        "header must show canonical sink qualname: {headers:?}"
    );
    // Same rule + canonical sink → one group: no identical titles.
    let mut seen = std::collections::BTreeSet::new();
    for h in &headers {
        let title: String = h.split(" — ").next().unwrap_or(h).to_string();
        assert!(seen.insert(title.clone()), "duplicate group title: {title}");
    }
    // The two sqlite sites are distinct canonical sinks → two distinct titles.
    let db: Vec<&&str> = headers
        .iter()
        .filter(|h| h.contains("blocking-db"))
        .collect();
    assert_eq!(
        db.len(),
        2,
        "expected 2 distinct blocking-db groups: {db:?}"
    );
}
