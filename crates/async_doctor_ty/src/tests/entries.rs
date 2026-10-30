//! entries integration tests.

use async_doctor_core::EntryLabel;
use std::fs;

use super::helpers::temp_project;
use crate::scan_path;

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
fn fastapi_imported_router_decorator_is_route() {
    let dir = temp_project(&[
        (
            "routes.py",
            "class APIRouter:\n    def get(self, path):\n        def deco(fn):\n            return fn\n        return deco\n\nrouter = APIRouter()\n",
        ),
        (
            "app.py",
            "import time\nfrom routes import router\n\n@router.get('/')\nasync def index():\n    time.sleep(1)\n",
        ),
    ]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert_eq!(report.findings[0].entry.label, EntryLabel::Route);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn fastapi_api_route_decorator_is_route() {
    let dir = temp_project(&[(
        "api_route.py",
        "import time\n\nclass App:\n    def api_route(self, path):\n        def deco(fn):\n            return fn\n        return deco\n\napp = App()\n\n@app.api_route('/x')\nasync def index():\n    time.sleep(1)\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert_eq!(report.findings[0].entry.label, EntryLabel::Route);
    let _ = fs::remove_dir_all(dir);
}
