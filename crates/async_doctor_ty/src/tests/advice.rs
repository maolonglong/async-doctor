//! Advice tier + nested-event-loop / offload-coroutine integration tests.

use super::helpers::{clone_fixture, temp_project};
use crate::scan_path;

fn rules(report: &async_doctor_core::Report) -> Vec<(&str, &str)> {
    report
        .findings
        .iter()
        .map(|f| (f.rule.as_str(), f.entry.name.as_str()))
        .collect()
}

fn advice_entries(report: &async_doctor_core::Report) -> Vec<&str> {
    let mut names: Vec<&str> = report
        .advice
        .iter()
        .filter(|a| a.rule == "offload-has-async-twin")
        .map(|a| a.entry.name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

#[test]
fn advice_offload_twin_positives_and_negatives() {
    let dir = clone_fixture("advice_offload_twin");
    let report = scan_path(&dir).unwrap();

    let advice = advice_entries(&report);
    for expected in [
        "advice_to_thread_httpx",
        "advice_run_in_executor_redis",
        "advice_partial_httpx",
        "advice_lambda_httpx",
        "advice_bound_method",
        "advice_asgiref_sync_to_async",
    ] {
        assert!(
            advice.contains(&expected),
            "missing advice for {expected}: {advice:?} full={:?}",
            report
                .advice
                .iter()
                .map(|a| (&a.rule, &a.entry.name, &a.fix.hint))
                .collect::<Vec<_>>()
        );
    }

    for banned in [
        "skip_pymongo_offload",
        "skip_requests_offload",
        "skip_mixed_sinks",
        "skip_sync_caller_exemption",
        "nested_behind_offload_ok",
    ] {
        assert!(
            !advice.contains(&banned),
            "advice must not fire for {banned}: {advice:?}"
        );
    }

    // Advice never fails the exit code by itself when only advice is present —
    // but this fixture also has nested-event-loop / offload-coroutine findings.
    assert!(
        report
            .advice
            .iter()
            .all(|a| a.category.as_str() == "performance"),
        "offload-has-async-twin is performance advice"
    );

    let finding_rules = rules(&report);
    assert!(
        finding_rules
            .iter()
            .any(|(r, e)| *r == "nested-event-loop" && *e == "nested_loop_error"),
        "nested_loop_error must be a finding: {finding_rules:?}"
    );
    assert!(
        finding_rules
            .iter()
            .all(|(r, e)| !(*r == "nested-event-loop" && *e == "nested_behind_offload_ok")),
        "nested loop behind offload must not be a finding: {finding_rules:?}"
    );
    assert!(
        finding_rules
            .iter()
            .any(|(r, e)| *r == "offload-coroutine" && *e == "offload_async_def_error"),
        "offload_async_def_error must be a finding: {finding_rules:?}"
    );
}

#[test]
fn advice_twin_not_installed_skips() {
    let dir = clone_fixture("advice_twin_missing");
    let report = scan_path(&dir).unwrap();
    assert!(
        report.advice.is_empty(),
        "without AsyncClient installed, no advice: {:?}",
        report.advice
    );
    assert!(
        report.findings.is_empty(),
        "offload cuts the blocking finding: {:?}",
        report.findings
    );
}

#[test]
fn advice_needs_missing_skips() {
    let dir = clone_fixture("advice_needs_missing");
    let report = scan_path(&dir).unwrap();
    assert!(
        report.advice.is_empty(),
        "elasticsearch twin needs aiohttp: {:?}",
        report.advice
    );
}

#[test]
fn advice_does_not_affect_exit_code() {
    let dir = temp_project(&[(
        "app.py",
        concat!(
            "import asyncio\n",
            "import httpx\n",
            "\n",
            "def fetch():\n",
            "    c = httpx.Client()\n",
            "    return c.get('https://example.com')\n",
            "\n",
            "async def handler():\n",
            "    await asyncio.to_thread(fetch)\n",
        ),
    )]);
    // Minimal httpx stub with AsyncClient so advice can fire.
    let sp = dir
        .join(".venv")
        .join("lib")
        .join("python3.12")
        .join("site-packages");
    std::fs::create_dir_all(&sp).unwrap();
    std::fs::write(
        dir.join(".venv").join("pyvenv.cfg"),
        "home = /usr\ninclude-system-site-packages = false\nversion = 3.12.3\n",
    )
    .unwrap();
    std::fs::write(
        sp.join("httpx.py"),
        "class Client:\n    def get(self, *a, **k): pass\nclass AsyncClient:\n    async def get(self, *a, **k): pass\n",
    )
    .unwrap();
    let report = scan_path(&dir).unwrap();
    assert!(
        !report.advice.is_empty(),
        "expected advice with twin installed: {:?}",
        report.advice
    );
    assert!(
        report.findings.is_empty(),
        "no findings expected: {:?}",
        report.findings
    );
    assert_eq!(report.exit_code(), async_doctor_core::EXIT_OK);
}

#[test]
fn nested_event_loop_direct() {
    let dir = temp_project(&[(
        "app.py",
        "import asyncio\n\nasync def bad():\n    asyncio.run(asyncio.sleep(0))\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.rule == "nested-event-loop" && f.entry.name == "bad"),
        "{:?}",
        report.findings
    );
}

#[test]
fn nested_event_loop_guarded_is_candidate() {
    // Mirrors litellm `_dispatch_from_sync`: asyncio.run only when no loop.
    let dir = temp_project(&[(
        "app.py",
        concat!(
            "import asyncio\n",
            "\n",
            "def dispatch(coro_factory):\n",
            "    try:\n",
            "        loop = asyncio.get_event_loop()\n",
            "    except RuntimeError:\n",
            "        loop = None\n",
            "    if loop is None:\n",
            "        asyncio.run(coro_factory())\n",
            "    elif loop.is_running():\n",
            "        loop.create_task(coro_factory())\n",
            "    else:\n",
            "        loop.run_until_complete(coro_factory())\n",
            "\n",
            "async def handler():\n",
            "    dispatch(lambda: asyncio.sleep(0))\n",
        ),
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.rule != "nested-event-loop"),
        "guarded nested run must not be a finding: {:?}",
        report.findings
    );
    assert!(
        report
            .candidates
            .iter()
            .any(|c| c.rule == "nested-event-loop" && c.entry.name == "handler"),
        "expected nested-event-loop candidate: {:?}",
        report.candidates
    );
}

#[test]
fn offload_coroutine_finding() {
    let dir = temp_project(&[(
        "app.py",
        concat!(
            "import asyncio\n",
            "\n",
            "async def work():\n",
            "    return 1\n",
            "\n",
            "async def bad():\n",
            "    await asyncio.to_thread(work)\n",
        ),
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.rule == "offload-coroutine" && f.entry.name == "bad"),
        "{:?}",
        report.findings
    );
}
