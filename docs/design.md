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
3. **JSON fields:** adopt the draft as-is (`schemaVersion`, `complete`,
   `findings[]` / `candidates[]`, `id`, `rule`, `entry`, `chain[]`,
   `conditions[]`, `fix` (+ `fixGroupId`), `why`).
4. **Exit codes:**
   - `0` — no findings
   - `1` — findings present (even if analysis is incomplete)
   - `2` — tool error only
   Incomplete analysis is reported via `complete=false` + `skipped` in JSON and
   a stderr warning, **not** via exit code. Candidates never affect exit code.
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

Blocking propagation is reachability, not taint: mark sinks, push up the reverse
call graph, report at call sites inside `async def`, BFS for the shortest chain.

**Offload boundaries:** `asyncio.to_thread`, `loop.run_in_executor`,
`anyio.to_thread.run_sync`, `starlette.concurrency.run_in_threadpool`. Watch for
eager evaluation: `to_thread(f())` still runs `f` on the event loop.

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

## JSON report (schemaVersion 1)

| Field | Meaning |
| --- | --- |
| `schemaVersion` | integer; bump on breaking changes |
| `complete` | false on timeout / parse failure; attach `skipped` |
| `findings[]` / `candidates[]` | separate arrays; candidates always have `reason` |
| `id` | stable hash of rule + entry + sink (baseline-friendly) |
| `rule` | e.g. `blocking-sleep`, `blocking-http` |
| `entry` | async def location + label (`route` / `lifecycle` / `task` / `other`) |
| `chain[]` | shortest evidence chain (`file`, `line`, `text`) |
| `conditions[]` | e.g. `unconditional`, `loop` |
| `fix` | hint + `fixGroupId` (shared across same root cause) |
| `why` | runnable `async-doctor why file:line` |

Default output is human text; `--format json` for agents; SARIF later.

## Agent integration (later milestones)

- `uvx async-doctor .` — zero-config
- `async-doctor why file:line` — explain a site
- `--changed [base]` and `--baseline` — CI / agent loops
- Inline ignore: `# async-doctor: ignore[rule]` and `# noqa: ASYNCxxx`
- Stop-hook snippets (Claude Code / Cursor), thin `SKILL.md`
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
| M3 | Agent integration | `why`, `--changed`, `--baseline`, inline ignore, SARIF, Skill, Stop hooks |
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
- Async-twin names only in `fix.hint`.

## M2 delivered here

- **Site-packages follow** for functions reachable from project async entries,
  down to stdlib blocking primitives; third-party hops marked on `chain[]`.
  Sink-table Externals are never rewritten to Project (deterministic; no lost
  direct findings). Follow budget is relevance-ranked and sorted.
- **Per-`(package, version)` disk cache** under the user cache dir, keyed by
  External qualname and versioned by tool + schema; full edge summaries;
  atomic batched writes. Cold and warm runs must agree.
- **Built-in third-party rule table** (type-based + C-extension shortcut) with
  async-twin fix hints; async counterparts never fire. Redis `Pipeline`
  buffering (`set`/`get`/…) is not IO — only `execute()` blocks.
- **Higher-order** callable arguments; **ty-resolved** offload only, plus
  inferred wrappers whose bodies call a real offload primitive.
- Nested defs keyed by full scope; lambda assign+call; `@property` gets;
  genexpr consumption; imperative FastAPI/Starlette routes / `include_router`.
- `--show-unresolved` aggregates by callee pattern (ranked by entries).
- Venv discovery: ty + `.venv` / `venv` / `VIRTUAL_ENV` (uv/poetry in-project).
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
