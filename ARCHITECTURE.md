# Architecture

## Bird's-eye view

`async-doctor` is a zero-config static analyzer: given a Python project path (or
a single `.py` file), it finds sync/blocking calls reachable from `async def`
entries, grades them into findings / candidates / advice, and emits text, JSON,
or SARIF.

Pipeline: discover files and environment → extract call-graph facts with ty
(pass-1 functions, pass-2 edges, third-party follow) → propagate sinks up the
reverse call graph with offload cuts → grade and report. Two caches sit on the
side: a per-package third-party edge cache, and a whole-scan report cache (see
**Whole-scan cache key** below). The tool never runs Python, never autofixes,
and never loads a project config file.

**Non-goals:** no runtime probing (recommend blockbuster/pyleak alongside); no
Trio-only semantics; no gevent/eventlet monkey-patch modelling; no autofix; no
general CPU profiling (obvious CPU hotspots may appear only as *candidates*).

Exit codes and JSON field meanings live in [`docs/json-contract.md`](docs/json-contract.md).

## Entry points

- `crates/async_doctor/src/main.rs`: CLI (`async-doctor`, `explain`, `why`);
  applies suppressions, `--changed` / `--baseline`, and reporters.
- `crates/async_doctor_ty/src/scan.rs`: public library scan
  (`scan_path` / `scan_path_with_options`); warm whole-scan cache short-circuit.
- `crates/async_doctor_ty/src/extract/mod.rs`: fact extraction into
  `ProjectFacts` (ty/ruff stay here).
- `crates/async_doctor_core/src/propagate/mod.rs`: sink seeding, reverse BFS,
  grading into a `Report`.

## Code map

### `crates/async_doctor`

Binary crate: clap args (`args.rs`), path resolution, inline
`# async-doctor: ignore[…]` suppressions, git `--changed` / baseline filters,
text / Markdown / JSON / SARIF reporters (`text/`, `markdown.rs`, `sarif.rs`), `explain` / `why`
(`help_cmd.rs`), and `install-skill` (`skill_cmd.rs`). Depends on `async_doctor_ty` for scanning and
`async_doctor_core` for report types. Non-MSVC builds use jemalloc.

**API boundary:** the published CLI surface. Exit codes: see
[`docs/json-contract.md`](docs/json-contract.md).

### `crates/async_doctor_ty`

Only crate allowed to depend on Astral `ty_*` / `ruff_*`. Owns `AsyncDoctorDb`,
search-path / editable / pyright-extra discovery (`search_paths.rs`,
`doctor_system.rs`), encoding skips (`encoding.rs`), environment summary for
reports (`env_info.rs`), and twin resolution against the scanned venv
(`twin_resolve.rs`).

`extract/`: path discovery (`files.rs`, `path.rs`), pass-1 function collection
(`functions.rs` with periodic Salsa resets), pass-2 typed edges (`pass2.rs` —
default parallel per-thread Salsa clones with periodic resets and a shared read-only
pass-1 function slice overlay), per-file `CallCollector` (`extract/collector/`), and
third-party follow with budget (`follow.rs`). Package cache: `cache.rs`. Whole-scan
report cache: `scan_cache.rs`. Site-packages helpers: `sitepkgs/`.

**Architecture invariant:** no ty/ruff types cross into `async_doctor_core`;
only plain `ProjectFacts` / `Report` values leave this crate.

**API boundary:** `scan_path*` and `extract_path*` (plus test seams for cache
root and follow budget). Callers must not mutate process-wide `XDG_CACHE_HOME`
under parallel tests — pass an explicit cache dir instead.

**Follow / search-path behaviour (current):**

- Venv/`site-packages` and system `dist-packages` are third-party: followed when
  reachable, never finding entries. Reverse propagation stops at third-party
  async. Awaited *sync* sinks still seed (sync body runs on the loop before
  `await`); awaited known-async APIs do not.
- Outside-scan-root editables are follow-only (never entry sources). Scanning a
  package directory (one containing a Python package init module) adds its
  parent for self-imports.
- Extra search paths (after src roots): pyright `extraPaths`,
  `[tool.ty.environment] extra-paths`, `[tool.ruff] src`, setuptools editable
  finder `MAPPING` parents.
- Imperative FastAPI/Starlette routes and `include_router` are recognized as
  entries. `--show-unresolved` aggregates by callee pattern; failed first-party
  imports surface once per file+module (`unresolved-import:…`); on-disk relative
  imports are not recorded as unresolved; missing third-party collapses to one
  note. Third-party sinks under parameter/branch guards (e.g. PIL
  `if is_path(fp):`) demote to candidates.

**Whole-scan cache key** (`scan_cache.rs` / `scan_input_fingerprint`): schema
version, tool version, build id, analyze options (`include_non_production`,
`show_unresolved`), follow budget, scan-root path, every first-party `.py` /
`.pyi` content hash, discovery config file contents
(`pyproject.toml` / pyright / ruff), active venv identity, and each installed
dist-info content fingerprint. Package-cache fingerprints are coarse
(RECORD/METADATA/one-level `.py`); identical RECORD + unchanged dist-info mtime
can reuse a package entry even if an unpacked `.py` was patched in place
(editable installs that refresh dist-info mtime invalidate correctly).

### `crates/async_doctor_core`

Call graph types (`graph.rs`: `ProjectFacts`, `CallEdge`, `ConditionTag`,
`AnalyzeOptions`), stable finding ids (`id.rs`), environment report struct
(`environment.rs`), sink-site grouping (`sink_sites.rs`), and report schema
(`report.rs`: `Report`, `Finding`, `Candidate`, `Advice`, `SCHEMA_VERSION`).

`propagate/`: mark sinks, materialize higher-order bindings
(`higher_order.rs`), reverse BFS + condition reconciliation (`grade.rs`),
special findings (`special.rs`: nested event loop, offload-coroutine), and
non-blocking advice (`advice.rs`).

**Architecture invariant:** this crate must not depend on ty or ruff.

**API boundary:** `analyze` / `analyze_with_options` plus the public report and
graph types. Propagation is reachability, not taint.

### `crates/async_doctor_rules`

Static builtin sink table (`stdlib/` for stdlib/builtins rows + normalization,
type-based third-party families in `third_party/`), structured async-twin table
(`async_twin.rs`), and rule doc anchors into `docs/rules.md` (`docs.rs`). No
analysis logic.

**API boundary:** `BuiltinSinkTable` implementing `SinkTable`, plus
`docs_anchor_for` / rule id constants.

### `scripts/`

Repo checks and corpus comparison: `scripts/check_rust_file_length.sh` (≤800
lines under `crates/`), `scripts/check_docs.py` (doc links, budgets,
plan/decision structure), `scripts/audit_compare.py` (audit corpus finding-ID
and location diff).

### `bench/`

Public-corpus cold/warm harness (`bench/run.sh`, `bench/repos.toml`). Clones
land under a gitignored directory inside `bench/` (see `bench/run.sh`).
Numbers live in `docs/benchmark.md`.

### `fixtures/`

Clean-room Python fixtures for rule families, offload, entries, third-party
follow, precision/recall cases, and multipackage cache scenarios. Exempt from
the file-length ceiling.

### `skills/`

User-facing product skill at `skills/async-doctor/SKILL.md` (`npx skills add`).
Not a repo-dev skill; packaging excludes it from the wheel.

### `.agents/skills/`

Repo agent skills (`codebase-design`, `agent-docs-workflow`,
`async-doctor-review`). Claude Code reaches them via symlinks under
`.claude/skills/`.

### `.github/`

CI (`ci.yml`: parallel `lint` + `test` jobs; main-only `stress` for concurrent
cold-cache; `check_docs.py` on lint) and the maturin/PyPI release workflow.
Strategy: [docs/quality.md](docs/quality.md) (CI / test strategy).

## Cross-cutting concerns

### Testing

- Unit/integration: `crates/async_doctor_ty/src/tests/` (split by area),
  `crates/async_doctor_core/src/propagate/tests.rs`,
  `crates/async_doctor_rules/src/third_party/tests.rs`.
- CLI: `crates/async_doctor/tests/` (`cli_*`, `concurrent_cache`,
  `file_length`).
- Default `cargo nextest run --workspace` / `cargo test --workspace` stays
  fast: all `concurrent_cold_scans*` are `#[ignore]` (main `stress` job runs
  N=15/20; see [docs/notes/2026-10-concurrent-cache-stress.md](docs/notes/2026-10-concurrent-cache-stress.md)).

### Error handling

Ty panics during per-file extract are caught and recorded as skipped work so
one bad file does not abort the scan; serial pass-2 panic rollback does not
undo mutations already applied to shared maps (deliberate). Tool failures
surface as exit 2 from the CLI.

### Performance

Targets and measured corpora: `docs/benchmark.md`. Warm path relies on the
whole-scan cache; package cache only skips third-party re-parse. Optional
`ASYNC_DOCTOR_PASS2_BATCH` / `locality` trades wall time for lower peak RSS.
`ASYNC_DOCTOR_TIMING=1` prints phase spans on stderr.

### Observability

No remote telemetry. Timing env as above; JSON always emits `environment`,
`complete`, and empty arrays for unused tiers (`schemaVersion` 2).
