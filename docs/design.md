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

## M0 delivered here

- Workspace crates: `async_doctor_core`, `async_doctor_rules`, `async_doctor_ty`,
  `async_doctor` (binary name `async-doctor`).
- All ty/ruff usage isolated in `async_doctor_ty`, pinned to **0.0.20**.
- Custom salsa `AsyncDoctorDb` implementing `ty_python_semantic::Db` (and the
  lower Db traits it requires).
- Direct `time.sleep` / `from time import sleep` inside `async def` → finding
  with file:line, text + JSON (`schemaVersion` 1), exit codes per decisions.
- Negative fixtures: sync `def`, `if TYPE_CHECKING`.
- maturin `bindings = "bin"`; `THIRD_PARTY_NOTICES.md` for typeshed Apache-2.0.
- CI: fmt, clippy, test.
- **No config file.**

### API deviations from the draft narrative

The design doc sketched hydroid's 0.0.15-era wiring. Against **0.0.20**:

- `ProgramSettings` lives in `ty_python_core::program` (not re-exported as a
  ty_python_semantic type for construction); it now also carries
  `python_environment`.
- `SearchPaths::try_register_static_roots` is used after
  `SearchPathSettings::to_search_paths(..., &FallibleStrategy)`.
- Parsing goes through `ruff_db::parsed::parsed_module` keyed by
  `PythonFile` / `ProgramFile`, not a bare parser call.
- M0 resolves `time.sleep` via import bindings on the AST (sufficient for
  *direct* calls); typed call-graph resolution is M1.
- Finding `id` hashes rule + entry file + **qualified** entry name
  (`Class.method` when nested) + sink + call site (file/line/column).
- Direct findings do not descend into **lambda** bodies (deferred); inline
  **comprehensions** still report.
- Syntax-error files set `complete=false` and a `skipped` entry; CLI warns on
  stderr. Exit code still follows findings only.
- Path argument may be a project directory or a single `.py` file.
