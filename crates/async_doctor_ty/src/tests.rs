//! Integration tests for M1 call-graph extraction + propagation.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use async_doctor_core::EntryLabel;

use crate::scan_path;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_project(files: &[(&str, &str)]) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("async-doctor-m1-{n}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for (name, content) in files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }
    dir
}

#[test]
fn reports_direct_time_sleep_in_async_def() {
    let dir = temp_project(&[(
        "pos.py",
        "import time\n\nasync def handler():\n    time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(report.complete, "skipped={:?}", report.skipped);
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let f = &report.findings[0];
    assert_eq!(f.rule, "blocking-sleep");
    assert_eq!(f.entry.name, "handler");
    assert_eq!(f.entry.label, EntryLabel::Other);
    assert!(f.chain[0].text.contains("sleep"));
    assert_eq!(report.exit_code(), 1);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ignores_sleep_in_sync_def() {
    let dir = temp_project(&[(
        "neg_sync.py",
        "import time\n\ndef sync_helper():\n    time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(report.findings.is_empty());
    assert_eq!(report.exit_code(), 0);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ignores_sleep_under_type_checking() {
    let dir = temp_project(&[(
        "neg_tc.py",
        "import time\nfrom typing import TYPE_CHECKING\n\nasync def handler():\n    if TYPE_CHECKING:\n        time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report.findings.is_empty(),
        "unexpected findings: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn ignores_sleep_under_if_false() {
    let dir = temp_project(&[(
        "dead.py",
        "import time\n\nasync def handler():\n    if False:\n        time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn multi_hop_cross_module_chain() {
    let dir = temp_project(&[
        (
            "helpers.py",
            "import time\n\ndef block():\n    time.sleep(1)\n",
        ),
        (
            "app.py",
            "from helpers import block\n\nasync def handler():\n    block()\n",
        ),
    ]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert_eq!(report.findings[0].chain.len(), 2);
    assert!(report.findings[0].chain[0].text.contains("block"));
    assert!(report.findings[0].chain[1].text.contains("sleep"));
    let _ = fs::remove_dir_all(dir);
}

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
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn shadowing_does_not_false_match() {
    let dir = temp_project(&[(
        "shadow.py",
        "import time as t\n\ndef time():\n    return 0\n\nasync def handler():\n    time()\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report.findings.is_empty(),
        "shadowed time() must not match time.sleep: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn alias_import_resolves_sleep() {
    let dir = temp_project(&[(
        "alias.py",
        "import time as t\n\nasync def handler():\n    t.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn from_import_resolves_sleep() {
    let dir = temp_project(&[(
        "from_imp.py",
        "from time import sleep as nap\n\nasync def go():\n    nap(0.1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn getattr_is_candidate_unresolved() {
    let dir = temp_project(&[(
        "dyn.py",
        "import time\n\nasync def handler():\n    getattr(time, 'sleep')(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report.findings.is_empty(),
        "getattr must not be a finding: {:?}",
        report.findings
    );
    assert!(
        report.candidates.iter().any(|c| c.reason == "unresolved"),
        "expected unresolved candidate: {:?}",
        report.candidates
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn entry_label_route_fastapi_style() {
    let dir = temp_project(&[(
        "routes.py",
        "import time\n\nclass App:\n    def get(self, path):\n        def deco(fn):\n            return fn\n        return deco\n\napp = App()\n\n@app.get('/')\nasync def index():\n    time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert_eq!(report.findings[0].entry.label, EntryLabel::Route);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn sync_route_not_entry() {
    let dir = temp_project(&[(
        "sync_route.py",
        "import time\n\nclass App:\n    def get(self, path):\n        def deco(fn):\n            return fn\n        return deco\n\napp = App()\n\n@app.get('/')\ndef index():\n    time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report.findings.is_empty(),
        "sync FastAPI-style route must not be an entry: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn entry_label_task_create_task() {
    let dir = temp_project(&[(
        "tasks.py",
        "import asyncio\nimport time\n\nasync def worker():\n    time.sleep(1)\n\nasync def main():\n    asyncio.create_task(worker())\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(!report.findings.is_empty(), "{:?}", report.findings);
    let worker = report
        .findings
        .iter()
        .find(|f| f.entry.name == "worker")
        .expect("worker finding");
    assert_eq!(worker.entry.label, EntryLabel::Task);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn cycle_in_call_graph() {
    let dir = temp_project(&[(
        "cycle.py",
        "import time\n\ndef a():\n    b()\n\ndef b():\n    a()\n    time.sleep(1)\n\nasync def handler():\n    a()\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn finding_ids_stable_when_lines_inserted() {
    let base = temp_project(&[(
        "stable.py",
        "import time\n\nasync def handler():\n    time.sleep(1)\n",
    )]);
    let moved = temp_project(&[(
        "stable.py",
        "# header\n# more\nimport time\n\nasync def handler():\n    time.sleep(1)\n",
    )]);
    let a = scan_path(&base).unwrap();
    let b = scan_path(&moved).unwrap();
    assert_eq!(a.findings.len(), 1);
    assert_eq!(b.findings.len(), 1);
    assert_eq!(
        a.findings[0].id, b.findings[0].id,
        "ids must ignore line shifts"
    );
    assert_ne!(a.findings[0].chain[0].line, b.findings[0].chain[0].line);
    let _ = fs::remove_dir_all(base);
    let _ = fs::remove_dir_all(moved);
}

#[test]
fn multiple_sleeps_get_distinct_ids() {
    let dir = temp_project(&[(
        "multi.py",
        "import time\n\nasync def handler():\n    time.sleep(1)\n    time.sleep(2)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 2);
    assert_ne!(report.findings[0].id, report.findings[1].id);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn subprocess_run_sink() {
    let dir = temp_project(&[(
        "sub.py",
        "import subprocess\n\nasync def handler():\n    subprocess.run(['true'])\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert_eq!(report.findings[0].rule, "blocking-subprocess");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn pathlib_read_text_sink() {
    let dir = temp_project(&[(
        "pathio.py",
        "from pathlib import Path\n\nasync def handler():\n    Path('x').read_text()\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert_eq!(report.findings[0].rule, "blocking-file");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn os_stat_is_candidate_metadata() {
    let dir = temp_project(&[(
        "meta.py",
        "import os\n\nasync def handler():\n    os.stat('.')\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    assert!(
        report.candidates.iter().any(|c| c.reason == "metadata"),
        "{:?}",
        report.candidates
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn open_builtin_sink() {
    let dir = temp_project(&[(
        "fopen.py",
        "async def handler():\n    f = open('x')\n    f.close()\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report.findings.iter().any(|f| f.rule == "blocking-file"),
        "{:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn typed_method_receiver() {
    let dir = temp_project(&[(
        "recv.py",
        "import time\n\nclass Worker:\n    def block(self):\n        time.sleep(1)\n\nasync def handler():\n    w = Worker()\n    w.block()\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert_eq!(report.findings[0].chain.len(), 2);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn async_twin_hint_when_present() {
    let dir = temp_project(&[(
        "twin.py",
        "import time\n\nclass Client:\n    def fetch(self):\n        time.sleep(1)\n    async def afetch(self):\n        return 1\n\nasync def handler():\n    Client().fetch()\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert!(
        report.findings[0].fix.hint.contains("afetch")
            || report.findings[0].fix.hint.contains("Async twin"),
        "hint={}",
        report.findings[0].fix.hint
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn syntax_error_marks_incomplete() {
    let dir = temp_project(&[(
        "broken.py",
        "import time\n\nasync def handler(:\n    time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(!report.complete);
    assert!(report.skipped.iter().any(|s| s.contains("broken.py")));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn accepts_single_py_file_path() {
    let dir = temp_project(&[(
        "only.py",
        "import time\n\nasync def handler():\n    time.sleep(1)\n",
    )]);
    let file = dir.join("only.py");
    let report = scan_path(&file).unwrap();
    assert!(report.complete);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].entry.file, "only.py");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn threading_lock_acquire_sink() {
    let dir = temp_project(&[(
        "locks.py",
        "import threading\n\nlock = threading.Lock()\n\nasync def handler():\n    lock.acquire()\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert_eq!(report.findings[0].rule, "blocking-lock");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn sqlite3_connect_sink() {
    let dir = temp_project(&[(
        "db.py",
        "import sqlite3\n\nasync def handler():\n    sqlite3.connect(':memory:')\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert_eq!(report.findings[0].rule, "blocking-db");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn urllib_urlopen_sink() {
    let dir = temp_project(&[(
        "http.py",
        "import urllib.request\n\nasync def handler():\n    urllib.request.urlopen('http://example.com')\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert_eq!(report.findings[0].rule, "blocking-http");
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

#[test]
fn function_local_import_resolves() {
    let dir = temp_project(&[(
        "local_imp.py",
        "async def handler():\n    import time\n    time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let _ = fs::remove_dir_all(dir);
}

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
