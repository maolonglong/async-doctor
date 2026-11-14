//! Regression tests for core call-graph propagation, parameter guards,
//! higher-order binding, finding ID disambiguation, and non-production chains.

use std::collections::{BTreeMap, BTreeSet};

use async_doctor_core::{
    AsyncTwinHit, CallEdge, CallKind, CalleeRef, ConditionTag, DecoratorRef, EntryLabel,
    FunctionFact, FunctionId, LiteralArg, OffloadKind, ParamGuard, ProjectFacts, SinkHit,
    SinkTable, TwinConfidence, analyze,
};

struct RegSinks;

impl SinkTable for RegSinks {
    fn lookup(&self, qualname: &str) -> Option<SinkHit> {
        match qualname {
            "time.sleep" => Some(SinkHit {
                rule: "blocking-sleep",
                sink_qualname: "time.sleep",
                fix_hint: "use asyncio.sleep",
                candidate_only: false,
                candidate_reason: None,
                with_only: false,
                consume_only: false,
            }),
            "httpx.get" => Some(SinkHit {
                rule: "blocking-http",
                sink_qualname: "httpx.get",
                fix_hint: "use httpx.AsyncClient",
                candidate_only: false,
                candidate_reason: None,
                with_only: false,
                consume_only: false,
            }),
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

fn edge(
    caller: u32,
    callee: CalleeRef,
    file: &str,
    line: u32,
    text: &str,
    conditions: Vec<ConditionTag>,
) -> CallEdge {
    CallEdge {
        caller: FunctionId(caller),
        callee,
        file: file.into(),
        line,
        text: text.into(),
        kind: CallKind::Call,
        conditions,
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

#[test]
fn bound_method_param_guard_and_higher_order_respect_self_offset() {
    // class Svc:
    //     def helper(self, skip_io: bool, do_sleep: bool):
    //         if do_sleep:
    //             time.sleep(1)
    //     def run_cb(self, cb):
    //         return cb()
    //
    // async def entry(svc: Svc):
    //     svc.helper(True, False)  # do_sleep is False -> must NOT report!
    //     svc.run_cb(blocker)      # cb is blocker -> MUST report!
    let entry = fn_fact(0, "entry", true, "app.py", 1);
    let mut helper = fn_fact(1, "Svc.helper", false, "app.py", 10);
    helper.param_names = vec!["self".into(), "skip_io".into(), "do_sleep".into()];
    let mut run_cb = fn_fact(2, "Svc.run_cb", false, "app.py", 20);
    run_cb.param_names = vec!["self".into(), "cb".into()];
    run_cb.invokes_params = vec![1];
    let blocker = fn_fact(3, "blocker", false, "app.py", 30);

    let mut call_helper = edge(
        0,
        CalleeRef::Project(FunctionId(1)),
        "app.py",
        2,
        "svc.helper(True, False)",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );
    call_helper.is_bound_call = true;
    call_helper.positional_literals = vec![Some(LiteralArg::True), Some(LiteralArg::False)];

    let mut helper_sleep = edge(
        1,
        CalleeRef::External {
            qualname: "time.sleep".into(),
        },
        "app.py",
        12,
        "time.sleep(1)",
        vec![ConditionTag::AsyncBranch],
    );
    // `do_sleep` is index 2 in `param_names` (`["self", "skip_io", "do_sleep"]`).
    helper_sleep.param_guards = vec![ParamGuard {
        index: 2,
        inverted: false,
    }];

    let mut call_run_cb = edge(
        0,
        CalleeRef::Project(FunctionId(2)),
        "app.py",
        3,
        "svc.run_cb(blocker)",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );
    call_run_cb.is_bound_call = true;
    call_run_cb.positional_callables = vec![Some(FunctionId(3))];

    let param_invoke = edge(
        2,
        CalleeRef::ParamInvoke { index: 1 },
        "app.py",
        21,
        "cb()",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );

    let blocker_sleep = edge(
        3,
        CalleeRef::External {
            qualname: "time.sleep".into(),
        },
        "app.py",
        31,
        "time.sleep(2)",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );

    let facts = ProjectFacts {
        functions: vec![entry, helper, run_cb, blocker],
        edges: vec![
            call_helper,
            helper_sleep,
            call_run_cb,
            param_invoke,
            blocker_sleep,
        ],
        ..Default::default()
    };

    let report = analyze(&facts, &RegSinks);
    assert_eq!(
        report.candidates.len(),
        0,
        "svc.helper(True, False) is provably false and must not become a candidate: {:?}",
        report.candidates
    );
    assert_eq!(
        report.findings.len(),
        1,
        "only svc.run_cb(blocker) should fire, got: {:?}",
        report.findings
    );
    assert_eq!(report.findings[0].chain.last().unwrap().line, 31);
}

#[test]
fn intermediate_hop_param_guard_suppresses_provably_false_chain() {
    // async def handler():
    //     outer(False)
    // def outer(flag: bool):
    //     if flag:
    //         inner()
    // def inner():
    //     time.sleep(1)
    let handler = fn_fact(0, "handler", true, "app.py", 1);
    let mut outer = fn_fact(1, "outer", false, "app.py", 5);
    outer.param_names = vec!["flag".into()];
    let inner = fn_fact(2, "inner", false, "app.py", 10);

    let mut call_outer = edge(
        0,
        CalleeRef::Project(FunctionId(1)),
        "app.py",
        2,
        "outer(False)",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );
    call_outer.positional_literals = vec![Some(LiteralArg::False)];

    let mut call_inner = edge(
        1,
        CalleeRef::Project(FunctionId(2)),
        "app.py",
        7,
        "inner()",
        vec![ConditionTag::AsyncBranch],
    );
    call_inner.param_guards = vec![ParamGuard {
        index: 0,
        inverted: false,
    }];

    let inner_sleep = edge(
        2,
        CalleeRef::External {
            qualname: "time.sleep".into(),
        },
        "app.py",
        11,
        "time.sleep(1)",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );

    let facts = ProjectFacts {
        functions: vec![handler, outer, inner],
        edges: vec![call_outer, call_inner, inner_sleep],
        ..Default::default()
    };

    let report = analyze(&facts, &RegSinks);
    assert!(
        report.findings.is_empty() && report.candidates.is_empty(),
        "outer(False) -> if flag: inner() -> sleep must be dropped, got findings={:?} candidates={:?}",
        report.findings,
        report.candidates
    );
}

#[test]
fn higher_order_callback_binds_to_exact_call_site_not_first_call_in_caller() {
    // async def handler(debug: bool):
    //     apply(safe_fn)             # line 10, unconditional
    //     if debug:
    //         apply(blocking_fn)     # line 20, AsyncBranch
    let handler = fn_fact(0, "handler", true, "app.py", 1);
    let mut apply = fn_fact(1, "apply", false, "app.py", 30);
    apply.param_names = vec!["fn".into()];
    apply.invokes_params = vec![0];
    let safe_fn = fn_fact(2, "safe_fn", false, "app.py", 40);
    let blocking_fn = fn_fact(3, "blocking_fn", false, "app.py", 50);

    let mut call_safe = edge(
        0,
        CalleeRef::Project(FunctionId(1)),
        "app.py",
        10,
        "apply(safe_fn)",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );
    call_safe.positional_callables = vec![Some(FunctionId(2))];

    let mut call_blocking = edge(
        0,
        CalleeRef::Project(FunctionId(1)),
        "app.py",
        20,
        "apply(blocking_fn)",
        vec![ConditionTag::AsyncBranch],
    );
    call_blocking.occurrence = 1;
    call_blocking.positional_callables = vec![Some(FunctionId(3))];

    let invoke = edge(
        1,
        CalleeRef::ParamInvoke { index: 0 },
        "app.py",
        31,
        "fn()",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );

    let sleep = edge(
        3,
        CalleeRef::External {
            qualname: "time.sleep".into(),
        },
        "app.py",
        51,
        "time.sleep(1)",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );

    let facts = ProjectFacts {
        functions: vec![handler, apply, safe_fn, blocking_fn],
        edges: vec![call_safe, call_blocking, invoke, sleep],
        ..Default::default()
    };

    let report = analyze(&facts, &RegSinks);
    assert_eq!(report.findings.len(), 1);
    let f = &report.findings[0];
    assert_eq!(
        f.chain[0].line, 20,
        "first hop must be line 20 apply(blocking_fn), got: {:?}",
        f.chain
    );
    assert_eq!(f.conditions, vec!["async-branch".to_string()]);
}

#[test]
fn distinct_nested_loop_and_offload_sinks_do_not_collide_or_panic() {
    let handler = fn_fact(0, "handler", true, "app.py", 1);
    let mut custom_offload = fn_fact(1, "run_bg", true, "app.py", 40);
    custom_offload.is_offload_wrapper = true;
    let coro = fn_fact(2, "coro", true, "app.py", 50);

    let run1 = edge(
        0,
        CalleeRef::External {
            qualname: "asyncio.run".into(),
        },
        "app.py",
        10,
        "asyncio.run(coro())",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );
    let run2 = edge(
        0,
        CalleeRef::External {
            qualname: "anyio.run".into(),
        },
        "app.py",
        11,
        "anyio.run(coro)",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );

    let mut off1 = edge(
        0,
        CalleeRef::Offload(OffloadKind::AsyncioToThread),
        "app.py",
        12,
        "asyncio.to_thread(coro)",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );
    off1.is_offload_wrapper = true;
    off1.positional_callables = vec![Some(FunctionId(2))];
    off1.callee_is_coroutine = Some(true);

    let mut off2 = edge(
        0,
        CalleeRef::Project(FunctionId(1)),
        "app.py",
        13,
        "run_bg(coro)",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );
    off2.positional_callables = vec![Some(FunctionId(2))];

    let facts = ProjectFacts {
        functions: vec![handler, custom_offload, coro],
        edges: vec![run1, run2, off1, off2],
        ..Default::default()
    };

    let report = analyze(&facts, &RegSinks);
    let nested_lines: Vec<u32> = report
        .findings
        .iter()
        .filter(|f| f.rule == "nested-event-loop")
        .map(|f| f.chain.last().unwrap().line)
        .collect();
    assert_eq!(nested_lines, vec![10, 11]);

    let offload_lines: Vec<u32> = report
        .findings
        .iter()
        .filter(|f| f.rule == "offload-coroutine")
        .map(|f| f.chain.last().unwrap().line)
        .collect();
    assert_eq!(offload_lines, vec![12, 13]);
}

#[test]
fn sync_to_async_decorated_project_function_emits_twin_advice() {
    let handler = fn_fact(0, "handler", true, "app.py", 1);
    let fetch = fn_fact(1, "fetch", false, "app.py", 10);

    let mut call_fetch = edge(
        0,
        CalleeRef::Project(FunctionId(1)),
        "app.py",
        2,
        "await fetch()",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );
    call_fetch.is_awaited = true;

    let http_call = edge(
        1,
        CalleeRef::External {
            qualname: "httpx.get".into(),
        },
        "app.py",
        11,
        "httpx.get(url)",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );

    let mut resolved_symbols = BTreeSet::new();
    resolved_symbols.insert("httpx.AsyncClient".to_string());
    let mut decorator_refs = BTreeMap::new();
    decorator_refs.insert(
        FunctionId(1),
        vec![DecoratorRef::External("asgiref.sync.sync_to_async".into())],
    );

    let facts = ProjectFacts {
        functions: vec![handler, fetch],
        edges: vec![call_fetch, http_call],
        decorator_refs,
        resolved_symbols,
        ..Default::default()
    };

    let report = analyze(&facts, &RegSinks);
    assert!(report.findings.is_empty());
    assert_eq!(
        report.advice.len(),
        1,
        "expected offload-has-async-twin advice on @sync_to_async fetch(), got: {:?}",
        report.advice
    );
    assert_eq!(report.advice[0].rule, "offload-has-async-twin");
}

#[test]
fn production_entry_calling_non_production_async_helper_surfaces_finding() {
    // app/routes.py: async def endpoint() -> await seed_helper()
    // scripts/seed.py: async def seed_helper() -> time.sleep(1)
    let mut endpoint = fn_fact(0, "endpoint", true, "app/routes.py", 1);
    endpoint.label = EntryLabel::Route;
    let seed_helper = fn_fact(1, "seed_helper", true, "scripts/seed.py", 10);

    let mut call_helper = edge(
        0,
        CalleeRef::Project(FunctionId(1)),
        "app/routes.py",
        2,
        "await seed_helper()",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );
    call_helper.is_awaited = true;

    let sleep = edge(
        1,
        CalleeRef::External {
            qualname: "time.sleep".into(),
        },
        "scripts/seed.py",
        11,
        "time.sleep(1)",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );

    let facts = ProjectFacts {
        functions: vec![endpoint, seed_helper],
        edges: vec![call_helper, sleep],
        ..Default::default()
    };

    let report = analyze(&facts, &RegSinks);
    assert_eq!(
        report.findings.len(),
        1,
        "production route awaiting non-production async helper must surface: {:?}",
        report
    );
    assert_eq!(report.findings[0].entry.name, "endpoint");
    assert_eq!(report.findings[0].chain.len(), 2);
}

#[test]
fn absolute_host_non_production_ancestors_do_not_hide_production_filename() {
    assert!(
        !async_doctor_core::is_non_production_entry_path("/Users/dev/benchmarks/prod_app/app.py"),
        "absolute host directory `benchmarks` must not classify `app.py` as non-production"
    );
    assert!(
        !async_doctor_core::is_non_production_entry_path("/tmp/.hidden_host/prod_app/routes.py"),
        "absolute host directory `.hidden_host` must not classify `routes.py` as non-production"
    );
    assert!(
        async_doctor_core::is_non_production_entry_path("/Users/dev/prod_app/test_routes.py"),
        "test_*.py filename must still classify as non-production even when path is absolute"
    );
    assert!(
        async_doctor_core::is_non_production_entry_path("benchmarks/bench_api.py"),
        "relative benchmarks/ directory inside project must remain non-production"
    );
}

#[test]
fn intermediate_first_party_param_guard_unknown_arg_stays_finding() {
    // async def route(model: str):
    //     get_rf(model)        # runtime non-literal -> Finding (async-branch)
    //     get_rf(None)         # provably false -> dropped
    // def get_rf(model: str | None):
    //     if model:
    //         load_colbert()
    // def load_colbert():
    //     time.sleep(1)
    let mut route = fn_fact(0, "route", true, "app/routes.py", 1);
    route.param_names = vec!["model".into()];
    let mut get_rf = fn_fact(1, "get_rf", false, "app/retrieval.py", 10);
    get_rf.param_names = vec!["model".into()];
    let load_colbert = fn_fact(2, "load_colbert", false, "app/colbert.py", 20);

    let mut call_runtime = edge(
        0,
        CalleeRef::Project(FunctionId(1)),
        "app/routes.py",
        2,
        "get_rf(model)",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );
    call_runtime.positional_literals = vec![None];

    let mut call_none = edge(
        0,
        CalleeRef::Project(FunctionId(1)),
        "app/routes.py",
        3,
        "get_rf(None)",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );
    call_none.occurrence = 1;
    call_none.positional_literals = vec![Some(LiteralArg::None)];

    let mut call_colbert = edge(
        1,
        CalleeRef::Project(FunctionId(2)),
        "app/retrieval.py",
        12,
        "load_colbert()",
        vec![ConditionTag::AsyncBranch],
    );
    call_colbert.param_guards = vec![ParamGuard {
        index: 0,
        inverted: false,
    }];

    let sleep = edge(
        2,
        CalleeRef::External {
            qualname: "time.sleep".into(),
        },
        "app/colbert.py",
        22,
        "time.sleep(1)",
        vec![ConditionTag::Unconditional, ConditionTag::AllPaths],
    );

    let facts = ProjectFacts {
        functions: vec![route, get_rf, load_colbert],
        edges: vec![call_runtime, call_none, call_colbert, sleep],
        ..Default::default()
    };

    let report = analyze(&facts, &RegSinks);
    assert_eq!(
        report.findings.len(),
        1,
        "first-party intermediate param_guard with runtime arg must stay a Finding: {:?}",
        report
    );
    assert_eq!(
        report.candidates.len(),
        0,
        "get_rf(None) must be dropped and get_rf(model) must not demote to Candidate: {:?}",
        report.candidates
    );
    assert_eq!(report.findings[0].chain[0].line, 2);
}
