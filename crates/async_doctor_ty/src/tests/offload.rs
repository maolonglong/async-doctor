//! offload integration tests.

use std::fs;

use super::helpers::temp_project;
use crate::scan_path;

#[test]
fn offload_to_thread_cuts() {
    let dir = temp_project(&[(
        "off.py",
        "import asyncio\nimport time\n\ndef work():\n    time.sleep(1)\n\nasync def handler():\n    await asyncio.to_thread(work)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report.findings.is_empty(),
        "to_thread should cut: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn offload_lambda_cuts() {
    let dir = temp_project(&[(
        "lam.py",
        "import asyncio\nimport time\n\nasync def handler():\n    await asyncio.to_thread(lambda: time.sleep(1))\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report.findings.is_empty(),
        "lambda offload should cut: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn offload_eager_call_still_reports() {
    let dir = temp_project(&[(
        "eager.py",
        "import asyncio\nimport time\n\ndef work():\n    time.sleep(1)\n    return 1\n\nasync def handler():\n    await asyncio.to_thread(work())\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        !report.findings.is_empty(),
        "eager work() must still report: {:?}",
        report.findings
    );
    assert!(
        report.findings.iter().any(|f| {
            f.fix.hint.contains("pass the callable") || f.fix.hint.contains("rather than calling")
        }),
        "eager hint must say pass callable: {:?}",
        report
            .findings
            .iter()
            .map(|f| &f.fix.hint)
            .collect::<Vec<_>>()
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn run_in_executor_cuts() {
    let dir = temp_project(&[(
        "exec.py",
        "import asyncio\nimport time\n\ndef work():\n    time.sleep(1)\n\nasync def handler():\n    loop = asyncio.get_running_loop()\n    await loop.run_in_executor(None, work)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report.findings.is_empty(),
        "run_in_executor should cut: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

/// `sync_to_async` / `database_sync_to_async` / project offload decorators cut
/// the call edge — the decorated body never runs on the caller's loop.
#[test]
fn offload_decorators_cut() {
    use super::helpers::clone_fixture;
    let dir = clone_fixture("m4_offload_decorators");
    let report = scan_path(&dir).unwrap();
    let cut_entries = [
        "via_bare",
        "via_called",
        "via_db",
        "via_project",
        "via_project_indirect",
    ];
    for entry in cut_entries {
        assert!(
            !report.findings.iter().any(|f| f.entry.name == entry),
            "{entry} must have no findings (offload decorator cut): {:?}",
            report.findings
        );
    }
    // Transparent decorators must keep reporting.
    for entry in ["via_fake_name", "via_retry"] {
        assert!(
            report.findings.iter().any(|f| f.entry.name == entry),
            "{entry} must keep its blocking-sleep finding: {:?}",
            report.findings
        );
    }
    let _ = fs::remove_dir_all(dir);
}
