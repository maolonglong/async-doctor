# Preserve Python execution semantics in diagnostics

## Purpose

Make async-doctor distinguish coroutine objects from async generators and async
protocols, preserve blocking work in offload wrappers, and follow provable
callback, generator-consumption, and context-manager execution. Strengthen the
benchmark and corpus comparison tools so invalid or incomplete scans cannot
masquerade as clean results. Keep the zero-config interface and existing caches.

## Progress

- [x] 2026-11-14 12:00 UTC: Reproduced and reviewed the six analysis gaps and benchmark validation gaps in the preceding assessment; current tracked worktree starts clean.
- [x] 2026-11-14 12:00 UTC: Saved the unchanged source and debug binary; started building an isolated-source release baseline before implementation.
- [x] 2026-11-14: Added five public scan contracts; all failed on unchanged analysis. Extended existing offload and package-cache regression controls.
- [x] 2026-11-14: Corrected coroutine/generator classification and protocol repair guidance, preserving existing async-generator diagnostics.
- [x] 2026-11-14: Retained wrapper-local execution, followed uniquely proven local generator origins at consumption, and tracked awaited worker exhaustion.
- [x] 2026-11-14: Resolved project context methods through ty's MRO and typed asyncio callbacks with explicit argument/partial bindings.
- [x] 2026-11-14: Added strict benchmark/audit validation, 42 Python script tests, and the lightweight CI step.
- [x] 2026-11-14 12:48 UTC: Workspace tests and nextest each passed 436 tests (four existing ignored); fmt, clippy, length, Python runtime controls, three pinned public corpora, 13 fixture groups, and cold/warm parity passed or had fully reviewed intended deltas.
- [x] 2026-11-14 12:48 UTC: Updated README, rules, architecture, benchmark/validation limits, and quality/debt docs; reviewed the combined diff and closed the plan. Work remains local and uncommitted.

## Surprises & Discoveries

- The existing async-generator negative control covers a lambda but not the
  equivalent direct callable. All 431 existing tests passed during assessment.
- A coroutine yielding a normal list must be awaited before ordinary iteration;
  changing `for` to `async for` still fails at runtime. The same distinction
  applies to a coroutine returning a synchronous context manager.
- Offloading generator creation does not offload later iteration. Runtime
  thread-identity probes confirmed that distinction and loop callback execution.
- `definitions_for_name` is IDE navigation and includes all scope bindings,
  not reaching definitions. Aliases surviving rebinding need `use_def_map` and
  a unique binding in the current function/lambda; scan tests cover this.
- Forwarded callback arguments require plain caller-parameter provenance to
  preserve `False` guards through lambdas and bound methods. The proof follows
  only earlier edges on the concrete path and stops on unknown/rebound values.
- Async-generator functions still need `sync-for-async-iterable`; narrowing
  every `async def` check to coroutines initially regressed an existing test.
  Iteration and eager-offload validation now ask different questions.
- The pinned default editable installs omit optional modules. Each public
  corpus retains one location-less import candidate; `complete=true` and a
  successful install are not dependency-health proofs.

## Decision log

- 2026-11-14: Implement the confirmed correctness and validation defects first.
  Defer speculative new advice and version-sensitive rules until the execution
  model is trustworthy; no cache redesign or claimed speedup.
- 2026-11-14: Split only independent work: one subagent owns benchmark/audit
  scripts, their Python tests, and the CI test step; the main agent owns Rust,
  documentation, integration, and combined verification.
- 2026-11-14: Preserve permissive human-readable audit comparison and add an
  explicit strict mode rather than rejecting every intentional diagnostic delta
  in the existing interface.
- 2026-11-14: Keep named-generator inference local and conservative; do not
  guess arbitrary consumers, global/nonlocal origins, or local partial aliases
  whose saved metadata loses bound arguments. Remaining coverage is tracked
  in `docs/quality.md`.
- 2026-11-14: Bump the package-cache schema from 24 to 25 for new execution
  facts and `param:N` bindings. Keep the build-keyed whole-scan cache layout
  (`scan-cache-v8`) and public report schema 2 unchanged.

## Context

The baseline is the current `main` checkout. Analysis facts are plain Rust in
`crates/async_doctor_core/src/graph.rs`; coroutine/protocol findings are seeded
in `propagate/special.rs`, and wrapper binding lives in
`propagate/higher_order.rs`. Python-specific resolution belongs to
`crates/async_doctor_ty/src/extract/collector/`, particularly `resolve.rs`,
`async_anti_patterns.rs`, `calls.rs`, `locals.rs`, and `control_flow/`.

Tests exercise `scan_path*` in `crates/async_doctor_ty/src/tests/`. The benchmark
is `bench/run.sh` plus `bench/harness.py`; `scripts/audit_compare.py` compares
corpus reports. The baseline is commit `37b664c`; its source and binaries were
used temporarily under `.amp/in/optimization/`, never as tracked code. Audit
logs and report evidence are separate from the disposable build/probe files.

## Plan of work

1. Extend scan-level tests with asymmetric positive/negative controls for the
   six defects. Run the targeted tests before changing production analysis.
2. Correct coroutine versus generator and protocol classification, including
   repair hints. Run the offload and async-protocol test areas, and execute
   representative original/fixed Python snippets.
3. Preserve wrapper execution while cutting only worker targets; track local
   delayed generator sources; resolve project implicit methods and typed
   asyncio callbacks. Verify offloaded and unresolved controls remain clean.
4. Add fail-closed report validation and opt-in strict benchmark/audit modes,
   tested through script entry points. Run the Python tests and shell syntax
   checks with the combined workspace suite.
5. Compare baseline/candidate release scans on repository fixtures and pinned
   HTTPX, Starlette, and FastAPI corpora with isolated caches. Explain every
   added/removed finding and require cold/warm byte equality. The two heavier
   benchmark corpora are outside this iteration's performance claims.

## Validation

- Targeted Rust scan tests fail before and pass after each relevant correction.
- `cargo test --workspace` and `cargo nextest run --workspace` pass, excluding
  the existing ignored stress tests; `cargo fmt --all -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` pass.
- `python3 -m unittest discover -s scripts/tests` passes for script contracts.
- `python3 scripts/check_docs.py` and
  `./scripts/check_rust_file_length.sh` pass.
- Runtime controls distinguish valid async-generator offload, awaited list/CM
  values, loop callbacks, and lazy generator consumption without using timing
  thresholds as correctness assertions.
- Release corpus reports are valid and complete. Baseline/candidate deltas
  receive an explicit explanation and each corpus's cold/warm reports match.

## Outcomes & Retrospective

All six confirmed execution-model defects and fail-open benchmark/audit
validation are addressed without a new scanned-project config, rule ID, public
JSON version, or cache redesign. The five new scan tests cover both blocking
paths and controls: actual worker execution, deferred coroutine/generator
bodies, false arguments, rebinding, MRO, and unrelated same-name methods.

Release audit against the same pinned sources and environments:

- HTTPX: 0 findings before/after; Starlette: 2 before/after; FastAPI: 0
  before/after. No finding/candidate/location-less/advice payload changed.
- Twelve existing fixture groups were unchanged. The new execution fixture
  went from 6 to 38 findings: 30 newly followed blocking paths, two new
  coroutine protocol errors, and three wrong protocol-rewrite findings replaced
  by `unawaited-coroutine`. All three removals have that explicit replacement;
  no other removals, renumberings, or modified IDs occurred.
- Strict comparison intentionally returned 1 for those reviewed fixture
  differences, rather than silently accepting them. It returned 0 for the
  public corpora. Candidate cold/warm reports matched byte-for-byte for all
  three public corpora and the new execution fixture; the actual strict bench
  runner also passed the three-corpus subset with dependency preparation.
- Python 3.11 runtime controls confirmed awaited list/context values, valid
  async-generator offload, scheduling methods and Future callbacks on the loop,
  worker-side generator creation, and exhaustive/partial consumption behavior.
- Workspace/nextest: 436 passed, four pre-existing stress tests skipped.
  Script regressions: 42 passed. No production tests were weakened to get green.

Limits: no large `litellm` / `open-webui` re-audit, full Python-version matrix,
or performance improvement claim. Strict tooling validates report envelopes
and recorded scan status, not the full JSON schema or dependency health.
Changes remain local until the user asks for a commit or push; no remote
workflow, merge, package publication, or release was performed.
