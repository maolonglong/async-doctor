//! Definition-time conditions must not become execution-time callee conditions.

use super::helpers::{advice_shared_venv, copy_dir, temp_project};
use crate::scan_path;

#[test]
fn named_offload_bodies_have_their_own_execution_conditions() {
    let dir = temp_project(&[(
        "app.py",
        include_str!("../../../../fixtures/advice_nested_scope/app.py"),
    )]);
    copy_dir(&advice_shared_venv(), &dir.join(".venv"));
    let report = scan_path(&dir).unwrap();
    let entries: Vec<_> = report
        .advice
        .iter()
        .filter(|a| a.rule == "offload-has-async-twin")
        .map(|a| a.entry.name.as_str())
        .collect();
    for expected in ["after_guard", "inside_try", "inside_branch", "inside_loop"] {
        assert!(
            entries.contains(&expected),
            "missing {expected}: {entries:?}"
        );
    }
    for banned in ["internal_branch", "mixed_sinks", "shared_with_sync"] {
        assert!(
            !entries.contains(&banned),
            "unexpected {banned}: {entries:?}"
        );
    }
    assert!(
        report.findings.is_empty(),
        "offloaded and dead bodies must not block the loop: {:?}",
        report.findings
    );
}
