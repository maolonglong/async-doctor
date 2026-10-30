//! constants integration tests.

use std::fs;

use super::helpers::temp_project;
use crate::scan_path;

#[test]
fn generator_bare_call_does_not_propagate() {
    let dir = temp_project(&[(
        "gen_bare.py",
        "import time\n\ndef gen():\n    time.sleep(1)\n    yield 1\n\nasync def handler():\n    g = gen()\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report.findings.is_empty(),
        "bare generator call must not propagate: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn generator_for_iter_propagates() {
    let dir = temp_project(&[(
        "gen_for.py",
        "import time\n\ndef gen():\n    time.sleep(1)\n    yield 1\n\nasync def handler():\n    for _ in gen():\n        pass\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(
        report.findings.len(),
        1,
        "iterating a generator must propagate: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn generator_list_consumes() {
    let dir = temp_project(&[(
        "gen_list.py",
        "import time\n\ndef gen():\n    time.sleep(1)\n    yield 1\n\nasync def handler():\n    list(gen())\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn module_const_dead_branch_pruned() {
    let dir = temp_project(&[(
        "const_dead.py",
        "import time\n\nDEBUG = False\n_EMPTY = ()\n\nasync def handler():\n    if DEBUG:\n        time.sleep(1)\n    if _EMPTY:\n        time.sleep(2)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report.findings.is_empty(),
        "module-const falsy branches must prune: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn module_const_not_empty_is_live() {
    let dir = temp_project(&[(
        "const_live.py",
        "import time\n\n_EMPTY = ()\n\nasync def handler():\n    if not _EMPTY:\n        time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(
        report.findings.len(),
        1,
        "`if not _EMPTY` is live when _EMPTY=(): {:?}",
        report.findings
    );
    assert!(
        report.findings[0]
            .conditions
            .iter()
            .any(|c| c == "async-branch"),
        "{:?}",
        report.findings[0].conditions
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn module_const_debug_false_else_is_branch() {
    let dir = temp_project(&[(
        "const_else.py",
        "import time\n\nDEBUG = False\n\nasync def handler():\n    if DEBUG:\n        pass\n    else:\n        time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert!(
        report.findings[0]
            .conditions
            .iter()
            .any(|c| c == "async-branch"),
        "{:?}",
        report.findings[0].conditions
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn local_reassign_does_not_prune_module_const() {
    let dir = temp_project(&[(
        "local_dbg.py",
        "import time\n\nDEBUG = False\n\nasync def handler():\n    DEBUG = True\n    if DEBUG:\n        time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(
        report.findings.len(),
        1,
        "local reassignment must not prune: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn param_shadow_does_not_prune_module_const() {
    let dir = temp_project(&[(
        "param_dbg.py",
        "import time\n\nDEBUG = False\n\nasync def handler(DEBUG):\n    if DEBUG:\n        time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(
        report.findings.len(),
        1,
        "param shadow must not prune: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn global_stmt_disables_module_const_prune() {
    let dir = temp_project(&[(
        "global_dbg.py",
        "import time\n\nDEBUG = False\n\ndef configure():\n    global DEBUG\n    DEBUG = True\n\nasync def handler():\n    if DEBUG:\n        time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(
        report.findings.len(),
        1,
        "global DEBUG must not prune: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}
