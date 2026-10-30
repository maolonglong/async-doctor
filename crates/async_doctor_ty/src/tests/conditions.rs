//! conditions integration tests.

use std::fs;

use super::helpers::temp_project;
use crate::scan_path;

#[test]
fn loop_condition_tag() {
    let dir = temp_project(&[(
        "loop.py",
        "import time\n\nasync def handler():\n    for _ in range(1):\n        time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert!(
        report.findings[0].conditions.iter().any(|c| c == "loop"),
        "{:?}",
        report.findings[0].conditions
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn branch_not_unconditional_or_all_paths() {
    let dir = temp_project(&[(
        "branch.py",
        "import time\n\nasync def handler(flag):\n    if flag:\n        time.sleep(1)\n    else:\n        time.sleep(2)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    // Entry-local param guards stay findings (external input), not demoted.
    assert_eq!(report.findings.len(), 2, "{:?}", report.findings);
    assert!(report.candidates.is_empty(), "{:?}", report.candidates);
    for f in &report.findings {
        assert!(
            !f.conditions
                .iter()
                .any(|c| c == "unconditional" || c == "all-paths"),
            "branched sink must not be unconditional/all-paths: {:?}",
            f.conditions
        );
        assert!(
            f.conditions.iter().any(|c| c == "async-branch"),
            "expected async-branch: {:?}",
            f.conditions
        );
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn entry_param_guard_stays_finding() {
    let dir = temp_project(&[(
        "entry_flag.py",
        "from pathlib import Path\n\nasync def handler(flag):\n    if flag:\n        Path('x').read_text()\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert!(report.candidates.is_empty(), "{:?}", report.candidates);
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
fn param_guard_unknown_is_conditional_candidate() {
    let dir = temp_project(&[(
        "param.py",
        "import time\n\ndef maybe(flag):\n    if flag:\n        time.sleep(1)\n\nasync def handler(debug):\n    maybe(debug)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report.findings.is_empty(),
        "param-guarded sink must not be a finding: {:?}",
        report.findings
    );
    assert!(
        report.candidates.iter().any(|c| c.reason == "conditional"),
        "expected conditional candidate: {:?}",
        report.candidates
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn param_guard_false_literal_omitted() {
    let dir = temp_project(&[(
        "param_false.py",
        "import time\n\ndef maybe(flag):\n    if flag:\n        time.sleep(1)\n\nasync def handler():\n    maybe(False)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    assert!(report.candidates.is_empty(), "{:?}", report.candidates);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn cache_miss_dependent_tag() {
    let dir = temp_project(&[(
        "cache.py",
        "import time\n\n_cache = None\n\nasync def handler():\n    global _cache\n    if _cache is None:\n        time.sleep(1)\n        _cache = 1\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert!(
        report.findings[0]
            .conditions
            .iter()
            .any(|c| c == "cache_miss_dependent"),
        "{:?}",
        report.findings[0].conditions
    );
    assert!(
        !report.findings[0]
            .conditions
            .iter()
            .any(|c| c == "unconditional" || c == "all-paths"),
        "{:?}",
        report.findings[0].conditions
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn early_return_before_sink_is_branch() {
    let dir = temp_project(&[(
        "early_ret.py",
        "import time\n\nasync def handler(x):\n    if x:\n        return\n    time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert!(
        report.findings[0]
            .conditions
            .iter()
            .any(|c| c == "async-branch"),
        "after early-return must be async-branch: {:?}",
        report.findings[0].conditions
    );
    assert!(
        !report.findings[0]
            .conditions
            .iter()
            .any(|c| c == "unconditional" || c == "all-paths"),
        "{:?}",
        report.findings[0].conditions
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn early_raise_before_sink_is_branch() {
    let dir = temp_project(&[(
        "early_raise.py",
        "import time\n\nasync def handler(ok):\n    if not ok:\n        raise ValueError('no')\n    time.sleep(1)\n",
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
fn cache_hit_early_return_tags_miss() {
    let dir = temp_project(&[(
        "cache_hit.py",
        "import time\n\n_cache = None\n\nasync def handler():\n    global _cache\n    if _cache is not None:\n        return _cache\n    time.sleep(1)\n    _cache = 1\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert!(
        report.findings[0]
            .conditions
            .iter()
            .any(|c| c == "cache_miss_dependent"),
        "{:?}",
        report.findings[0].conditions
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn key_in_cache_early_return_tags_miss() {
    let dir = temp_project(&[(
        "cache_in.py",
        "import time\n\n_cache = {}\n\nasync def handler(key):\n    if key in _cache:\n        return _cache[key]\n    time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert!(
        report.findings[0]
            .conditions
            .iter()
            .any(|c| c == "cache_miss_dependent"),
        "{:?}",
        report.findings[0].conditions
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn always_true_early_return_prunes_rest() {
    let dir = temp_project(&[(
        "empty_ret.py",
        "import time\n\n_EMPTY = ()\n\nasync def handler():\n    if not _EMPTY:\n        return\n    time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report.findings.is_empty(),
        "unreachable after always-true early return: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn comprehension_filter_is_branch() {
    let dir = temp_project(&[(
        "comp_if.py",
        "import time\n\ndef block():\n    time.sleep(1)\n    return 1\n\nasync def handler(xs, flag):\n    _ = [block() for x in xs if flag]\n",
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
