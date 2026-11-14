# Zero-FP soundness, environment auto-discovery, and LLM async advice loop

## Purpose

Eliminate silent third-party resolution blind spots, false-positive edge cases under nested/inverted parameter guards, and agent-loop friction when remediating async Python projects:
1. Auto-discover external virtualenvs (PDM PEP 582 `__pypackages__`, Poetry, and Pipenv) alongside `VIRTUAL_ENV`, ancestor `.venv`/`venv`, `CONDA_PREFIX`, and `PATH` `python3`/`python` (filtering ephemeral `uvx`/`pipx` tool venvs), expose an explicit `--python <PATH>` flag across extraction, whole-scan caching, and async-twin checks, and report missing top-level packages in `environment.unresolvedThirdPartyModules`.
2. Harden parameter-guard propagation with multi-guard conjunction (`param_guards`), `else` / early-return guard inversion, and `keyword_literals` (`helper(sync_mode=False)`), while expanding third-party sink/twin tables and eliminating false positives on non-IO calls (`BytesIO`/`StringIO` file ops, `TarFile.addfile` buffer IO, `sqlite3` `:memory:` connects, non-blocking `socket.setblocking(False)`/`settimeout(0)`).
3. Close `--changed` blind spots across cross-file `def` → `async def` conversions and intra-function / `Class.__init__` edits, surface sync-caller fan-in (`syncCallerCount`, `sampleSyncCaller`) in JSON and text/`why` output, remove the process-global ignore-candidate `Mutex`, and ship three zero-FP LLM async correctness `advice` rules (`offload-stateful-session`, `useless-local-async-lock`, `swallowed-cancelled-error`).

## Progress

- [x] (2026-11-13 17:18 UTC) Wave 1 landed & verified: merged all 5 non-overlapping Wave-1 worktrees (`ty-env`, `ty-collector`, `core-prop`, `rules-cat`, `cli-ux`) into the workspace (`0` collisions; `cargo nextest run --workspace` 100% green).
- [x] (2026-11-13 17:34 UTC) Wave 2.1–2.3 completed: wired `--python <PATH>` end-to-end (`args.rs` → `ScanOptions` → `scan_cache.rs` → `AsyncDoctorDb` / `env_discovery.rs` / `env_info.rs` / `twin_resolve.rs`), replaced process-global `LAST_SCAN_IGNORE_CANDIDATES` with `Report.ignore_candidate_files`, surfaced `environment.unresolvedThirdPartyModules` (top 10 sorted), and rendered `syncCallerCount` / `sampleSyncCaller` in grouped text and `why` output.
- [x] (2026-11-13 17:52 UTC) Wave 2.4 completed: implemented multi-guard `ParamGuard` conjunction, `else` and early-exit `if [not] param: return/raise` inversion, and `keyword_literals` across `graph.rs`, `collector/control_flow.rs`, `constants.rs`, `propagate/grade.rs`, `cache.rs` (`CACHE_SCHEMA_VERSION = 24`), and `sitepkgs/batch.rs`.
- [x] (2026-11-13 18:14 UTC) Wave 2.5 completed: implemented span-aware `--changed` retention (`FunctionFact.end_line`, `Report.item_spans`, `scan-cache-v8`, cross-file `def` → `async def` target tracking on `unawaited-coroutine`, `offload-coroutine`, `sync-with-async-cm`, and `sync-for-async-iterable`, plus intermediate/sink function body and `Class.__init__` span overlap).
- [x] (2026-11-13 18:27 UTC) Wave 2.6 completed: implemented `offload-stateful-session`, `useless-local-async-lock`, and `swallowed-cancelled-error` (`AdviceCategory::Correctness`) across `async_doctor_rules`, `async_doctor_ty` (`collector/llm_advice.rs`, `collector/llm_advice/local_lock.rs`), and `async_doctor_core` (`anti_pattern_advice/llm_rules.rs`), verified with positive + FP-guard unit and CLI E2E suites.
- [x] (2026-11-13 18:48 UTC) Wave 3 & 4 completed: synced `README.md`, `ARCHITECTURE.md`, `docs/json-contract.md`, `docs/cli.md`, `docs/rules.md`, `docs/quality.md`, and `skills/async-doctor/SKILL.md`; fixed `--python` stripping in `sink_sites::why_file_line` and singular/plural agreement in `help_cmd.rs`; passed full nextest, doctest, concurrent cold-cache stress, clippy, file-length, and doc checks.

## Surprises & Discoveries

- **Single-guard `param_guard: Option<ParamGuard>` silently dropped outer guards on nested branches**: In `def f(sync, verbose): if sync: if verbose: time.sleep(1)`, overwriting a single `Option<ParamGuard>` kept only `verbose`, causing `f(False, True)` to propagate as reachable instead of pruning as `ProvablyFalse`. Storing `param_guards: Vec<ParamGuard>` and evaluating conjunction (`any ProvablyFalse -> ProvablyFalse`, `all ProvablyTrue -> ProvablyTrue`, else `Unknown`) fixed nested guards cleanly.
- **Third-party `else`-branch sinks vs intermediate guarded helpers**: Direct sinks inside an `else` branch of an installed third-party function carry `ConditionTag::Branch` and legitimately demote to `Candidate` (`findings.rs` third-party branch rule), whereas a parameter-guarded call in an installed package to an unconditional sink helper (`else: _blocking_io()`) resolves to `ProvablyTrue` when the caller passes the matching literal (`run(False)`), remaining a `Finding` while `run(True)` prunes completely.
- **Cross-file `def` → `async def` under `--changed`**: When a helper in `b.py` changes from `def helper():` to `async def helper():`, existing sync call sites in untouched `a.py` become `unawaited-coroutine`, `offload-coroutine`, `sync-with-async-cm`, or `sync-for-async-iterable`, yet every chain hop points at `a.py`. Attaching target `FunctionId` spans (`FunctionFact.line..=end_line`) to `Report.item_spans` lets `--changed` retain those regressions without sacrificing `<20ms` non-Python diff short-circuits.
- **Core module file-length headroom**: Splitting `pass2.rs`, `resolve.rs`, `suppress.rs`, `families.rs`, `cache.rs`, `propagate/mod.rs`, `grade.rs`, and `llm_advice.rs` by responsibility brought every production `.rs` file in `crates/` to `<= 575` lines, clearing the 700–779 line tech-debt band recorded in `docs/quality.md`.

## Decision log

- **Decision**: Auto-discover PDM `__pypackages__`, Poetry, and Pipenv virtualenvs statically without invoking Python subprocesses, filter ephemeral `uvx`/`pipx` tool venvs on `PATH`, and add `--python <PATH>` as the highest-precedence override.
  - **Rationale**: Preserves the zero-Python-execution invariant (`ARCHITECTURE.md`), works deterministically in agent sandboxes where shell activation does not persist across commands, and hashes cleanly into `scan-cache-v8`.
  - **Date**: 2026-11-13.
- **Decision**: Eliminate the process-global `LAST_SCAN_IGNORE_CANDIDATES` `Mutex` by returning `ignore_candidate_files: Option<Vec<PathBuf>>` on `Report` (`#[serde(skip)]`, recomputed during `scan_input_fingerprint_with_python`) alongside `item_spans` persisted in `ScanCacheFile` (`scan-cache-v8`).
  - **Rationale**: Concurrent in-process scans previously shared a process-wide slot (`codebase-design`: return results instead of mutating hidden state). Carrying `ignore_candidate_files` and `item_spans` on `Report` makes cold and warm runs thread-safe and bit-for-bit identical without changing the public JSON schema.
  - **Date**: 2026-11-13.
- **Decision**: Ship `offload-stateful-session`, `useless-local-async-lock`, and `swallowed-cancelled-error` strictly in `advice[]` (`AdviceCategory::Correctness`) using `CalleeRef::External` seeds (`"offload-stateful-session:..."`, `"useless-local-async-lock:..."`, `"swallowed-cancelled-error:..."`) instead of adding rule-specific fields to `CallEdge`.
  - **Rationale**: All three rules catch common LLM-generated async bugs while keeping `CallEdge` lean (`0` schema bloat) and preserving exit code `0` per [2026-10-31-advice-tier.md](../../decisions/2026-10-31-advice-tier.md). Suppressing `swallowed-cancelled-error` on `EntryLabel::Task` and `EntryLabel::Lifecycle` entries, on re-raising handlers, and when an earlier `except CancelledError:` clause re-raises eliminates false alarms on legitimate cancellation boundaries.
  - **Date**: 2026-11-13.

## Context

- `crates/async_doctor`: CLI flags (`args.rs`), stateless inline suppressions (`suppress.rs`, `suppress/lexer.rs`), span-aware git diff filter (`changed.rs`), grouped text / Markdown / JSON / SARIF output, `explain` / `why` (`help_cmd.rs`), and `install-skill` (`skill_cmd.rs`).
- `crates/async_doctor_ty`: Python environment resolution (`env_discovery.rs`, `db.rs`, `env_info.rs`, `twin_resolve.rs`), AST + `ty` call-graph extraction (`extract/collector/`), package cache (`cache.rs`, `CACHE_SCHEMA_VERSION = 24`), and whole-scan report cache (`scan_cache.rs`, `scan-cache-v8`).
- `crates/async_doctor_core`: plain call-graph/report structures (`graph.rs`, `report.rs`, `environment.rs`, `sink_sites.rs`) and reverse-BFS propagation/grading (`propagate/`).
- `crates/async_doctor_rules`: sink and async-twin catalogs (`stdlib/`, `third_party/`, `async_twin.rs`) and rule documentation metadata (`docs.rs` ↔ `docs/rules.md`).

## Plan of work

### Milestone 1 — Wave-1 subsystem improvements & modular splits
Landed five verified Wave-1 slices: (1) PDM/Poetry/Pipenv environment discovery (`env_discovery.rs`), ephemeral `uvx`/`pipx` tool-venv filtering, and unresolved third-party module collection; (2) collector soundness fixes (`BytesIO`/`StringIO` in-memory IO, `TarFile.addfile` buffer checks, `sqlite3` `:memory:`, non-blocking `socket.setblocking(False)`/`settimeout(0)`, async generator wrappers, `asyncio.shield` / `wait` task consumers) and `resolve/` splits; (3) `propagate/grade/merge.rs` split, transitive sync-caller fan-in (`SinkSiteSummary`), and `FunctionFact.end_line`; (4) expanded third-party sink/twin coverage (`httpx` module verbs, `boto3`/`botocore` + `Waiter.wait` + `StreamingBody`, `sqlmodel`, `RedisCluster`, `valkey`, `pymongo` collection ops, `AnthropicBedrock`/`AnthropicVertex`, `confluent_kafka`, `groq`, `ollama`, `qdrant_client`) and `third_party/` splits; (5) `.pre-commit-hooks.yaml`, `action.yml`, piped compact JSON, and `suppress/lexer.rs` split.

### Milestone 2 — Cross-crate seams (`--python`, stateless suppressions, agent context)
Wired `--python <PATH>` through `Cli` and `WhyArgs` (`args.rs`), `ScanOptions` (`scan.rs`), `scan_input_fingerprint` (`scan_cache.rs`), `AsyncDoctorDb` (`db.rs`), `detect_environment` (`env_info.rs`), and `resolve_available_twins` (`twin_resolve.rs`). Removed `LAST_SCAN_IGNORE_CANDIDATES` in favor of `Report.ignore_candidate_files`. Surfaced `environment.unresolvedThirdPartyModules` in JSON and `sync_caller_count` / `sample_sync_caller` in grouped text and `why` output.

### Milestone 3 — Multi-guard `ParamGuard` & span-aware `--changed`
Replaced single `param_guard` with `param_guards: Vec<ParamGuard>` and added `keyword_literals: Vec<(String, Option<LiteralArg>)>` (`CACHE_SCHEMA_VERSION = 24`), supporting nested guard conjunction, `else`-branch inversion, and early-exit `if [not] param: return/raise` inversion. Added `Report.item_spans` (`scan-cache-v8`) covering intermediate/sink function spans (`line..=end_line`), `Class.__init__` spans on `self.<attr>` sinks, and target `async def` / class spans on `unawaited-coroutine`, `offload-coroutine`, `sync-with-async-cm`, and `sync-for-async-iterable`.

### Milestone 4 — LLM async correctness `advice` rules & documentation sync
Implemented `offload-stateful-session`, `useless-local-async-lock`, and `swallowed-cancelled-error` in `collector/llm_advice.rs` (`+ local_lock.rs`) and `propagate/anti_pattern_advice/llm_rules.rs`, added `RuleDoc` entries in `docs.rs` and sections in `docs/rules.md`, updated `README.md`, `ARCHITECTURE.md`, `docs/json-contract.md`, `docs/cli.md`, `docs/quality.md`, and `skills/async-doctor/SKILL.md`, and ran the full verification suite.

## Validation

- `cargo nextest run --workspace` and `cargo test --workspace --doc` pass with 0 failures, including `crates/async_doctor/tests/cli_wave2_seams.rs`, `crates/async_doctor_ty/src/tests/param_guards.rs`, and `crates/async_doctor_ty/src/tests/llm_advice_rules.rs`.
- `cargo nextest run -p async_doctor --test concurrent_cache --profile stress --run-ignored only -E 'test(concurrent_cold_scans) and not test(/_stress$/)'` passes concurrent cold-cache parity.
- `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings` exits `0` with 0 warnings.
- `./scripts/check_rust_file_length.sh` and `python3 scripts/check_docs.py` exit `0` with 0 errors and 0 warnings.

## Outcomes & Retrospective

All Wave-1 and Wave-2 soundness, environment discovery, `--changed` span retention, and LLM async advice improvements landed with zero project config files, zero `ty`/`ruff` leakage outside `async_doctor_ty`, zero process-global scan state, and every production `.rs` file at or below 575 lines. Returning `ignore_candidate_files` and `item_spans` directly on `Report` (`#[serde(skip)]`) and encoding new collector-proven anti-patterns via synthetic external advice seeds kept cross-crate interfaces small while preserving bit-for-bit parity between cold and warm caches.
