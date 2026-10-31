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
fn advice_review_a_fires_inside_try_if_loop_and_httpx_module() {
    let dir = clone_fixture("advice_review_a");
    let report = scan_path(&dir).unwrap();
    let advice = advice_entries(&report);
    for expected in [
        "p1_bound_method_unconditional",
        "p2_in_if",
        "p3_in_retry_loop",
        "p4_lambda",
        "p5_partial",
        "p6_redis_executor",
        "p7_helper",
        "p8_module_client",
        "p9_httpx_module_get",
        "p10_count_tokens",
        "p11_ctx_client_helper",
    ] {
        assert!(
            advice.contains(&expected),
            "missing advice for {expected}: {advice:?} hints={:?}",
            report
                .advice
                .iter()
                .map(|a| (&a.entry.name, &a.fix.hint))
                .collect::<Vec<_>>()
        );
    }

    let finding_rules = rules(&report);
    assert!(
        finding_rules
            .iter()
            .any(|(r, e)| *r == "nested-event-loop" && *e == "e1_nested"),
        "unconditional nested run: {finding_rules:?}"
    );
    assert!(
        finding_rules
            .iter()
            .any(|(r, e)| *r == "nested-event-loop" && *e == "e3_nested_via_helper"),
        "nested via helper: {finding_rules:?}"
    );
    assert!(
        finding_rules
            .iter()
            .all(|(r, e)| !(*r == "nested-event-loop" && *e == "e4_guarded")),
        "loop-state guarded nested run must be dropped: {finding_rules:?}"
    );
    assert!(
        report
            .candidates
            .iter()
            .all(|c| !(c.rule == "nested-event-loop" && c.entry.name == "e4_guarded")),
        "loop-state guarded must not be a candidate either: {:?}",
        report.candidates
    );
    assert!(
        finding_rules
            .iter()
            .all(|(r, e)| !(*r == "nested-event-loop" && *e == "e5_offloaded_asyncio_run")),
        "nested behind offload silent: {finding_rules:?}"
    );
    assert!(
        finding_rules
            .iter()
            .any(|(r, e)| *r == "offload-coroutine" && *e == "e2_offload_coro"),
        "offload-coroutine: {finding_rules:?}"
    );
}

#[test]
fn advice_review_c_try_and_notry_both_fire() {
    let dir = clone_fixture("advice_review_c");
    let report = scan_path(&dir).unwrap();
    let advice = advice_entries(&report);
    assert!(
        advice.iter().any(|n| n.ends_with("r1_try")),
        "try-wrapped offload must still advise: {advice:?}"
    );
    assert!(
        advice.iter().any(|n| n.ends_with("r2_notry")),
        "unconditional offload must advise: {advice:?}"
    );
}

#[test]
fn advice_review_d_lambda_in_try() {
    let dir = clone_fixture("advice_review_d");
    let report = scan_path(&dir).unwrap();
    let advice = advice_entries(&report);
    for expected in ["s1", "s2", "s3", "s4"] {
        assert!(
            advice.contains(&expected),
            "lambda body must be graded relative to lambda ({expected}): {advice:?}"
        );
    }
}

#[test]
fn advice_review_g_negatives_silent() {
    let dir = clone_fixture("advice_review_g");
    let report = scan_path(&dir).unwrap();
    assert!(
        report.advice.is_empty(),
        "negatives must be silent: {:?}",
        report.advice
    );
}

#[test]
fn advice_review_h_offload_coroutine_on_async_externals() {
    let dir = clone_fixture("advice_review_h");
    let report = scan_path(&dir).unwrap();
    let finding_rules = rules(&report);
    for expected in [
        "c1_async_ext_method",
        "c2_async_httpx",
        "c3_run_in_executor_async_ext",
    ] {
        assert!(
            finding_rules
                .iter()
                .any(|(r, e)| *r == "offload-coroutine" && *e == expected),
            "missing offload-coroutine for {expected}: {finding_rules:?}"
        );
    }
    assert!(
        finding_rules
            .iter()
            .all(|(r, e)| !(*r == "offload-coroutine" && *e == "c4_ok_plain")),
        "plain sync offload must not fire: {finding_rules:?}"
    );
    assert!(
        report.advice.is_empty(),
        "no twin advice expected: {:?}",
        report.advice
    );
}

#[test]
fn advice_azure_openai_suggests_async_azure() {
    // Method on `openai.AzureOpenAI` so the sink stays Azure-prefixed (shared
    // `openai.resources.*` would map to AsyncOpenAI).
    let dir = temp_project(&[(
        "app.py",
        concat!(
            "import asyncio\n",
            "from openai import AzureOpenAI\n",
            "\n",
            "def call_azure():\n",
            "    return AzureOpenAI().complete(model='m', messages=[])\n",
            "\n",
            "async def handler():\n",
            "    await asyncio.to_thread(call_azure)\n",
        ),
    )]);
    let sp = dir
        .join(".venv")
        .join("lib")
        .join("python3.12")
        .join("site-packages");
    std::fs::create_dir_all(sp.join("openai")).unwrap();
    std::fs::write(
        dir.join(".venv").join("pyvenv.cfg"),
        "home = /usr\ninclude-system-site-packages = false\nversion = 3.12.3\n",
    )
    .unwrap();
    std::fs::write(
        sp.join("openai").join("__init__.py"),
        concat!(
            "class AzureOpenAI:\n",
            "    def complete(self, *a, **k): pass\n",
            "class AsyncAzureOpenAI:\n",
            "    async def complete(self, *a, **k): pass\n",
            "class OpenAI:\n",
            "    pass\n",
            "class AsyncOpenAI:\n",
            "    pass\n",
        ),
    )
    .unwrap();
    let report = scan_path(&dir).unwrap();
    assert!(
        report
            .advice
            .iter()
            .any(|a| { a.entry.name == "handler" && a.fix.hint.contains("AsyncAzureOpenAI") }),
        "expected AsyncAzureOpenAI advice: {:?}",
        report
            .advice
            .iter()
            .map(|a| (&a.entry.name, &a.fix.hint))
            .collect::<Vec<_>>()
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
fn nested_event_loop_guarded_dropped() {
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
        "loop-state guarded nested run must not be a finding: {:?}",
        report.findings
    );
    assert!(
        report
            .candidates
            .iter()
            .all(|c| c.rule != "nested-event-loop"),
        "loop-state guarded nested run must not be a candidate: {:?}",
        report.candidates
    );
}

#[test]
fn nested_event_loop_non_loop_else_branch_is_finding() {
    // GCS-style else branch (storage type), not a loop-state guard.
    let dir = temp_project(&[(
        "app.py",
        concat!(
            "import asyncio\n",
            "\n",
            "def load(storage_type: str):\n",
            "    if storage_type == 's3':\n",
            "        return True\n",
            "    else:\n",
            "        return asyncio.run(asyncio.sleep(0))\n",
            "\n",
            "async def handler():\n",
            "    load('gcs')\n",
        ),
    )]);
    let report = scan_path(&dir).unwrap();
    assert!(
        report.findings.iter().any(|f| {
            f.rule == "nested-event-loop"
                && f.entry.name == "handler"
                && f.conditions.iter().any(|c| c == "async-branch")
        }),
        "non-loop else branch must stay a finding: findings={:?} candidates={:?}",
        report.findings,
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

#[test]
fn advice_sorted_by_site_file_line() {
    let dir = clone_fixture("advice_review_d");
    let report = scan_path(&dir).unwrap();
    assert!(report.advice.len() >= 2);
    let sites: Vec<_> = report
        .advice
        .iter()
        .map(|a| {
            a.chain
                .last()
                .map(|s| (s.file.as_str(), s.line))
                .unwrap_or((a.entry.file.as_str(), a.entry.line))
        })
        .collect();
    let mut sorted = sites.clone();
    sorted.sort();
    assert_eq!(sites, sorted, "advice must be sorted by site file/line");
}
