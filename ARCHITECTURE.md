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

Diagnosis is independent of repair: schema 2 keeps `fix.hint` as a string,
empty when no edit is established. Only same-object async-only protocol
corrections inside already-async functions retain hints; async-API availability
and eager execution remain chain evidence, not migration or offload
instructions. Rationale:
[evidence-bounded repair guidance](docs/decisions/2026-11-14-evidence-bounded-repair-guidance.md).

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

Binary crate: clap args (`args.rs` — including `--python`, `--changed` / `--diff`,
`--fail-on`, and `why --format json`), path resolution, whole-file inline
`# async-doctor: ignore[…]` suppressions (`suppress.rs`, `suppress/lexer.rs` —
consuming `Report.ignore_candidate_files` directly with no process-global state),
git `--changed` / baseline filters (`changed.rs`, `baseline.rs`), text /
Markdown / JSON / SARIF reporters (`text/`, `markdown.rs`, `sarif.rs`),
`explain` / `why` (`help_cmd.rs`), and `install-skill` (`skill_cmd.rs`). Depends
on `async_doctor_ty` for scanning and `async_doctor_core` for report types.
Human reporters omit empty repair sections; SARIF messages use diagnostic
chain evidence rather than repair hints. Non-MSVC builds use jemalloc.

**API boundary:** the published CLI surface. Exit codes: see
[`docs/json-contract.md`](docs/json-contract.md).

### `crates/async_doctor_ty`

Only crate allowed to depend on Astral `ty_*` / `ruff_*`. Owns `AsyncDoctorDb`
(`db.rs`), interpreter/venv discovery (`env_discovery.rs` — resolving `--python`,
`VIRTUAL_ENV`, ancestor `.venv`/`venv`, PDM PEP 582 `__pypackages__`, external
Poetry and Pipenv virtualenvs, `CONDA_PREFIX`, and `PATH` `python3`/`python`
while filtering ephemeral `uvx`/`pipx` tool venvs), search-path / editable /
pyright-extra discovery (`search_paths.rs`, `doctor_system.rs`), encoding skips
(`encoding.rs`), environment summary (`env_info.rs`, including
`unresolvedThirdPartyModules`), and twin resolution against the scanned venv
(`twin_resolve.rs`).

Explicit venv interpreter links retain the pyvenv.cfg environment rather than
resolving to the base interpreter; ordinary aliases still resolve their target.
Extraction, empty scans, warm refresh, fingerprints, and twin checks share this
normalization in `env_discovery.rs`.

`extract/`: path discovery (`files.rs`, `path.rs`), pass-1 function collection
(`functions.rs`, recording `FunctionFact.end_line`), pass-2 typed edges
(`pass2.rs` / `pass2/` — default parallel path caps workers at 8, clears
`ParsedModule` ASTs after per-file visits and at wave-level 200-file intervals
without cross-worker eviction, and runs in coordinated 2,500-file waves where
all workers share one `Storage` per wave and drop it to refcount 0 between
waves, with a shared read-only pass-1 function slice and pre-indexed
`SiteRootIndex`), per-file `CallCollector` (`extract/collector/`, including
`llm_advice.rs`, local deferred-generator consumption in `generators.rs`,
typed asyncio callback binding in `calls/callbacks.rs`, and local concurrent
Future completion proofs in `future_guards.rs`), and third-party follow with
budget (`follow.rs`).
Package cache: `cache.rs` / `cache/`. Whole-scan report cache: `scan_cache.rs`.
Site-packages helpers: `sitepkgs/`. See
[docs/notes/2026-11-salsa-fresh-empty-rss.md](docs/notes/2026-11-salsa-fresh-empty-rss.md).

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
- Named function and synthetic lambda bodies reset execution depths and parameter
  guards; call-site edges carry the enclosing conditions. Dead definitions
  retain their dead status.
- Future completion guards require the actual concurrent Future `done` and
  `result` methods. Proofs follow local true/false branches, intersect at live
  continuations, and invalidate on explicit receiver writes; deferred functions
  do not inherit them. Direct inline lambdas keep call-site capture proofs,
  excluding shadowing parameters. This is not heap/alias, arbitrary-call
  mutation, or cross-coroutine completion analysis.
- Local generator-origin and parameter-forwarding proofs use ty's use-def map,
  not IDE name navigation (which includes non-reaching bindings). Only a unique
  binding owned by the current function/lambda is followed; rebinding or
  ambiguity stops proof. Generator bodies attach to consumption, not creation;
  unconditional exhaustive consumption records that source as exhausted.
- Coroutine classification excludes async-generator bodies and nested lambda
  yields. Protocol misuse distinguishes a coroutine needing `await` from an
  actual async-only iterator/context manager. Implicit project context methods
  resolve through ty's MRO; asyncio callback edges require actual loop/Future
  method definitions, never a same-name-method heuristic.
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

**Whole-scan cache key** (`scan_cache.rs` / `scan_input_fingerprint`): bypassed
on single-file targets (`path.is_file()`) and never saved when `!report.complete`.
On directory scans, hashes schema version (`scan-cache-v8`), tool version, build
id, analyze options (`include_non_production`, `show_unresolved`), explicit
`--python` override, follow budget, scan-root path, every first-party `.py` /
`.pyi` content hash, ancestor and project discovery config files
(`pyproject.toml` / pyright / ruff), resolved interpreter/venv identity
(`--python` / `VIRTUAL_ENV` / `.venv` / PDM `__pypackages__` / Poetry / Pipenv /
`CONDA_PREFIX` / `PATH` `python3`), `.pth` / editable finder / loose
site-packages `.py` inputs, outside-root `extraPaths` / editable `.py`/`.pyi`
content hashes, and each installed dist-info content fingerprint. `ScanCacheFile`
persists `item_spans` while `scan_input_fingerprint_with_python` recomputes
`ignore_candidate_files` (`Option<Vec<PathBuf>>`) and restores both onto `Report`
(`#[serde(skip)]` on JSON output) so warm `--changed` and unused-suppression
checks match cold scans without process-global state. Package-cache schema is
`CACHE_SCHEMA_VERSION` 26 (including the corrected Future completion semantics):
it persists cross-package `follow_targets` referenced by analyzed edges alongside
`is_bound_call`, `is_eager_offload_arg`, `constructed_receiver`,
`positional_externals`, `positional_constructed_receivers`, decorator qualnames,
literal-`True` keyword args, multi-guard `param_guards`, and positional/keyword
literal bindings (including proven caller-parameter references as `param:N`)
so warm runs match cold runs bit-for-bit; a whole-scan cache hit preserves
`environment.followBudgetSkipped` and `environment.unresolvedThirdPartyModules`
while re-detecting the rest of `environment`.

### `crates/async_doctor_core`

Call graph types (`graph.rs`: `ProjectFacts`, `CallEdge`, `ConditionTag`,
`ParamGuard`, `AnalyzeOptions`, `DecoratorRef`, `OffloadKind`;
`FunctionFact.offload_decorator`, `is_offload_wrapper`, and `end_line` drive
offload-edge materialization and span-aware `--changed` filtering), stable
finding ids (`id.rs`), environment report struct (`environment.rs`), sink-site
grouping with sync-caller fan-in (`sink_sites.rs`), and report schema
(`report.rs`: `Report`, `Finding`, `Candidate`, `Advice`, `SCHEMA_VERSION`).
Function facts do not carry name-inferred async replacements. Sink-site and
choke-point counts describe reachability, not how many findings an edit clears;
sync-caller counts do not establish complete external/dynamic caller coverage.

`propagate/`: mark sinks, materialize higher-order bindings
(`higher_order.rs`), reverse BFS + multi-guard condition reconciliation + merged
open/read-write chain attribution (`grade.rs`, `grade/merge.rs`), special
findings (`special.rs`: nested event loop, offload-coroutine, sync-with-async-cm,
sync-for-async-iterable, unawaited-coroutine, eager-offload-call), and
non-blocking advice (`advice.rs`: offload-has-async-twin;
`anti_pattern_advice.rs` and `anti_pattern_advice/llm_rules.rs`: dangling-task,
ephemeral-async-client, shared-async-session, offload-stateful-session,
useless-local-async-lock, swallowed-cancelled-error).

Inferred wrappers retain their ordinary caller-to-wrapper edge alongside worker
target metadata; only the worker edge cuts propagation. Offload-decorated
function bodies remain whole-body cuts. `LiteralArg::Param` is plain provenance
for guard evaluation along an existing concrete path; rebinding/unknown values
stop the proof, and each step shortens the path prefix (not general taint).

**Architecture invariant:** this crate must not depend on ty or ruff.

**API boundary:** `analyze` / `analyze_with_options` / `analyze_owned` plus the
public report and graph types. Propagation is reachability, not taint. Reverse
BFS continues past first-party `async def` functions in non-production paths so
production entries awaiting non-production helpers still report findings.

### `crates/async_doctor_rules`

Static builtin sink table (`stdlib/` for stdlib/builtins rows + normalization,
type-based third-party families in `third_party/`), structured async-twin table
(`async_twin.rs`), and rule doc anchors into `docs/rules.md` (`docs.rs`). No
analysis logic. Builtin sinks supply no repair hint; rule explanations remain
available independently of a repair. Structured twins describe resolved
same-package availability, not semantically equivalent replacements.

**API boundary:** `BuiltinSinkTable` implementing `SinkTable`, plus
`docs_anchor_for` / rule id constants.

### `scripts/`

Repo checks and corpus comparison: `scripts/check_rust_file_length.sh` (≤800
lines under `crates/`), `scripts/check_docs.py` (doc links, budgets,
plan/decision structure), `scripts/audit_compare.py` (finding/candidate/advice
ID, location, and payload diff; optional strict gate), `scripts/agent_eval.py`
(external source/check captures and transcript-reviewed comparison eligibility),
and `scripts/tests/` (standard-library public-script regressions). Evaluation
snapshots and caches must stay disjoint from the project; scan/test Python
prefixes must agree. Transcript review is manual process evidence, not an
enforced isolation sandbox. See the
[evaluation note](docs/notes/2026-11-14-agent-repair-evaluation.md).

### `bench/`

Public-corpus cold/warm harness (`bench/run.sh`, `bench/repos.toml`). Clones
land under a gitignored directory inside `bench/` (see `bench/run.sh`).
`BENCH_STRICT=1` rejects failed preparation, incomplete/truncated report
envelopes, and cold/warm mismatches. Numbers and validation limits live in
`docs/benchmark.md`.

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
cold-cache; docs and Python script regressions on lint) and the maturin/PyPI
release workflow.
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
whole-scan cache; package cache only skips third-party re-parse. The default
parallel pass 2 bounds memory via explicit `ParsedModule::clear` calls and
coordinated 2,500-file Salsa waves (dropping the completed wave's shared
`Arc<Zalsa>` before starting the next). Optional
`ASYNC_DOCTOR_PASS2_BATCH=N` / `locality` overrides wave sizing with serial
batched resets. `ASYNC_DOCTOR_TIMING=1` prints phase spans on stderr.

### Observability

No remote telemetry. Timing env as above; JSON always emits `environment`,
`complete`, and empty arrays for unused tiers (`schemaVersion` 2).
