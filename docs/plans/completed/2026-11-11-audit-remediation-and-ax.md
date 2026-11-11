# Remediate audit correctness, cache soundness, performance, and AX gaps

## Purpose

Eliminate false-positive and silent-drop bugs across call-graph extraction, propagation, rule matching, whole-scan/package caching, and inline suppressions; cut cold-scan filesystem/memory overhead on large Python monorepos; and add zero-config `react-doctor`-style CLI/agent ergonomics (`--fail-on`, `--diff`, `why --format json`). After this lands, agents and CI pipelines get deterministic, non-panicking findings with accurate line-level suppressions, safe single-file scans, structured `why` JSON output, and measurable cold-scan latency/binary-size/recall wins on the external benchmark corpus.

## Progress

- [x] (2026-11-11 14:47Z) Verified clean baseline (`cargo nextest run --workspace`, `cargo clippy`, `check_docs.py`, `check_rust_file_length.sh`, concurrent cold-cache stress tests) and captured external corpus baseline via `./bench/run.sh` (`litellm`: 11.27s cold / 0.21s warm / 1.58 GiB RSS, 3874 findings, 7466 candidates).
- [x] (2026-11-11 15:21Z) Milestone 1: Core propagation, `ty` extraction, and third-party rule precision fixes (with TDD regression tests).
- [x] (2026-11-11 15:43Z) Milestone 2: Whole-scan & package cache soundness fixes plus cold-scan RSS and throughput optimizations.
- [x] (2026-11-11 15:56Z) Milestone 3: Suppression scanner accuracy, single-file & `--changed`/`--diff` fixes, and CLI/AX ergonomics (`--fail-on`, `why --format json`, SARIF/text/markdown fixes).
- [x] (2026-11-11 16:09Z) Milestone 4: Full test/stress verification, external corpus benchmark re-run & `audit_compare.py` diff review, documentation sync, and plan completion.

## Surprises & Discoveries

- **Baseline corpus profile (`./bench/run.sh` at `40bd0ba`)**: `litellm` (`5029d027a1b6`, 3009 files) took 11.27s cold, 0.21s warm, and 1653360 KiB (1.58 GiB) peak RSS (`identical=yes`). Smaller corpora (`open-webui`, `httpx`, `starlette`, `fastapi`) completed in 0.13s–0.78s cold and 97–312 MiB peak RSS.
- **Existing rule test contracts**: `crates/async_doctor_rules/src/third_party/tests.rs` explicitly asserts `pymysql.connections.Connection.cursor` remains a sink, and `fixtures/m2_third_party_rules/app.py` asserts `pymongo` `Collection.find({})` and `grpc.insecure_channel(...)` fire; tightening `pymysql`/`pymongo`/`grpc` targeted pure helpers, exception/model types, credentials, `.future`, and database/collection handle getters without altering those tested sinks.
- **Intermediate first-party `param_guard` vs sink `param_guard`**: Pushing intermediate first-party `e.param_guard` entries directly into the same `Unknown -> force_conditional` pool as `seed_edge.param_guard` initially demoted 1,682 conditional first-party findings (such as `if reranking_model: ColBERT(...)` in `open-webui` and `get_secret` branches in `litellm`) to `Candidate`. Separating `edge_param_guards` (prune `ProvablyFalse` dead chains; demote `Unknown` only for installed third-party callers) from `seed_param_guards` preserved 100% of baseline `(entry, sink)` sites while pruning dead literal-`False` multi-hop chains and surfacing +650 genuine higher-order bound-method findings on `litellm`.

## Decision log

- **Decision**: Keep cloud, AI, search, messaging, and RPC SDKs (`openai`, `anthropic`, `boto3`/`botocore`, `elasticsearch`, `kafka`, `grpc`) under `blocking-http` and align `crates/async_doctor_rules/src/docs.rs` + `docs/rules.md` with implementation.
  - **Rationale**: Preserves stable `finding_id` hashes, inline `# async-doctor: ignore[blocking-http]` suppressions, and baseline corpus comparability while fixing the documentation mismatch (`blocking-db` vs `blocking-http`).
  - **Date**: 2026-11-11 (approved by user).
- **Decision**: Continue reverse BFS in `shortest_paths_to_async_entries` past first-party `async def` functions located in non-production paths (`scripts/`, `cli/`, etc.) so production `async def` routes calling helper coroutines in non-production paths still surface as findings.
  - **Rationale**: Prevents silent false negatives when production endpoints reuse async helpers placed under utility modules, while keeping direct non-production entries demoted to `Candidate`.
  - **Date**: 2026-11-11 (approved by user).
- **Decision**: Bypass the whole-scan report cache when scanning a single `.py` file (`path.is_file()`), while keeping the third-party `PackageCache`, and scope `suppress::apply` on single-file scans strictly to that target `.py` file.
  - **Rationale**: Single-file scans previously hashed only the target file (returning stale reports when imported sibling modules changed) and walked the entire parent directory for `unusedSuppressions` (causing spurious exit-code 1 failures).
  - **Date**: 2026-11-11 (approved by user).
- **Decision**: Add `--fail-on <findings|none>` (default `findings`), `--diff [BASE]` as an alias for `--changed`, and `--format <text|json>` on `async-doctor why` (with `suppress::apply` applied before lookup), while staying strictly zero-config.
  - **Rationale**: Matches `react-doctor` CI/agent ergonomics without introducing config files or breaking existing CLI flags.
  - **Date**: 2026-11-11 (approved by user).

## Context

`async-doctor` is organized into four crates:
- `crates/async_doctor_core`: call-graph data model (`model.rs`, `graph.rs`), reverse-BFS propagation (`propagate/`), and inline suppression matching (`suppress.rs`).
- `crates/async_doctor_rules`: stdlib and third-party sink tables (`stdlib/`, `third_party/`), async-twin mapping (`async_twin.rs`), and rule documentation (`docs.rs`).
- `crates/async_doctor_ty`: Salsa/`ty` Pass-1 discovery (`files.rs`), Pass-2 per-file extraction (`pass2.rs`, `extract/collector/`), wave-based third-party follow (`extract/follow.rs`), third-party `PackageCache` (`cache.rs`, `sitepkgs/`), and whole-scan cache (`scan_cache.rs`, `scan.rs`).
- `crates/async_doctor`: CLI flags, `why`/`rule`/`rules` subcommands, `--changed` git diff filtering (`changed.rs`), and text/JSON/SARIF/markdown renderers (`main.rs`, `text/`, `sarif.rs`, `markdown.rs`).

## Plan of work

### Milestone 1 — Core propagation, `ty` extraction, and rule precision
Fixed bound-method `self`/`cls` parameter indexing (`FunctionFact` / `CallEdge`, `grade.rs`, `higher_order.rs`); evaluated intermediate `e.param_guard` along multi-hop paths (`propagate/mod.rs`); tracked exact binding edge index (`context_from_edge`) instead of coarse `FunctionId` for callbacks; eliminated `finding_id` collisions when distinct sinks normalize to `"nested-event-loop"` or offload rules; preserved `positional_callables` target on `@sync_to_async` project functions; continued reverse BFS past non-production `async def` helpers to production `async def` entries (`grade.rs`); fixed `CallCollector` state scoping (`async_with_receivers`, inline lambda enclosing depths, shadowed `narrowed_locals`, Union vs narrowed local precedence, non-dataclass `__post_init__`, 2-arg `super(Cls, self)`, `fn_stack`-scoped local classes, `functools.partial` / offload argument visiting, third-party decorator/constant/idle-loop extraction in `follow.rs`); and tightened pure non-IO exclusions for `pymysql`, `grpc`, and `pymongo` while aligning `docs.rs` and `docs/rules.md`.

### Milestone 2 — Cache soundness, cold/warm throughput, and peak RSS
Preserved cross-package `follow_targets` referenced by analyzed functions in `PackageCacheBatch::flush`; persisted `is_bound_call`, `is_eager_offload_arg`, `constructed_receiver`, `positional_externals`, and `positional_constructed_receivers` in `CachedEdge` (bumped `CACHE_SCHEMA_VERSION` to `21`); fixed `budget_skipped` wave double-counting; hardened `scan_input_fingerprint` (skipped whole-scan cache on single-file targets and incomplete scans, hashed outside-root editable/`.pth`/`extraPaths` inputs, supported `CONDA_PREFIX`, multi-level ancestor `.venv`, `lib64`, chunk-independent file hashing, PEP 420 namespace packages, single-file site-packages modules, and thread-unique atomic temp files). Optimized cold scans with a shared `SitePackagesIndex` across Pass 2 + follow, per-worker Salsa file eviction without cross-worker AST thrashing, wave-level jemalloc decay, zero-copy UTF-8 source decoding, `O(1)` `FunctionId` property lookup, O(new-edges) `upsert_function`, per-function AST eviction in `follow.rs`, ownership-based `ProjectFacts` propagation without redundant clones, memoized `canonicalize()`, removal of unused `rayon`, and release symbol stripping.

### Milestone 3 — Suppression accuracy, `--changed`, and CLI/AX ergonomics
Made `suppress.rs` multiline-string-aware, supported trailing ignore comments on closing lines of multi-line calls and comments above `@decorator`s, and restricted single-file scans to inspect only the target file. Filtered `unused_suppressions` in `changed::filter_report` and hardened `git diff` flags (`--no-ext-diff`, `--src-prefix=a/`, `--dst-prefix=b/`, `-c core.quotepath=false`). Fixed `extract/path.rs` relative segment classification and `imports.rs` `src/` layout root detection. Added `--fail-on <findings|none>`, `--diff [BASE]`, and `async-doctor why --format <text|json>` (with `suppress::apply_scoped` and space-safe location formatting), preserved pre-truncation `sink_sites` in grouped text/markdown output, fixed SARIF global rule descriptors and URI encoding, and updated `skills/async-doctor/SKILL.md`.

### Milestone 4 — Corpus audit, stress verification, and documentation closure
Ran full workspace tests, doc tests, `--ignored` N=100 concurrent cold-cache stress tests, `./scripts/check_rust_file_length.sh`, release binary build, `./bench/run.sh`, `python3 scripts/audit_compare.py diff`, documentation sync across `ARCHITECTURE.md`, `docs/rules.md`, `docs/cli.md`, `docs/json-contract.md`, `docs/benchmark.md`, and `docs/quality.md`, and `python3 scripts/check_docs.py`.

## Validation

- Added 31 dedicated regression tests across `crates/async_doctor_core/tests/propagation_regressions.rs` (9 tests), `crates/async_doctor_ty/src/tests/extraction_regressions.rs` (8 tests), `crates/async_doctor_ty/src/tests/cache_regressions.rs` (7 tests), and `crates/async_doctor/tests/cli_audit_regressions.rs` (8 tests), plus rule exclusion assertions in `crates/async_doctor_rules/src/third_party/tests.rs` — verified failing before fixes and passing via `cargo nextest run --workspace`.
- `cargo test -p async_doctor --test concurrent_cache -- --ignored --nocapture` passed all 4 concurrent cold-cache suites (including N=100 multi-package stress trials) with `0` mismatches.
- `./bench/run.sh` reported `identical=yes` on all 5 pinned corpora; `litellm` cold time improved from `11.27s` to `10.12s` (-10.2%), release binary size dropped from `35.9 MiB` (`37,602,368` B) to `28.1 MiB` (`29,494,128` B, -21.6%), and `audit_compare.py` confirmed `0` lost `(entry, sink)` sites across all 5 corpora with `+650` genuine higher-order bound-method findings surfaced on `litellm`.
- `./scripts/check_rust_file_length.sh` and `python3 scripts/check_docs.py` exited `0`.

## Outcomes & Retrospective

All correctness, cache soundness, performance, binary-size, and DX/AX audit items were resolved with TDD regression coverage and verified against both N=100 concurrent cold-cache stress tests and the 5-repo external benchmark corpus. Separating intermediate-hop parameter guards from sink-hop parameter guards proved essential on real-world Python codebases (`litellm`, `open-webui`) where intermediate helper functions routinely guard calls with `if model:` or `if secret_name:` checks that represent normal runtime branches rather than boolean sync/async sink toggles.
