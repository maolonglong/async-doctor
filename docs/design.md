# async-doctor design (v0 + owner decisions)

> Tagline: **Your agent blocks the event loop. This catches it.**
>
> Spec source: design canvas v0 (2026-10-30) plus owner decisions recorded the
> same day (inlined below). Decisions override the draft where they differ.

## Goals and non-goals

**Goal:** one command on any Python project that finds sync/blocking calls
reachable from `async def` entries (network, disk, subprocess, sleep, locks),
prints a shortest evidence chain, and tells an agent how to fix it.

| Do | Don't |
| --- | --- |
| Static analysis across files/types, into site-packages | Runtime probing (recommend blockbuster/pyleak alongside) |
| asyncio and AnyIO semantics; FastAPI/Starlette entry recognition | Trio-only semantics; gevent/eventlet monkey-patches |
| Fix hints (`to_thread`, async clients) | Autofix in v0 |
| Obvious CPU hotspots as *candidates* only | General CPU profiling |

## Hard principles

1. **Zero-config.** No config file. Entries, rules, and offload boundaries are
   built-in or inferred. The only escape hatch is inline ignore comments.
2. **Clean-room.** Public docs and public crates only. Do not copy code from
   react-doctor (license forbids AI training/eval use) or strato (no license).
   hydroid/ruff/ty may be read for *how to use* the crates.
3. **Agent-first.** Stable JSON by default for agents; every finding carries an
   evidence chain, fix hint, and `why` command. False positives are correctness
   bugs: uncertain cases go to `candidates`, never `findings`.

## Owner decisions (2026-10-30)

These override the draft:

1. **Name:** `async-doctor`; repo `maolonglong/async-doctor` (private); **MIT**.
2. **Async twin:** implement only as a hint inside the `fix` field — never a
   separate finding.
   **Exception (owner-approved):** rule `offload-has-async-twin` emits a
   non-blocking `advice[]` item at offload sites that are already cut (so
   nothing is double-reported with `blocking-*` findings). Structured
   high-confidence same-package twins live in an `async_twin` table on sink
   families; the twin symbol and its `needs` must resolve in the scanned venv.
   Advice never changes the exit code.
3. **JSON fields:** adopt the draft as-is (`schemaVersion`, `complete`,
   `findings[]` / `candidates[]`, `id`, `rule`, `entry`, `chain[]`,
   `conditions[]`, `fix` (+ `fixGroupId`), `why`), plus additive
   `advice[]` with `category` (`performance` | `correctness`).
   **schemaVersion 2:** always emit top-level arrays (even when empty);
   add `environment` (interpreter/venv/site-packages + advisory `note`) and
   `locationLessCandidates` (e.g. aggregated missing third-party); `why`
   `--path` is relative to the scan cwd, never an absolute machine path.
4. **Exit codes:**
   - `0` — no findings
   - `1` — findings present (even if analysis is incomplete)
   - `2` — tool error only
   Incomplete analysis is reported via `complete=false` + `skipped` in JSON and
   a stderr warning, **not** via exit code. Candidates and advice never affect
   exit code.
5. **Entry labels:** `route` (HTTP/WebSocket handlers), `lifecycle`
   (lifespan/startup/shutdown), `task` (`create_task` / TaskGroup / background),
   plus fallback `other`. All auto-inferred. Labels affect ordering/presentation
   only, never whether something is reported.

## Technical base (verified 2026-10-30)

- `ruff_*` / `ty_*` crates are on crates.io (e.g. `ty_python_semantic` **0.0.20**,
  2026-10-27). Internal components, no semver promise — pin with `=`.
- salsa `0.28.5`; ty enables `salsa_unstable`.
- Approach (same as hydroid): exact pins, **one adapter crate** owns every
  ty/ruff call; bump versions in a dedicated PR after benchmarks.
- Packaging: maturin `bindings = "bin"` so `uvx async-doctor` works without a
  Rust toolchain. typeshed ships via `ty_vendored` (Apache-2.0 notice required).

## Architecture

```
parse (ruff_python_parser, parallel)
  → ty semantic index (own salsa Db, incl. site-packages)
  → call graph (typed receivers; dunder / property / with / for)
  → blocking propagation (primitives up reverse edges to async def)
  → offload boundaries (to_thread et al. cut the edge)
  → grade (findings / candidates + condition tags)
  → reporter (text, JSON, SARIF)
```

### Crate layout

| Crate | Role |
| --- | --- |
| `async_doctor_core` | Call graph, propagation, grading, report types — **no ty** |
| `async_doctor_ty` | **Only** crate that touches ty/ruff; emits plain facts |
| `async_doctor_rules` | Built-in rule table (static data) |
| `async_doctor` | clap CLI + reporters |

### Module map (source layout)

| Path | Responsibility |
| --- | --- |
| `async_doctor_ty::extract` | Path discovery, function collection, call-graph edges, third-party follow |
| `async_doctor_ty::extract::collector` | Per-file `CallCollector` (visit / control-flow / calls / resolve) |
| `async_doctor_ty::tests` | Integration tests split by area (`basics`, `offload`, `entries`, `conditions`, `constants`, `m2`) |
| `async_doctor_core::propagate` | Sink seeding, reverse BFS, grading (`higher_order`, `grade`) |
| `async_doctor_rules::third_party` | Type-based third-party sinks (`families`, `sinks`) |

Source files under `crates/` aim for ~500 lines and must stay at or under **800** (enforced in CI).

Blocking propagation is reachability, not taint: mark sinks, push up the reverse
call graph, report at call sites inside `async def`, BFS for the shortest chain.

**Offload boundaries:** `asyncio.to_thread`, `loop.run_in_executor`,
`anyio.to_thread.run_sync`, `starlette.concurrency.run_in_threadpool`,
`asgiref.sync.sync_to_async`, plus inferred wrappers. Watch for eager
evaluation: `to_thread(f())` still runs `f` on the event loop.

**Condition tags:** `unconditional`, `all-paths`, `loop`, `async-branch`,
`cache_miss_dependent`, `sync_entry`. Literal-constant conditions prune dead
branches (`if False`, `if TYPE_CHECKING`).

## Built-in rules (v0 plan)

| Category | Typical sinks | Notes |
| --- | --- | --- |
| stdlib | `time.sleep`, socket, urllib, ssl handshake, getaddrinfo, select | asyncio variants do not count |
| file IO | `open`, read/write, pathlib, `os.stat`/`listdir`/`walk`, shutil | small metadata → candidates |
| subprocess | `subprocess.run`/`call`/`check_output`, `Popen.wait`/`communicate`, `os.system` | `asyncio.create_subprocess_*` OK |
| locks | `threading.Lock.acquire`, `Event.wait`, `queue.Queue.get`, `Future.result` | uncontended short locks → candidates |
| HTTP | requests, `httpx.Client`, urllib3 | `httpx.AsyncClient` OK |
| DB | sync SQLAlchemy Session/Engine, sync psycopg, `redis.Redis` | `AsyncSession.run_sync` is greenlet, not a thread |
| cloud / AI | boto3, `openai.OpenAI` | `AsyncOpenAI` OK; aioboto3 not in v0 |

General patterns: deferred objects block when used (e.g. `with Engine.begin()`);
dead branches are ignored; async twins appear only as fix hints.

FastAPI sync `def` routes already run in a thread pool — do not report them.

## JSON report (schemaVersion 2)

| Field | Meaning |
| --- | --- |
| `schemaVersion` | integer; bump on breaking changes (currently **2**) |
| `complete` | false on timeout / parse failure; attach `skipped` |
| `environment` | interpreter/venv/site-packages used; `note` when degraded (ratio-aware when a venv is active) |
| `findings[]` / `candidates[]` / `advice[]` | always emitted (even empty); candidates have `reason`; advice is non-blocking (SARIF `note`) |
| `locationLessCandidates[]` | always emitted; the missing-third-party aggregate (formerly a hidden `--show-unresolved` candidate) lives here with empty `entry.file`/`chain` and empty `why` (no `file:line` locator) |
| `skipped[]` / `sinkSites[]` / `unusedSuppressions[]` / `unresolvedSummary[]` | always emitted (even empty) |
| `hiddenUnresolvedCandidates` / `omittedNonProductionEntries` | always emitted, including `0` |
| `id` | stable hash of rule + entry + sink (baseline-friendly) |
| `rule` | e.g. `blocking-sleep`, `blocking-http` |
| `entry` | async def location + label (`route` / `lifecycle` / `task` / `other`) |
| `chain[]` | shortest evidence chain (`file`, `line`, `text`) |
| `conditions[]` | e.g. `unconditional`, `loop` |
| `fix` | hint + `fixGroupId` (shared across same root cause) |
| `why` | runnable `async-doctor why file:line --path <cwd-relative>`; empty on location-less entries |

Default output is human text (leads with environment + sink-site count); `--format json` for agents; `--format sarif` for SARIF 2.1.0.

**Environment note policy:** no project venv and zero unresolved third-party →
silent. No project venv with unresolved third-party → activate/`VIRTUAL_ENV`
note. Active venv with unresolved/resolved third-party ratio ≥ 25% → “install
project deps/extras” note (does not tell the user to set `VIRTUAL_ENV` again).

## Agent integration (M3)

- `uvx async-doctor .` — zero-config
- `async-doctor why file:line` — explain findings touching a site (re-scan)
- `async-doctor explain <rule-id>` — long-form rule help + `docs/rules.md` anchor
- Findings carry full `chain[]` (file:line each hop), `fix.hint`, stable `rule`,
  additive `docsAnchor`, and runnable `why`
- `--changed [base]` and `--baseline` — CI / agent loops
- Inline ignore: `# async-doctor: ignore[rule]: <reason>` (reason required)
- Thin agent skill at `skills/async-doctor/SKILL.md` (`npx skills add`)
- MCP only after M4 if needed

## Performance targets (to calibrate)

| Scenario | Target |
| --- | --- |
| ~1000 project files, first run with site-packages | under 10s |
| Second run (deps cached) | under 2s |
| `--changed` small diff | ~1s |

Public corpora for benchmarks: full-stack-fastapi-template, open-webui, litellm,
langgraph, home-assistant core, plus small fixtures per rule/pattern.

## Risks

| Risk | Mitigation |
| --- | --- |
| ty API churn weekly | exact pins; single adapter crate; upgrade PR + benchmarks |
| Dynamic dispatch | mark unresolved → candidates only; never guess by name |
| C extensions without source | rule table by type; unknown natives unreported |
| Large site-packages | follow reachable functions only; cache by package version |
| False positives | findings require proof; each FP becomes a regression fixture |

## Milestones

| ID | Title | Scope |
| --- | --- | --- |
| **M0** | Skeleton | Cargo workspace; pin ty `=0.0.20`; own Db; report direct `time.sleep` in `async def`; maturin packaging so `uvx` works |
| M1 | Call graph + propagation | Typed receivers, shortest chain, offload cuts, stdlib rules, JSON + text |
| M2 | Follow dependencies | site-packages graph + version cache, third-party rules, findings/candidates + conditions |
| M3 | Agent integration | `why`, `--changed`, `--baseline`, inline ignore, SARIF, Skill |
| M4 | Benchmarks + 0.1 | Public corpora, precision tuning, PyPI; then evaluate MCP |

## M0 delivered

- Workspace crates: `async_doctor_core`, `async_doctor_rules`, `async_doctor_ty`,
  `async_doctor` (binary name `async-doctor`).
- All ty/ruff usage isolated in `async_doctor_ty`, pinned to **0.0.20**.
- Custom salsa `AsyncDoctorDb` implementing `ty_python_semantic::Db`.
- Direct `time.sleep` inside `async def`, JSON/text, exit codes, maturin.
- **No config file.**

## M1 delivered

- Type-resolved call graph in the adapter (`SemanticModel` +
  `definitions_for_name` / `definitions_for_attribute` / `HasType`).
- Blocking propagation from stdlib sinks up the reverse call graph to
  `async def` entries; shortest evidence chain via BFS.
- Offload cuts: `asyncio.to_thread`, `run_in_executor`,
  `anyio.to_thread.run_sync`, `starlette.concurrency.run_in_threadpool`,
  `ThreadPoolExecutor.submit/map`, plus callable refs / `partial` / lambdas
  passed to those. Eager `to_thread(f())` still reports.
- Entry labels inferred (`route` / `lifecycle` / `task` / `other`). Sync
  FastAPI/Starlette-style route handlers are not entries.
- Unresolved dynamic dispatch → `candidates` (`reason=unresolved`), hidden
  unless `--show-unresolved`. Metadata IO → candidates. Findings only when
  provable.
- `conditions[]`: prove-only. `unconditional` / `all-paths` never accompany
  `if`/`else`/`try`/`match` guards, early-exit guards (`if x: return` /
  `raise`, cache-hit returns), or comprehension `if` filters; also `loop`,
  `async-branch`, `cache_miss_dependent`. Parameter guards in *callees*
  with non-literal caller args → candidates (`reason=conditional`); literal
  falsy args prune. Entry-local param guards stay findings (`async-branch`).
- Dead-branch pruning for unreassigned module constants only when ty
  confirms the name binds to that module symbol (no local reassign, param
  shadow, `global`, or import); always-true early exits make the rest
  unreachable. Also `if False` / `TYPE_CHECKING`.
- Generators: create-without-iterate does not propagate; consume via
  `for` / `list` / `next` / `yield from` does.
- Zero-config scope: skip entries under tests/docs/examples/scripts/hidden
  dirs by default (`--include-non-production` to include; count in summary).
- Merge `open(...)` + same-entry `f.read()`/`f.write()` into one finding.
- Stable `id`: hash(rule + entry qualname + sink qualname + chain qualnames
  + occurrence). **Not** line/column.
- Async-twin names in `fix.hint` for findings; additionally,
  `offload-has-async-twin` advice at cut offload sites (see decision #2
  exception).

## M2 delivered here

- **Third-party follow** for functions reachable from project async entries,
  down to stdlib blocking primitives; third-party hops marked on `chain[]`.
  Sink-table Externals are never rewritten to Project (deterministic; no lost
  direct findings). Follow budget is relevance-ranked and sorted.
  **Policy:** both venv/`site-packages` and system `dist-packages` are
  third-party — they are followed when reachable, tagged `is_third_party`, and
  **never** treated as finding entries. Attribute Externals prefer the
  *receiver type* over mixin defining classes (redis `Pipeline` vs
  `CoreCommands`). Project subclasses of `redis.Redis` /
  `redis.asyncio.Redis` inherit sink status from the *receiver MRO*
  (asyncio base ⇒ never a sync sink), not from whether the shared
  commands-mixin method is `async def`. Only the Call that is the *direct*
  operand of `await` /
  `async for` iter / `async with` context is marked `is_awaited` (nested
  argument calls like `await send(open(p).read())` stay sync sinks). Awaited
  *known-async* APIs are not sync sinks and are not followed into library
  internals; awaited *sync* sinks (e.g. `await redis.Redis().get(...)`) still
  seed because the sync body runs on the loop before `await`. Reverse
  propagation stops at third-party async. Third-party sinks under a
  parameter/branch guard (e.g. PIL `if is_path(fp):`) demote to candidates.
  Sync `with Engine.begin()` records the context `Call` as `WithEnter`.
  Consume-only sinks (`redis.PubSub.listen`) fire only when iterated.
- **Per-`(package, version, content-fingerprint)` disk cache** under the user
  cache dir (`v11/{tool}/{pkg}/{version}-{fingerprint}.json`), keyed by
  External qualname. Package meta lookups are memoized in a **per-scan**
  `PackageMetaMemo` (no process-global state). Callers (and tests) may inject
  the cache root explicitly — do not rely on mutating process-wide
  `XDG_CACHE_HOME` under parallel tests. Cached `follow_targets` only retain
  callees that were actually analyzed (cold == warm == warm). Full edge
  summaries; atomic batched writes.
- **Whole-scan report cache** at `v11/{tool}/scan/{input-fingerprint}.json`
  (first-party **content hashes**, venv dist-info fingerprints, analyze
  options, follow budget, tool/schema): when unchanged, a warm run reloads the
  prior report and skips ty extract/analyze (cold == warm JSON). Schema bumps
  invalidate both package and scan caches. Default first-party pass-2 uses
  parallel per-thread salsa DB clones with work-stealing; `ASYNC_DOCTOR_PASS2_BATCH`
  trades wall time for lower peak RSS.

  **Content fingerprint (coarse):** prefer hashing wheel `RECORD` bytes mixed
  with the dist-info directory mtime; else `METADATA` len+mtime; else a
  one-level `(name,len,mtime)` of `.py` files in the package dir — never a
  recursive walk. **Limits:** does not see edits to nested non-`.py` assets,
  C-extension binaries, or files outside the package/dist-info tree; identical
  `RECORD` bytes with an unchanged dist-info mtime will reuse a cache entry
  even if an unpacked `.py` was patched in place (editable installs that
  refresh dist-info mtime invalidate correctly).
- **Built-in third-party rule table** (type-based + C-extension shortcut) with
  async-twin fix hints; async counterparts never fire. Redis sync `Pipeline`
  buffering (`set`/`get`/…) is not IO — only sync `Pipeline.execute()` blocks;
  `redis.asyncio` / `await pipe.execute()` never fire. Eager
  `to_thread(f())` / `run_in_executor(None, f())` report with a pass-callable
  hint. See `docs/precision_m3a.md` for corpus-gap status.
- **Higher-order** callable arguments (positional + kwargs, `partial`, bound
  methods) with cheap call-context sensitivity so bindings do not leak across
  call sites; **ty-resolved** offload only, plus inferred wrappers whose bodies
  call a real offload primitive. Optional/union receivers narrowed by
  `isinstance` / `is not None`. Proven in-memory buffers (`BytesIO` /
  `StringIO`) are never blocking-file sinks (skip open/Image.open follow).
- Nested defs keyed by full scope; lambda assign+call; `@property` gets;
  genexpr consumption; imperative FastAPI/Starlette routes / `include_router`.
- `--show-unresolved` aggregates by callee pattern (ranked by entries).
  Failed *first-party* imports surface once per file+module
  (`unresolved-import:…`); missing third-party packages collapse to one note.
  Relative imports that exist on disk (common when the scan root *is* the
  package) are not recorded. Plain unresolved Name/Attribute *calls* stay silent.
- Venv discovery: ty + `.venv` / `venv` / `VIRTUAL_ENV` (uv/poetry in-project).
- Zero-config extra search paths (appended after src roots; skip root dupes):
  `pyrightconfig` / `[tool.pyright] extraPaths`, `[tool.ty.environment]
  extra-paths`, `[tool.ruff] src`, and setuptools editable finder `MAPPING`
  parents. Outside-scan-root editables are follow-only (never entry sources).
  Scanning a package dir with `__init__.py` adds its parent for self-imports.
- Precision notes: `docs/precision_m2.md`.

### API notes (ty 0.0.20)

- `ProgramSettings` lives in `ty_python_core::program`; carries
  `python_environment`.
- `SearchPaths::try_register_static_roots` after
  `SearchPathSettings::to_search_paths(..., &FallibleStrategy)`.
- Parsing via `ruff_db::parsed::parsed_module` on `ProgramFile`.
- Return-type inference enabled (`ReturnTypeInference::Always`) so typed
  receivers resolve more often.
- Lambda bodies stay deferred unless the lambda is invoked; offload-passed
  lambdas are cut.
- Path argument may be a project directory or a single `.py` file.
