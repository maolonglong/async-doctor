# Keep Future completion proofs tied to their receiver and execution

## Purpose

Avoid reporting a completed concurrent Future as a blocking wait when local
control flow proves completion, without hiding a pending Future or a custom
method that happens to be named `result`. Keep diagnosis independent of repair;
do not prescribe `await`, offloading, or an upstream rewrite.

## Progress

- [x] 2026-11-14T15:46:00Z: Committed the previous environment/evaluation work
  locally as `c2595fc`; the worktree was clean. No push or release.
- [x] 2026-11-14T15:46:00Z: Read collector ownership, existing structural guards,
  public scan tests, cache semantics, and Python's documented Future contract.
- [x] 2026-11-14T15:46:00Z: Saved a release baseline and reproduced eight local
  cases: five hidden blocking paths and three false blocking reports.
- [x] 2026-11-14T16:00:00Z: Extended the existing public scan regression and
  corrected actual method identity, function-local proof lifetime, explicit
  receiver writes, and live-branch intersections. Loop, exception, pattern,
  comprehension, and inline-lambda counterexamples were red before their fixes
  and pass together; custom Future overrides remain reachable.
- [x] 2026-11-14T16:00:00Z: Incremented the package-cache semantic version to 26
  and documented local proof ownership and limits in rules/architecture/quality.
- [x] 2026-11-14T16:07:16Z: Validated 441 workspace tests, 53 script tests,
  52 bounded runtime cases, fmt, Clippy, docs, file lengths, and whitespace.
- [x] 2026-11-14T16:07:16Z: Reviewed release deltas and real cold/warm hits on
  both 24-job manifests. All changes are in the expanded Future fixture;
  publish selected evidence and update the running review during closeout.

## Surprises & Discoveries

The existing `.done()` exemption runs before project-call resolution and checks
only a textual receiver path. A custom client's `done()` therefore suppresses
its `result()` body even when that body calls `time.sleep`.

The initial public-CLI probe reports only `deferred_local_keeps_outer_proof`,
`negative_guard_is_complete`, and `complete_in_else` (all three are safe at their
reported result site). It misses custom blocking bodies, a nested function's
pending parameter, attribute replacement, a rebind in a nested branch, and a
walrus rebind. All scans are complete, with no unresolved candidates.

The first implementation still leaked body-only proofs across zero-iteration
loops, exception paths, match siblings, and suppressing context managers.
Additional public regressions exposed those errors. Failed match guards retain
capture bindings, imports can replace locals, comprehension targets do not
replace outer locals, and annotation-only statements do not assign a value.
Directly called lambdas need call-site capture proofs, unlike deferred bodies;
clearing all lambda proofs created a reproducible false positive.

The archived audit has two manifests: the primary one supplies synthetic
dependencies to advice fixtures; the local-environment variant intentionally
omits them for three fixtures. Their advice/candidate totals differ. Both
variants were replayed with identical baseline/candidate inputs and fresh
caches; their totals must not be compared across environments.

The previously measured Starlette lifecycle sites need a different proof:
cross-coroutine channel ordering plus portal ownership. A local `.done()`
improvement cannot establish that ordering. Retain the known limitation rather
than special-casing Starlette or downgrading all Future sinks.

## Decision log

- 2026-11-14: Limit this change to local completion guards and their invalidation.
  Resolve actual concurrent Future methods, isolate deferred function bodies,
  invalidate explicit receiver writes, and retain only proofs shared by live
  continuations. These are generic Python semantics, not library-name heuristics.
- 2026-11-14: Preserve JSON schema 2 and the whole-scan cache layout. Increment
  the package-cache semantic version when extraction changes, so old cached
  missing/extra edges cannot survive the correction.
- 2026-11-14: Keep the guard implementation local and structural. Do not claim
  general alias/heap or arbitrary-call mutation tracking; no cross-coroutine
  ordering inference, Starlette special case, sink downgrade, or repair hint.
- 2026-11-14: A suite ending in a direct `return`/`raise` has no continuation
  even when preceded by other statements. Reuse this existing early-exit seam
  rather than adding a separate Future-only control-flow classifier.

## Context

At baseline, `thread_guards.rs` finds textual `.done()` tests and `buffers.rs`
skips matching `.result()` calls. The correction moves Future-specific proof
logic into `crates/async_doctor_ty/src/extract/collector/future_guards.rs`;
`control_flow.rs`, `visit.rs`, and `locals.rs` manage its lifetime. Public
regressions live in `src/tests/guards_and_archives.rs` and
`fixtures/off_loop_thread/app.py`. Only ty owns semantic identities; no ty/ruff
types should cross into core or rules.

The baseline is local commit `c2595fc403e7b5f44aa2125623bebedee9b5b48d`.
Temporary probes, copied binaries, caches, and raw logs stay under
`.amp/in/future-completion/`; publish only selected review evidence.

## Plan of work

First extend the existing public scan regression with asymmetric safe and
blocking cases. Confirm failure before editing the implementation. Then keep
Future-specific proof logic together within the collector, without changing
the sink table or adding repair guidance. Finally compare release binaries on
the existing 24-job fixed corpus, review every diagnostic delta, and replay
cold/warm scans with isolated caches and real exit codes.

## Validation

- `cargo test -p async_doctor_ty p1_off_loop_thread_and_done_guards`: fails on
  the unmodified extraction and passes after the correction, including retained
  pending-Future controls and unchanged off-loop cases.
- `cargo test --workspace`: 441 passed, zero failed, four existing stress tests
  ignored. Python script unittests: 53 passed. fmt, Clippy with `-D warnings`,
  Rust file-length, docs (50 documents), and diff checks pass.
- The unchanged fixture executes in 52 bounded Python 3.11.6 runtime cases:
  25 pending controls enter real `Condition.wait`, four custom-method controls
  execute `time.sleep`, and 23 nonblocking controls do neither. The probe only
  caps Future result timeouts at 20 ms; it does not replace result/state logic.
- Both 24-job release comparisons use per-job isolated `XDG_CACHE_HOME` and a
  2400-second timeout. Exactly 12 true blocking reports are restored and 13
  completed-Future false reports removed, with no renumbering or modified kept
  diagnostics. All 23 other reports are byte-identical. Strict diff exits 1 for
  these reviewed changes; the baseline and gate were not weakened.
- Primary totals: findings 119 → 118, advice 31 → 31, location-less candidates
  3 → 3. Local-environment variant: findings 119 → 118, advice 25 → 25,
  location-less candidates 4 → 4. Location-bearing candidates stay zero.
- All 24 primary candidate JSONs and all 24 local-environment JSONs match their
  cold and warm replays byte-for-byte, with actual cold misses and warm hits.
  HTTPX/Starlette/FastAPI keep 0/2/0 findings; unresolved optional/version-gated
  modules remain 10/8/7 in these reused benchmark environments. Preparation was
  not rerun, and completeness is not a dependency-health guarantee.

## Outcomes & Retrospective

The local proof correction and its integration verification are complete.
The expanded public fixture covers 29 blocking entries and 13 safe entries;
runtime probes independently exercise both directions and completed exceptions
and cancellation. The shared early-exit change has no unrelated corpus delta.

Package-cache schema 26 invalidates stale extracted edges; JSON schema 2 and
the `scan-cache-v8` layout are unchanged. The two coordination-based Starlette
findings remain a documented limitation, not a claimed fix. No new repair
instructions, Agent evaluations, larger-corpus benchmarks, or Python version
matrix were added. Selected source, raw outputs, and checks are archived before
cleaning temporary probes and binaries. The previous commit is local only;
this new work has not been committed or pushed.
