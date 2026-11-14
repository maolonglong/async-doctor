# Distinguish Future waits from immediate polling

## Purpose

Report blocking `concurrent.futures.Future.exception()` calls as well as
`result()`, without reporting literal nonpositive-timeout polls as waits.
Apply the existing typed completion proofs to both methods. Preserve custom
method bodies, eager receiver evaluation, and nonblocking asyncio Futures;
do not add repair instructions or claim cross-coroutine completion analysis.

## Progress

- [x] 2026-11-14T16:16:00Z: Committed the previous local proof correction as
  `b9255af`; verified all 17 source hashes and saved its release baseline.
- [x] 2026-11-14T16:16:00Z: Read the collector/rules seam, Python 3.11's public
  Future contract, and the installed CPython Future/Condition implementation.
- [x] 2026-11-14T16:18:00Z: Reproduced missing exception waits and false polling
  reports through the public scan interface. The expanded test failed before
  implementation and passed after the initial bound-method correction.
- [x] 2026-11-14T16:25:00Z: A second red test exposed unbound-method polling and
  completion reads being misclassified. Reused existing call-binding and AST
  argument lookup rather than keeping an instance-only argument parser.
- [x] 2026-11-14T16:25:00Z: Updated the existing sink catalogue and explanations,
  generalized typed Future guards, and advanced package-cache semantics to 27.
- [x] 2026-11-14T16:27:00Z: Workspace checks passed: 441 Rust tests, 53 script
  tests, fmt, Clippy, Rust file-length, docs, and whitespace checks. All 30
  bounded runtime cases and 138 public CLI signature controls passed.
- [x] 2026-11-14T16:31:00Z: Reviewed all eight added waits and six removed polls
  in the 24-job release audit. The other 23 complete reports are byte-identical;
  all 24 jobs independently reproduced cold misses, warm hits, and identical
  JSON. Strict comparison correctly returned 1 for the reviewed differences.

## Surprises & Discoveries

Both CPython methods call `Condition.wait(timeout)` while pending. For a
nonpositive timeout, that condition attempts its waiter lock with
`acquire(False)`, rather than waiting for completion. At baseline the catalogue
listed only `result`, and neither Future method had a polling exemption. The
first public regression confirms six missing exception-wait entries and five
false result-poll reports. Six direct Python controls execute both real methods
with zero, negative, and positive timeouts. Contract:
[Python Future methods](https://docs.python.org/3.11/library/concurrent.futures.html#future-objects).

Unbound calls such as `Future.exception(self=future, timeout=0)` need a different
argument position and the actual `self` receiver for completion proofs. A
second public red test confirms the problem. The existing external-call
binding helper and Ruff's `find_argument_value` already own those contracts;
using them also avoids assuming a positional argument after `*args`.

The two Starlette lifecycle sites require reasoning about channel publication,
portal ownership, and the absence of a later scheduling checkpoint. The
collector has local call/control-flow facts, not that cross-coroutine ordering
model. Do not add a library-name exemption or downgrade all Future sinks.

## Decision log

- 2026-11-14: Keep this correction at the existing Future proof seam and builtin
  sink table. Literal timeout polling is specific to the Future protocol;
  do not reuse negative-timeout semantics for locks, where `-1` can wait forever.
- 2026-11-14: Reuse `blocking-lock`, empty repair hints, schema 2, and the
  whole-scan layout. Invalidate cached extraction facts when semantics change.
- 2026-11-14: Leave Starlette's coordination-based precision limit explicit.
  A generic local correction is useful independently and does not solve it.

## Context

`crates/async_doctor_ty/src/extract/collector/future_guards.rs` owns typed
completion proofs; `calls.rs` applies them before project resolution, and
`resolve/external.rs` identifies inherited stdlib Future methods. The runtime
sink table is `crates/async_doctor_rules/src/stdlib/sinks/runtime.rs`.
The existing public scan regression and `fixtures/off_loop_thread/app.py`
provide a shared test surface. Rules are documented in `docs/rules.md` and
`crates/async_doctor_rules/src/docs.rs`.

Baseline source is local commit `b9255afab792d9145340ec7b27ee1dcee7445f2b`.
The saved release baseline has SHA-256
`38ad1a0abf859ca62248cd256f4c0492ddb35bf23b0d9419765b7a03865b217b`.
Temporary probes and evidence stay under `.amp/in/future-coordination/`,
outside scanned roots. Export selected results before removing scratch files.

## Plan of work

First extend the existing fixture and scan test, keeping safe cases and
positive-timeout, rebinding, custom-method, and eager-evaluation controls.
Record failures before changing implementation. Then generalize only the
actual Future wait methods and recognize direct signed numeric timeout
literals. Dynamic timeouts remain unproven. Finish with runtime checks and
release comparisons on identical sources/environments and isolated caches.

## Validation

- `cargo test -p async_doctor_ty p1_off_loop_thread_and_done_guards` fails
  before implementation, then passes with the expected blocking entry set.
- `cargo test --workspace`, script unittests, fmt, Clippy `-D warnings`, docs,
  Rust file-length, and whitespace checks pass.
- A bounded Python 3.11 probe executes both stdlib methods, completed success,
  failure, and cancellation, nonpositive polls, pending waits, and custom bodies.
  It must assert returned values/exceptions, not only lack of hangs.
- The existing primary 24-job manifest uses fixed public pins and matching
  fixture environments. Review additions/removals and require actual cold
  misses followed by byte-identical warm hits. Do not weaken strict diff.

## Outcomes & Retrospective

The existing public scan regression covers 24 added async entries: 11 blocking
entries and 13 safe controls. All expected diagnostic sets pass. The rules
retain empty repair hints, public schema 2, and the whole-scan cache layout.
Only the package-cache version changes, from 26 to 27.

The release baseline and candidate scan identical current fixture sources and
fixed HTTPX, Starlette, and FastAPI checkouts, with the same existing venvs and
isolated caches. Findings total 127 to 129: eight exception waits are added and
six immediate result polls removed, all in the extended Future fixture. No
retained diagnostic changes content or identity. The other 23 complete reports,
31 advice entries, and three location-less candidates are unchanged. All 24
candidate reports match independent cold and warm runs byte-for-byte.

Runtime validation executes the actual fixture and standard-library methods on
Python 3.11.6. Thirty cases assert return/exception semantics and observe nine
real pending condition waits, twelve immediate poll condition calls, and two
loop-thread sleeps. The separate 138-case CLI matrix checks binding and signed
literal variants; it is static validation, not 138 additional runtime cases.
The final workspace has 441 passing Rust tests and four existing stress tests
ignored; all 53 script tests and format/lint/docs checks pass.

Starlette's two coordination-based findings remain unchanged. The public
project venvs still have 10/8/7 unresolved optional or version-conditional
modules respectively; completeness does not prove full dependency health.
Nonpositive Future timeouts do not wait for completion, but internal mutex
contention is not modeled. No performance, all-version, or scheduling claim is
made. There are no upstream edits, suppressions, additional Agent trials, or
new repair instructions.

Selected raw reports, actual exit codes, red/green logs, replay probes, final
sources, and hashes are exported under `.amp/in/artifacts/` before scratch
cleanup. At verification closeout, this correction is uncommitted; only the
preceding completed Future proof change was committed as requested. Nothing
has been pushed or released. The prior completed plan remains historical and
is not rewritten.
