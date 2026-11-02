# Release-review analyzer fixes (M1–M2)

## Purpose

Post-release review found correctness false positives and output-scaling pain
points. After this lands, decorator-based offloads (`sync_to_async`,
`database_sync_to_async`, and project offload decorators) cut call edges,
`functools.cached_property` getters are followed, non-IO client methods no
longer report, follow-budget truncation is visible in output, `--max-findings`
bounds large JSON payloads, `--changed` filters at hunk granularity, and text
group headers show canonical sink qualnames. See `docs/cli.md` and
`docs/json-contract.md` for the resulting contracts.

## Progress

- [x] 2026-11-02: Decorator offload detection — ty-resolved `asgiref.sync.sync_to_async` and `channels.db.database_sync_to_async` (bare and called forms), plus project decorators whose returned nested def forwards the wrapped callable as arg 0 into an offload primitive or inferred wrapper (`OffloadKind::ChannelsDbSyncToAsync`, `DecoratorRef`, `FunctionFact.offload_decorator`, fixpoint in `propagate/higher_order.rs`). Regression: `tests::offload::offload_decorators_cut`.
- [x] 2026-11-02: `functools.cached_property` followed like `@property` (`decorator_is_property`). Regression: `tests::recall::cached_property_getter_is_followed`.
- [x] 2026-11-02: Non-IO sink exclusions — `requests.Session.{mount,get_adapter}`, `httpx.Client.{build_request,mount}`, `pymysql.{err,constants,converters}.*`, kafka consumer local-state methods, `openai`/`anthropic` `{with_options,copy}`. Regression: `third_party::tests::non_io_client_methods_never_fire`, `m2_non_io_client_and_namespace_exclusions`.
- [x] 2026-11-02: `environment.followBudgetSkipped` — counts budget-truncated third-party follow candidates (JSON, omitted when 0; text note). Regression: `tests::budget::follow_budget_skipped_reported_and_identical_cold_warm`.
- [x] 2026-11-02: `--max-findings N` output cap — round-robin over `sinkSites` for findings, first-N for candidates/advice, `omitted*` JSON fields, text/markdown footers, SARIF honors it, `--write-baseline` unaffected. Regression: `cli_max_findings` (2 tests).
- [x] 2026-11-02: Hunk-level `--changed` — NEW-side line ranges from `git diff -U0` (base, unstaged, cached), untracked = whole file, pure deletion marks `s`/`s+1`, parse failure = whole file. Regressions: `changed_hunk_*` (5 tests).
- [x] 2026-11-02: Grouped text/markdown headers list canonical `hit.sink_qualname` (was call text). Regression: `cli_group_headers`.
- [x] 2026-11-02: Docs — SKILL/README PyPI-live wording, benchmark numbers, json-contract/cli updates.

## Surprises & Discoveries

- `wrap_app_handling_exceptions` in starlette was briefly mis-marked an offload
  wrapper: its nested def forwards param `conn` as a *data* arg of
  `run_in_threadpool(handler, conn, exc)`. Deferred decorator-wrapper
  candidates now only record the param at arg index 0 (the callable slot).
- `followBudgetSkipped` read 0 on warm runs despite correct extraction: the
  whole-scan cache hit rebuilds `environment` and dropped the new field.
- Two `[blocking-db] sqlite3.connect` groups on the probe were legitimately
  distinct (`sqlite3.connect` vs `sqlite3.Connection.execute`); only the
  call-text titles collided.

## Decision log

- Decision: decorator offload scope is ty-resolved `asgiref.sync.sync_to_async`/`channels.db.database_sync_to_async` plus inferred project offload decorators; unknown decorators stay transparent; inferred wrapper forwarding gated to arg 0 / Rationale: FP correctness; name-matching would mis-cut `tenacity`/`lru_cache` and the starlette data-arg case / Date: 2026-11-02.
- Decision: budget truncation is a count on `environment`, not `complete=false` / Rationale: extraction still finished; honesty without changing exit semantics / Date: 2026-11-02.
- Decision: `--max-findings` is output-only; score/exit/sinkSites/`--write-baseline` keep the full report / Rationale: a display cap must not change grading / Date: 2026-11-02.
- Decision: `--changed` is hunk-level; edits that change resolution without touching a chain line (type annotations, pure renames) are not reported / Rationale: stated limitation; full scan or `--baseline` before merge / Date: 2026-11-02.

## Context

Touch points: `crates/async_doctor_core/src/{graph,report,environment}.rs`,
`propagate/higher_order.rs`, `crates/async_doctor_ty/src/extract/{follow,collector/*}.rs`,
`cache.rs` (schema v18), `scan.rs`, `sitepkgs/batch.rs`,
`crates/async_doctor_rules/src/third_party/families.rs`,
`crates/async_doctor/src/{args,changed,main}.rs`, `text/{mod,grouped}.rs`,
`markdown.rs`, `sarif.rs`. Benchmarks: `bench/.repos/*` pinned corpora.

## Plan of work

M1: decorator offloads, prefix-sink exclusions, cached_property.
M2: follow-budget signal, `--max-findings`, hunk-level `--changed`,
canonical group headers. M3: docs + release bookkeeping.

## Validation

`cargo test --workspace` green; `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings`,
`scripts/check_rust_file_length.sh`, `scripts/check_docs.py` all pass. Corpus:
cold/warm byte-identical on all 5 bench repos; litellm audit vs M1 baseline
shows only the expected `build_request`/`session.mount` removals.
`--max-findings 50` shrinks litellm JSON to ~445 KB.

## Outcomes & Retrospective

All M1–M3 items shipped. Follow-ups: edit-loop perf (litellm warm 0.33s → 8.7s
after a one-line edit — needs a new decision vs
[../../decisions/2026-11-01-reject-incremental-file-cache.md](../../decisions/2026-11-01-reject-incremental-file-cache.md)),
base-class virtual dispatch is a silent miss, no Django ORM sink family,
starlette testclient `.result()` after lifespan end is done-implied, and the
arg-0-only gate misses wrappers whose callable sits later
(`def w(a, fn): to_thread(fn)`).
