//! Wave 1 regression tests:
//! - Bound offload wrapper methods (`self.wrap(sync_fn, async_cb)` vs `self.wrap(async_fn)`)
//! - Quoted / `--path` locator parsing in `why_file_line` (`refresh_sink_sites`)
//! - Sync-only caller fan-in on `SinkSiteSummary` (`syncCallerCount` + `sampleSyncCaller`)
//! - `.venv`-aware `findings[].fix.hint` qualification via `resolved_symbols` + `SinkTable::async_twin`

use async_doctor_core::{
    AsyncTwinHit, CallEdge, CallKind, CalleeRef, ConditionTag, EntryLabel, FunctionFact,
    FunctionId, ProjectFacts, SinkHit, SinkTable, TwinConfidence, analyze,
};

struct Wave1Sinks;

fn sink(rule: &'static str, sink_qualname: &'static str, fix_hint: &'static str) -> SinkHit {
    SinkHit {
        rule,
        sink_qualname,
        fix_hint,
        candidate_only: false,
        candidate_reason: None,
        with_only: false,
        consume_only: false,
    }
}

impl SinkTable for Wave1Sinks {
    fn lookup(&self, qualname: &str) -> Option<SinkHit> {
        match qualname {
            "time.sleep" => Some(sink(
                "blocking-sleep",
                "time.sleep",
                "Replace `time.sleep(...)` with `await asyncio.sleep(...)`.",
            )),
            "httpx.get" => Some(sink(
                "blocking-http",
                "httpx.get",
                "Use `httpx.AsyncClient` (`await client.get(...)`) or offload via `await asyncio.to_thread(...)`.",
            )),
            "requests.get" => Some(sink(
                "blocking-http",
                "requests.get",
                "Use an async HTTP client (`httpx.AsyncClient` / `aiohttp.ClientSession`), or offload via `await asyncio.to_thread(...)`.",
            )),
            "sqlalchemy.orm.Session.execute" => Some(sink(
                "blocking-db",
                "sqlalchemy.orm.Session.execute",
                "Use `sqlalchemy.ext.asyncio.AsyncSession` (`await session.execute(...)`) or offload via `await asyncio.to_thread(...)`.",
            )),
            "builtins.open" => Some(sink(
                "blocking-file",
                "builtins.open",
                "Use `aiofiles.open` or offload file IO via `await asyncio.to_thread(...)`.",
            )),
            "io.TextIOWrapper.write" => Some(sink(
                "blocking-file",
                "io.TextIOWrapper.write",
                "Offload file IO via `await asyncio.to_thread(...)`.",
            )),
            _ => None,
        }
    }

    fn async_twin(&self, qualname: &str) -> Option<AsyncTwinHit> {
        match qualname {
            "httpx.get" => Some(AsyncTwinHit {
                symbol: "httpx.AsyncClient",
                same_package: true,
                needs: &[],
                any_needs: &[],
                confidence: TwinConfidence::High,
            }),
            "requests.get" => Some(AsyncTwinHit {
                symbol: "httpx.AsyncClient",
                same_package: false,
                needs: &[],
                any_needs: &[],
                confidence: TwinConfidence::High,
            }),
            "builtins.open" => Some(AsyncTwinHit {
                symbol: "aiofiles.open",
                same_package: false,
                needs: &[],
                any_needs: &[],
                confidence: TwinConfidence::High,
            }),
            "sqlalchemy.orm.Session.execute" => Some(AsyncTwinHit {
                symbol: "sqlalchemy.ext.asyncio.AsyncSession",
                same_package: true,
                needs: &["greenlet"],
                any_needs: &["asyncpg", "aiosqlite", "aiomysql", "psycopg"],
                confidence: TwinConfidence::High,
            }),
            _ => None,
        }
    }
}

fn fn_fact(id: u32, qualname: &str, is_async: bool, file: &str, line: u32) -> FunctionFact {
    FunctionFact {
        id: FunctionId(id),
        file: file.into(),
        line,
        end_line: line,
        module: "app".into(),
        qualname: qualname.into(),
        is_async,
        is_generator: false,
        label: EntryLabel::Other,
        is_sync_route: false,
        async_twins: vec![],
        param_names: vec![],
        is_third_party: false,
        package_name: None,
        package_version: None,
        invokes_params: vec![],
        is_offload_wrapper: false,
        offload_decorator: None,
    }
}

fn route_fact(id: u32, qualname: &str, file: &str, line: u32) -> FunctionFact {
    let mut f = fn_fact(id, qualname, true, file, line);
    f.label = EntryLabel::Route;
    f
}

fn edge(caller: u32, callee: CalleeRef, file: &str, line: u32, text: &str) -> CallEdge {
    CallEdge {
        caller: FunctionId(caller),
        callee,
        file: file.into(),
        line,
        text: text.into(),
        kind: CallKind::Call,
        conditions: vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
        occurrence: 0,
        is_offload_wrapper: false,
        positional_literals: vec![],
        keyword_literals: vec![],
        positional_callables: vec![],
        positional_externals: vec![],
        positional_constructed_receivers: vec![],
        keyword_callables: vec![],
        param_guards: vec![],
        is_awaited: false,
        callee_is_coroutine: None,
        is_eager_offload_arg: false,
        is_bound_call: false,
        context_from_caller: None,
        context_from_edge: None,
        constructed_receiver: None,
    }
}

fn proj_edge(caller: u32, callee: u32, file: &str, line: u32, text: &str) -> CallEdge {
    edge(
        caller,
        CalleeRef::Project(FunctionId(callee)),
        file,
        line,
        text,
    )
}

fn ext_edge(caller: u32, qualname: &str, file: &str, line: u32, text: &str) -> CallEdge {
    edge(
        caller,
        CalleeRef::External {
            qualname: qualname.into(),
        },
        file,
        line,
        text,
    )
}

fn base_facts(functions: Vec<FunctionFact>, edges: Vec<CallEdge>) -> ProjectFacts {
    ProjectFacts {
        functions,
        edges,
        complete: true,
        ..Default::default()
    }
}

#[test]
fn bound_offload_wrapper_with_async_callback_does_not_shift_sync_target() {
    let entry = route_fact(0, "Pool.handle", "pool.py", 1);
    let mut run_in_pool = fn_fact(1, "Pool.run_in_pool", true, "pool.py", 10);
    run_in_pool.param_names = vec!["self".into(), "fn".into(), "on_done".into()];
    run_in_pool.is_offload_wrapper = true;
    let sync_fn = fn_fact(2, "fetch_sync", false, "pool.py", 20);
    let async_cb = fn_fact(3, "notify_done", true, "pool.py", 30);

    // `CallCollector` already excludes `self` from `positional_callables` on bound calls:
    // `await self.run_in_pool(fetch_sync, notify_done)` -> `[Some(2), Some(3)]`.
    let mut call_wrap = edge(
        0,
        CalleeRef::Project(FunctionId(1)),
        "pool.py",
        5,
        "await self.run_in_pool(fetch_sync, notify_done)",
    );
    call_wrap.is_bound_call = true;
    call_wrap.is_awaited = true;
    call_wrap.callee_is_coroutine = Some(true);
    call_wrap.positional_callables = vec![Some(FunctionId(2)), Some(FunctionId(3))];

    let sync_sink = edge(
        2,
        CalleeRef::External {
            qualname: "httpx.get".into(),
        },
        "pool.py",
        21,
        "httpx.get(url)",
    );

    let mut facts = base_facts(
        vec![entry, run_in_pool, sync_fn, async_cb],
        vec![call_wrap, sync_sink],
    );
    facts.resolved_symbols.insert("httpx.AsyncClient".into());

    let report = analyze(&facts, &Wave1Sinks);
    assert!(
        report.findings.is_empty(),
        "bound offload wrapper passing (sync_fn, async_cb) must not emit offload-coroutine: {:?}",
        report.findings
    );
    assert_eq!(report.advice.len(), 1);
    assert_eq!(report.advice[0].rule, "offload-has-async-twin");
}

#[test]
fn bound_and_unbound_offload_wrappers_still_catch_async_target_and_unbound_self() {
    let entry = route_fact(0, "Pool.handle", "pool.py", 1);
    let mut run_in_pool = fn_fact(1, "Pool.run_in_pool", true, "pool.py", 10);
    run_in_pool.param_names = vec!["self".into(), "fn".into(), "cb".into()];
    run_in_pool.is_offload_wrapper = true;
    let async_fn = fn_fact(2, "fetch_async", true, "pool.py", 20);
    let sync_cb = fn_fact(3, "done_sync", false, "pool.py", 30);

    // 1. Bound `self.run_in_pool(async_fn)` -> must emit `offload-coroutine`.
    let mut bound_single = edge(
        0,
        CalleeRef::Project(FunctionId(1)),
        "pool.py",
        5,
        "await self.run_in_pool(fetch_async)",
    );
    bound_single.is_bound_call = true;
    bound_single.is_awaited = true;
    bound_single.callee_is_coroutine = Some(true);
    bound_single.positional_callables = vec![Some(FunctionId(2))];

    let report_single = analyze(
        &base_facts(
            vec![
                entry.clone(),
                run_in_pool.clone(),
                async_fn.clone(),
                sync_cb.clone(),
            ],
            vec![bound_single],
        ),
        &Wave1Sinks,
    );
    assert_eq!(report_single.findings.len(), 1);
    assert_eq!(report_single.findings[0].rule, "offload-coroutine");

    // 2. Bound `self.run_in_pool(async_fn, sync_cb)` -> must also emit `offload-coroutine`.
    let mut bound_two = edge(
        0,
        CalleeRef::Project(FunctionId(1)),
        "pool.py",
        6,
        "await self.run_in_pool(fetch_async, done_sync)",
    );
    bound_two.is_bound_call = true;
    bound_two.is_awaited = true;
    bound_two.callee_is_coroutine = Some(true);
    bound_two.positional_callables = vec![Some(FunctionId(2)), Some(FunctionId(3))];

    let report_two = analyze(
        &base_facts(
            vec![entry.clone(), run_in_pool.clone(), async_fn, sync_cb],
            vec![bound_two],
        ),
        &Wave1Sinks,
    );
    assert_eq!(report_two.findings.len(), 1);
    assert_eq!(report_two.findings[0].rule, "offload-coroutine");

    // 3. Unbound `Pool.run_in_pool(pool_self, sync_fn, async_cb)` (`is_bound_call = false`) ->
    //    explicit `pool_self` at slot 0 is drained so `sync_fn` becomes slot 0.
    let sync_fn = fn_fact(2, "fetch_sync", false, "pool.py", 20);
    let async_cb = fn_fact(3, "notify_done", true, "pool.py", 30);
    let mut unbound = edge(
        0,
        CalleeRef::Project(FunctionId(1)),
        "pool.py",
        7,
        "await Pool.run_in_pool(pool, fetch_sync, notify_done)",
    );
    unbound.is_bound_call = false;
    unbound.is_awaited = true;
    unbound.callee_is_coroutine = Some(true);
    unbound.positional_callables = vec![None, Some(FunctionId(2)), Some(FunctionId(3))];
    let sync_sink = edge(
        2,
        CalleeRef::External {
            qualname: "httpx.get".into(),
        },
        "pool.py",
        21,
        "httpx.get(url)",
    );

    let mut facts_unbound = base_facts(
        vec![entry, run_in_pool, sync_fn, async_cb],
        vec![unbound, sync_sink],
    );
    facts_unbound
        .resolved_symbols
        .insert("httpx.AsyncClient".into());
    let report_unbound = analyze(&facts_unbound, &Wave1Sinks);
    assert!(report_unbound.findings.is_empty());
    assert_eq!(report_unbound.advice.len(), 1);
    assert_eq!(report_unbound.advice[0].rule, "offload-has-async-twin");
}

#[test]
fn refresh_sink_sites_parses_quoted_targets_with_spaces_and_path_flag() {
    let entry = route_fact(0, "save_handler", "my app/a.py", 1);
    let mut open_edge = edge(
        0,
        CalleeRef::External {
            qualname: "builtins.open".into(),
        },
        "my app/a.py",
        10,
        "open('out.txt', 'w')",
    );
    open_edge.kind = CallKind::WithEnter;
    let write_edge = edge(
        0,
        CalleeRef::External {
            qualname: "io.TextIOWrapper.write".into(),
        },
        "my app/a.py",
        11,
        "f.write(data)",
    );

    let mut report = analyze(
        &base_facts(vec![entry], vec![open_edge, write_edge]),
        &Wave1Sinks,
    );
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.sink_sites.len(), 1);
    assert_eq!(report.sink_sites[0].line, 10);

    // Simulate CLI rewrite (`main.rs` quotes paths with spaces and appends `--path <root>`).
    report.findings[0].why = "async-doctor why \"my app/a.py:10\" --path .".into();
    report.refresh_sink_sites();
    assert_eq!(report.sink_sites[0].file, "my app/a.py");
    assert_eq!(report.sink_sites[0].line, 10);

    // Single-quoted target + quoted `--path`.
    report.findings[0].why = "async-doctor why 'my app/a.py:10' --path \"my scan root\"".into();
    report.refresh_sink_sites();
    assert_eq!(report.sink_sites[0].file, "my app/a.py");
    assert_eq!(report.sink_sites[0].line, 10);

    // Unquoted target + `--python` (with and without `--path`).
    let mut unquoted_entry = route_fact(0, "save_handler", "app/a.py", 1);
    unquoted_entry.end_line = 12;
    let mut unquoted_open = edge(
        0,
        CalleeRef::External {
            qualname: "builtins.open".into(),
        },
        "app/a.py",
        10,
        "open('out.txt', 'w')",
    );
    unquoted_open.kind = CallKind::WithEnter;
    let unquoted_write = edge(
        0,
        CalleeRef::External {
            qualname: "io.TextIOWrapper.write".into(),
        },
        "app/a.py",
        11,
        "f.write(data)",
    );
    let mut unquoted_report = analyze(
        &base_facts(vec![unquoted_entry], vec![unquoted_open, unquoted_write]),
        &Wave1Sinks,
    );
    unquoted_report.findings[0].why =
        "async-doctor why app/a.py:10 --python /opt/venv/bin/python".into();
    unquoted_report.refresh_sink_sites();
    assert_eq!(unquoted_report.sink_sites[0].file, "app/a.py");
    assert_eq!(unquoted_report.sink_sites[0].line, 10);

    unquoted_report.findings[0].why =
        "async-doctor why app/a.py:10 --path . --python /opt/venv/bin/python".into();
    unquoted_report.refresh_sink_sites();
    assert_eq!(unquoted_report.sink_sites[0].file, "app/a.py");
    assert_eq!(unquoted_report.sink_sites[0].line, 10);

    // Quoted target + `--python` (both standalone and combined with `--path`).
    report.findings[0].why =
        "async-doctor why \"my app/a.py:10\" --python \"/opt/my venv/bin/python\"".into();
    report.refresh_sink_sites();
    assert_eq!(report.sink_sites[0].file, "my app/a.py");
    assert_eq!(report.sink_sites[0].line, 10);

    report.findings[0].why =
        "async-doctor why 'my app/a.py:10' --path . --python /opt/venv/bin/python".into();
    report.refresh_sink_sites();
    assert_eq!(report.sink_sites[0].file, "my app/a.py");
    assert_eq!(report.sink_sites[0].line, 10);
}

#[test]
fn sink_sites_report_sync_caller_fan_in_and_omit_when_zero() {
    // Case A: 2 async routes share `shared_helper`, which ALSO has 2 sync-only callers.
    let route_a = route_fact(0, "route_a", "routes.py", 1);
    let route_b = route_fact(1, "route_b", "routes.py", 10);
    let shared_helper = fn_fact(2, "shared_helper", false, "helpers.py", 20);
    let cli_main = fn_fact(3, "cli_main", false, "cli.py", 5);
    let cron_job = fn_fact(4, "cron_job", false, "worker.py", 15);
    let async_only_helper = fn_fact(5, "async_only_helper", false, "helpers.py", 40);

    let edges = vec![
        proj_edge(0, 2, "routes.py", 3, "shared_helper()"),
        proj_edge(1, 2, "routes.py", 13, "shared_helper()"),
        ext_edge(2, "time.sleep", "helpers.py", 22, "time.sleep(1)"),
        proj_edge(3, 2, "cli.py", 12, "shared_helper()"),
        proj_edge(4, 2, "worker.py", 25, "shared_helper()"),
        proj_edge(0, 5, "routes.py", 4, "async_only_helper()"),
        proj_edge(1, 5, "routes.py", 14, "async_only_helper()"),
        ext_edge(5, "time.sleep", "helpers.py", 42, "time.sleep(2)"),
    ];

    let mut report = analyze(
        &base_facts(
            vec![
                route_a,
                route_b,
                shared_helper,
                cli_main,
                cron_job,
                async_only_helper,
            ],
            edges,
        ),
        &Wave1Sinks,
    );
    assert_eq!(report.sink_sites.len(), 2);

    let shared_site = report.sink_sites.iter().find(|s| s.line == 22).unwrap();
    assert_eq!(shared_site.sync_caller_count, 2);
    assert_eq!(shared_site.sample_sync_caller.as_deref(), Some("cli.py:12"));

    let pure_async_site = report.sink_sites.iter().find(|s| s.line == 42).unwrap();
    assert_eq!(pure_async_site.sync_caller_count, 0);
    assert_eq!(pure_async_site.sample_sync_caller, None);

    // Refreshing sink sites (e.g. after CLI filtering) must preserve sync-caller metadata.
    report.refresh_sink_sites();
    let refreshed = report.sink_sites.iter().find(|s| s.line == 22).unwrap();
    assert_eq!(refreshed.sync_caller_count, 2);
    assert_eq!(refreshed.sample_sync_caller.as_deref(), Some("cli.py:12"));

    // Case B: single-route chain through followed third-party sink (`route -> mid -> tp_fn -> sleep`)
    // where `sync_cli -> mid` (1 finding, third-party sink file, no choke point).
    let single_route = route_fact(0, "single_route", "routes.py", 1);
    let mid = fn_fact(1, "mid", false, "helpers.py", 10);
    let mut tp_fn = fn_fact(2, "fetch", false, "site-packages/pkg/client.py", 50);
    tp_fn.is_third_party = true;
    tp_fn.package_name = Some("pkg".into());
    let sync_cli = fn_fact(3, "sync_cli", false, "cli.py", 35);
    let tp_report = analyze(
        &base_facts(
            vec![single_route, mid, tp_fn, sync_cli],
            vec![
                proj_edge(0, 1, "routes.py", 2, "mid()"),
                proj_edge(1, 2, "helpers.py", 11, "pkg.fetch()"),
                ext_edge(
                    2,
                    "time.sleep",
                    "site-packages/pkg/client.py",
                    52,
                    "time.sleep(1)",
                ),
                proj_edge(3, 1, "cli.py", 40, "mid()"),
            ],
        ),
        &Wave1Sinks,
    );
    assert_eq!(tp_report.sink_sites.len(), 1);
    assert_eq!(tp_report.sink_sites[0].sync_caller_count, 1);
    assert_eq!(
        tp_report.sink_sites[0].sample_sync_caller.as_deref(),
        Some("cli.py:40")
    );
}

#[test]
fn findings_fix_hint_reflects_scanned_venv_async_twins() {
    let make_single_sink_facts = |qualname: &str, symbols: &[&str]| {
        let entry = route_fact(0, "handler", "app.py", 1);
        let sink_edge = edge(
            0,
            CalleeRef::External {
                qualname: qualname.into(),
            },
            "app.py",
            5,
            "call()",
        );
        let mut facts = base_facts(vec![entry], vec![sink_edge]);
        for &sym in symbols {
            facts.resolved_symbols.insert(sym.into());
        }
        facts
    };

    // 1. Empty `resolved_symbols` (synthetic unit tests without env scan): unchanged base hint.
    let empty_env = analyze(&make_single_sink_facts("httpx.get", &[]), &Wave1Sinks);
    assert_eq!(
        empty_env.findings[0].fix.hint,
        "Use `httpx.AsyncClient` (`await client.get(...)`) or offload via `await asyncio.to_thread(...)`."
    );

    // 2. Twin installed in scanned env -> notes `(installed in env)`.
    let installed = analyze(
        &make_single_sink_facts("httpx.get", &["httpx.AsyncClient"]),
        &Wave1Sinks,
    );
    assert!(
        installed.findings[0]
            .fix
            .hint
            .contains("(installed in env)")
    );

    // 3. Non-empty scanned env where async twin package is NOT installed -> warns & favors to_thread.
    let missing_pkg = analyze(
        &make_single_sink_facts("requests.get", &["redis.asyncio"]),
        &Wave1Sinks,
    );
    let missing_hint = &missing_pkg.findings[0].fix.hint;
    assert!(
        missing_hint.contains("not installed in the scanned env")
            && missing_hint.contains("asyncio.to_thread")
    );

    // 4. SQLAlchemy `AsyncSession` + `greenlet` without async DB driver (`any_needs`) ->
    //    warns driver is not installed; once `asyncpg` is present, marks `(installed in env)`.
    let missing_driver = analyze(
        &make_single_sink_facts(
            "sqlalchemy.orm.Session.execute",
            &["sqlalchemy.ext.asyncio.AsyncSession", "greenlet"],
        ),
        &Wave1Sinks,
    );
    assert!(
        missing_driver.findings[0]
            .fix
            .hint
            .contains("not installed in the scanned env")
    );

    let with_driver = analyze(
        &make_single_sink_facts(
            "sqlalchemy.orm.Session.execute",
            &["sqlalchemy.ext.asyncio.AsyncSession", "greenlet", "asyncpg"],
        ),
        &Wave1Sinks,
    );
    assert!(
        with_driver.findings[0]
            .fix
            .hint
            .contains("(installed in env)")
    );
}
