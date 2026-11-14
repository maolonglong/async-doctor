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

#[test]
fn fastapi_on_event_startup_shutdown_is_lifecycle_not_route() {
    let dir = temp_project(&[
        (".venv/pyvenv.cfg", "home = /usr/bin\n"),
        (
            ".venv/lib/python3.12/site-packages/httpx/__init__.py",
            concat!(
                "class AsyncClient:\n",
                "    async def __aenter__(self):\n",
                "        return self\n",
                "    async def __aexit__(self, *args):\n",
                "        pass\n",
            ),
        ),
        (
            "lifecycle.py",
            r#"
import time
import httpx

class App:
    def on_event(self, event_type: str):
        def deco(fn):
            return fn
        return deco

app = App()

@app.on_event("startup")
async def init_db():
    time.sleep(1)

@app.on_event("shutdown")
async def close_pools():
    async with httpx.AsyncClient() as client:
        _ = client
"#,
        ),
    ]);
    let report = scan_path(&dir).unwrap();
    let _ = fs::remove_dir_all(dir);

    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    assert_eq!(report.findings[0].entry.name, "init_db");
    assert_eq!(
        report.findings[0].entry.label,
        EntryLabel::Lifecycle,
        "@app.on_event must be EntryLabel::Lifecycle, not Route"
    );
    assert!(
        !report
            .advice
            .iter()
            .any(|a| a.rule == "ephemeral-async-client"),
        "Lifecycle @app.on_event must not trigger per-route ephemeral-async-client: {:?}",
        report.advice
    );
}

#[test]
fn fastapi_depends_and_annotated_wire_async_and_threadpool_sync_deps() {
    let dir = temp_project(&[
        (
            "tests/di_helpers.py",
            r#"
import time
from fastapi import Depends

async def async_dep() -> str:
    time.sleep(1)
    return "ok"

async def async_gen_dep():
    time.sleep(1)
    yield "db"

async def async_auth_dep() -> str:
    time.sleep(1)
    return "user"

async def sub_dep() -> int:
    time.sleep(1)
    return 42

async def outer_dep(v: int = Depends(sub_dep)) -> int:
    return v
"#,
        ),
        (
            "deps_app.py",
            r#"
from typing import Annotated
import httpx
from fastapi import Depends, Security
from tests.di_helpers import async_auth_dep, async_dep, async_gen_dep, outer_dep

def sync_http_dep() -> str:
    return httpx.get("https://example.com")

async def route_default(svc: str = Depends(async_dep)) -> str:
    return svc

async def route_annotated(svc: Annotated[str, Depends(async_dep)]) -> str:
    return svc

async def route_yield_dep(db=Depends(async_gen_dep)):
    return db

async def route_security(user: Annotated[str, Security(async_auth_dep, scopes=["read"])]) -> str:
    return user

async def route_transitive(val: int = Depends(outer_dep)) -> int:
    return val

async def route_sync_dep_offloaded(
    a: str = Depends(sync_http_dep),
    b: Annotated[str, Depends(sync_http_dep)] = "",
) -> str:
    return a + b
"#,
        ),
    ]);
    super::helpers::copy_dir(&super::helpers::advice_shared_venv(), &dir.join(".venv"));
    let fastapi_dir = dir.join(".venv/lib/python3.12/site-packages/fastapi");
    fs::create_dir_all(&fastapi_dir).unwrap();
    fs::write(
        fastapi_dir.join("__init__.py"),
        concat!(
            "def Depends(dependency=None, *, use_cache: bool = True):\n",
            "    return dependency\n",
            "def Security(dependency=None, *, scopes=None, use_cache: bool = True):\n",
            "    return dependency\n",
        ),
    )
    .unwrap();
    let report = scan_path(&dir).unwrap();
    let _ = fs::remove_dir_all(dir);

    let finding_entries: std::collections::BTreeSet<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for expected in [
        "route_default",
        "route_annotated",
        "route_yield_dep",
        "route_security",
        "route_transitive",
    ] {
        assert!(
            finding_entries.contains(expected),
            "expected blocking finding propagated into {expected} via Depends/Security: {finding_entries:?}"
        );
    }
    assert!(
        !finding_entries.contains("route_sync_dep_offloaded"),
        "sync def FastAPI dependency runs in Starlette threadpool and must not emit blocking finding: {:?}",
        report.findings
    );
    assert!(
        report.advice.iter().any(|a| {
            a.entry.name == "route_sync_dep_offloaded" && a.rule == "offload-has-async-twin"
        }),
        "sync def FastAPI dependency should wire positional_callables for offload-has-async-twin: {:?}",
        report.advice
    );
}
