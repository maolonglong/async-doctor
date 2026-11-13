# Expand async-twin advice and add high-confidence async anti-pattern rules

## Purpose

Extend `async-doctor` beyond sync-IO reachability with five zero-config, zero-false-positive rules covering the highest-leverage non-blocking async bugs and performance/concurrency anti-patterns:
1. Expand `offload-has-async-twin` (`advice: performance`) to cover `grpc.aio`, `sqlalchemy.ext.asyncio.AsyncSession` / `AsyncEngine` (when `greenlet` and an async DB driver resolve in the scanned venv), `psycopg.connect`, `redis.Lock`, and `httpx._client.Client.`.
2. Add `sync-for-async-iterable` (`findings[]`) when a synchronous `for` loop or comprehension iterates over an async-only iterable (`__aiter__` without `__iter__`/`__getitem__`, async generator call, or known async-only stream).
3. Add `eager-offload-call` (`findings[]`) when an offload boundary (`asyncio.to_thread`, `run_in_executor`, `anyio.to_thread.run_sync`, `run_in_threadpool`, `sync_to_async`) receives an eagerly evaluated call expression `fn(...)` whose return value is provably non-callable.
4. Add `unawaited-coroutine` (`findings[]`) when a bare expression statement calls a proven coroutine-returning function and immediately discards the coroutine without `await`, `create_task`, `gather`, or assignment.
5. Add `ephemeral-async-client` (`advice: performance`) for per-request (`Route`) or per-loop instantiation-and-teardown of `httpx.AsyncClient` / `aiohttp.ClientSession`, and `shared-async-session` (`advice: correctness`) when a single non-concurrency-safe `AsyncSession` / `AsyncConnection` / `asyncpg.Connection` / `psycopg.AsyncConnection` is shared across concurrent tasks in `asyncio.gather` or `TaskGroup`.

## Progress

- [x] (2026-11-13 04:15Z) Milestone 1: Expanded `ASYNC_TWINS` (`any_needs` support in `AsyncTwinHit` / `AsyncTwinEntry`, `grpc.aio`, `sqlalchemy.ext.asyncio.AsyncSession` / `AsyncEngine`, `psycopg.connect`, `redis.Lock.`, `httpx._client.Client.`) + unit tests.
- [x] (2026-11-13 04:28Z) Milestone 2: Deterministic error rules (`sync-for-async-iterable`, `eager-offload-call`, `unawaited-coroutine`) across `async_doctor_rules`, `async_doctor_ty` (`async_anti_patterns.rs`, `eager_offload.rs`), and `async_doctor_core` (`special.rs`, `report.rs`) + deduplication against inner `blocking-*` findings.
- [x] (2026-11-13 04:36Z) Milestone 3: Lifecycle & concurrency advice rules (`ephemeral-async-client`, `shared-async-session` in `concurrency_anti_patterns.rs` and `anti_pattern_advice.rs`), `CACHE_SCHEMA_VERSION` bump (`21` -> `22`), rule docs (`docs/rules.md`, `crates/async_doctor_rules/src/docs.rs`, `ARCHITECTURE.md`, `skills/async-doctor/SKILL.md`), and 10 CLI E2E tests in `crates/async_doctor/tests/cli_anti_patterns.rs`.
- [x] (2026-11-13 04:46Z) Milestone 4: Full workspace test suite, file-length & doc checks, release corpus audit (`./bench/run.sh`: `identical=yes` across all 5 corpora; 0 false positives in `findings[]`; +61 genuine `advice[]` hits on `open-webui`), and plan completion.

## Surprises & Discoveries

- **Intermediate `async def` helpers between `Route` and `AsyncClient()`/`ClientSession()`**: Standard `shortest_paths_to_async_entries` stops at the nearest production `async def` (`EntryLabel::Other`), which hid transitive per-request client creation such as `chat_completion (Route) -> process_pipeline_inlet_filter (async_def) -> aiohttp.ClientSession(...)` in `open-webui` (`open_webui/routers/pipelines.py:75`). Adding `shortest_paths_to_route_or_loop_entries` in `crates/async_doctor_core/src/propagate/anti_pattern_advice.rs` walked past `EntryLabel::Other` helpers to `EntryLabel::Route` / `ConditionTag::Loop` callers while still rejecting `EntryLabel::Lifecycle` and one-shot CLI `main()` scripts.
- **Real-world corpus precision (`open-webui` & `litellm`)**:
  - Across all 5 pinned corpora (`litellm`, `open-webui`, `httpx`, `starlette`, `fastapi` — 3,410 Python files), the new `findings[]` rules produced **0 false positives**.
  - On `open-webui`, `ephemeral-async-client` surfaced **59 genuine per-request `aiohttp.ClientSession` sites** across `open_webui/main.py` and `open_webui/routers/*.py`, and `offload-has-async-twin` surfaced **2 genuine async-twin opportunities**: `asyncio.to_thread(_sync_db_ping)` -> `sqlalchemy.ext.asyncio.AsyncEngine` (`open_webui/main.py:3084`) and `asyncio.to_thread(_search)` -> `psycopg.AsyncConnection` (`open_webui/retrieval/external.py:283`).
  - On `litellm`, `admin_mcp.py:274` (`httpx2.AsyncClient(transport=httpx2.ASGITransport(...))`), `proxy_server.py:1458` (`ClientSession(connector=connector, ...)`), and `presidio.py:391` (`self._http_session = aiohttp.ClientSession()`) were all cleanly excluded without a single false-alarm `ephemeral-async-client` item.
- **File-length discipline**: Adding `eager-offload-call` directly to `async_anti_patterns.rs` pushed it over the 800-line ceiling; splitting `eager_offload.rs` and `concurrency_anti_patterns.rs` (on the `ty` side) and `anti_pattern_advice.rs` (on the `core` side) kept every `.rs` file well under 800 lines.

## Decision log

- **Decision**: Place `sync-for-async-iterable`, `eager-offload-call`, and `unawaited-coroutine` in `findings[]`, and place `ephemeral-async-client` (`AdviceCategory::Performance`) and `shared-async-session` (`AdviceCategory::Correctness`) in `advice[]`.
  - **Rationale**: `sync-for-async-iterable` and `eager-offload-call` deterministically raise `TypeError` at runtime, and `unawaited-coroutine` on a bare `Stmt::Expr` never executes the coroutine body (matching `offload-coroutine` and `sync-with-async-cm`). By contrast, `ephemeral-async-client` and `shared-async-session` run valid async calls with performance/concurrency hazards that must not break CI exit codes (`advice[]` per [2026-10-31-advice-tier.md](../../decisions/2026-10-31-advice-tier.md)).
  - **Date**: 2026-11-12.
- **Decision**: Support `any_needs: &'static [&'static str]` alongside `needs` on `AsyncTwinEntry` / `AsyncTwinHit` for SQLAlchemy async twins, and keep `pymongo` excluded from `ASYNC_TWINS`.
  - **Rationale**: `sqlalchemy.ext.asyncio` requires both `greenlet` (`needs`) and at least one async DBAPI driver (`asyncpg`, `aiosqlite`, `aiomysql`, `asyncmy`, or `psycopg.AsyncConnection` via `any_needs`) to be actionable without installing new packages; `pymongo` exclusion is an established rule contract tested in `crates/async_doctor_rules/src/async_twin.rs`.
  - **Date**: 2026-11-12.
- **Decision**: Never emit first-party anti-pattern rules (`sync-for-async-iterable`, `eager-offload-call`, `unawaited-coroutine`, `ephemeral-async-client`, `shared-async-session`) from installed `site-packages` / `dist-packages` (`FunctionFact::is_installed_third_party()`), and deduplicate `eager-offload-call` when the eagerly evaluated inner call already produces a `blocking-*` finding at the same offload site.
  - **Rationale**: Users cannot edit installed third-party code, and emitting two findings for `await asyncio.to_thread(time.sleep(1))` (`blocking-sleep` + `eager-offload-call`) would double-report one bug.
  - **Date**: 2026-11-12.

## Context

- `crates/async_doctor_rules`: owns rule ID constants (`lib.rs`), `ASYNC_TWINS` (`async_twin.rs`), and `RULE_DOCS` (`docs.rs`).
- `crates/async_doctor_core`: owns plain graph/report types (`graph.rs`, `report.rs`) and propagation/grading (`propagate/mod.rs`, `propagate/special.rs`, `propagate/advice.rs`, `propagate/anti_pattern_advice.rs`). Must never depend on `ty_*` or `ruff_*`, and all `.rs` files must stay `<= 800` lines (`scripts/check_rust_file_length.sh`).
- `crates/async_doctor_ty`: owns AST + `ty` semantic extraction (`extract/collector/{async_anti_patterns.rs, eager_offload.rs, concurrency_anti_patterns.rs}`) and `twin_resolve.rs`, plus `CACHE_SCHEMA_VERSION` in `cache.rs`.

## Plan of work

### Milestone 1 — Async-twin expansion
Added `any_needs: &'static [&'static str]` to `AsyncTwinHit` and `AsyncTwinEntry`; included `any_needs` symbols in `twin_resolution_symbols()`; checked `any_needs.is_empty() || any_needs.iter().any(...)` in `advice.rs`; and added `grpc.aio`, `sqlalchemy.ext.asyncio.AsyncSession`, `sqlalchemy.ext.asyncio.AsyncEngine`, plus `psycopg.connect`, `redis.Lock.`, and `httpx._client.Client.` matchers in `crates/async_doctor_rules/src/async_twin.rs`.

### Milestone 2 — Deterministic anti-pattern findings
Added rule constants and `RULE_DOCS` entries for `sync-for-async-iterable`, `eager-offload-call`, and `unawaited-coroutine`. Implemented AST + `ty` semantic collectors in `async_anti_patterns.rs` and `eager_offload.rs` to detect:
- sync `for` / sync comprehension iterables proven async-only (undecorated `async def` call, base-free project class with `__aiter__` and without `__iter__`/`__getitem__`, or known async-only stream/iterator qualnames);
- offload callable-slot `Expr::Call` arguments (excluding `functools.partial`) whose return type is provably non-callable (`None`, builtin scalar/collection types, coroutine/generator calls, base-free non-`__call__` project class instances, or undecorated project functions with no value-returning `return`);
- bare `Stmt::Expr` calls to proven coroutine functions (undecorated or route/classmethod/staticmethod-only `async def` without `yield`, and typed external `async def`s whose return annotation is not `AsyncIterator`/`AsyncGenerator`/`AsyncIterable`).
Seeded and graded them into `findings[]` in `crates/async_doctor_core/src/propagate/special.rs` (excluding installed third-party callers and deduplicating `eager-offload-call` against existing findings) and added severity weights in `report.rs`.

### Milestone 3 — Lifecycle & concurrency advice rules + docs
Added `ephemeral-async-client` (`AdviceCategory::Performance`) and `shared-async-session` (`AdviceCategory::Correctness`) in `concurrency_anti_patterns.rs` and `anti_pattern_advice.rs`:
- `ephemeral-async-client`: detects `httpx.AsyncClient(...)` / `aiohttp.ClientSession(...)` (without `transport=` / `mounts=` / `app=` / `connector=` / `connector_owner=` kwargs) entered via `async with` or closed (`aclose`/`close`) in the same function scope, when reached from an `EntryLabel::Route` entry or across a `ConditionTag::Loop` hop (excluding `EntryLabel::Lifecycle` entries).
- `shared-async-session`: detects a single `AsyncSession` / `AsyncConnection` / `asyncpg.Connection` / `psycopg.AsyncConnection` local or attribute used across `>= 2` concurrent coroutine spawns in `asyncio.gather(...)` / `TaskGroup` (or unpacked comprehension / loop task spawn inside `TaskGroup` where the session was bound outside the loop).
Bumped `CACHE_SCHEMA_VERSION` to `22`, updated `ARCHITECTURE.md`, `docs/rules.md`, and `skills/async-doctor/SKILL.md`, and expanded `crates/async_doctor/tests/cli_anti_patterns.rs` to 10 E2E tests.

### Milestone 4 — Verification & corpus audit
Ran `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`, `./scripts/check_rust_file_length.sh`, `python3 scripts/check_docs.py`, concurrent cold-cache tests, and release corpus audit via `./bench/run.sh` (`identical=yes` on all 5 corpora; verified 0 false positives across `litellm`, `open-webui`, `httpx`, `starlette`, and `fastapi`).

## Validation

- `cargo test --workspace` passes including all 10 CLI E2E tests in `crates/async_doctor/tests/cli_anti_patterns.rs`.
- `async-doctor explain <rule>` exits `0` for `sync-for-async-iterable`, `eager-offload-call`, `unawaited-coroutine`, `ephemeral-async-client`, and `shared-async-session`.
- `./scripts/check_rust_file_length.sh` and `python3 scripts/check_docs.py` exit `0`.
- `./bench/run.sh` reports `identical=yes` on all 5 benchmark corpora (`litellm`, `open-webui`, `httpx`, `starlette`, `fastapi`), with 0 false positives in `findings[]` and +61 genuine `advice[]` hits on `open-webui`.

## Outcomes & Retrospective

All five non-blocking async anti-pattern and async-twin features shipped cleanly within the existing Pass-2 collector + reverse-BFS propagation architecture without adding config files or crossing crate boundaries. Separating deterministic `TypeError` / dead-coroutine bugs (`findings[]`) from performance and concurrency hazards (`advice[]`), requiring both `greenlet` and an async DBAPI driver via `any_needs` for SQLAlchemy async-twin advice, and walking past intermediate `EntryLabel::Other` helpers for `ephemeral-async-client` proved critical for zero-false-positive precision and high real-world recall on `open-webui` and `litellm`.
