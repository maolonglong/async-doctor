---
name: async-doctor
description: >-
  Diagnoses Python async blocking IO and protocol errors using async-doctor.
  Use after changing async Python code, before committing, or for a full audit.
  Runs changed/full scans, traces explain/why evidence, and separates proven
  diagnoses from repairs that require application and library context.
version: "1.3.0"
---

# async-doctor

Report proven async problems; recommend an edit only when inspected program and library evidence supports it.

`async-doctor` finds sync/blocking IO reachable from Python `async def`, protocol/coroutine errors, and non-blocking advice, with evidence chains. It never autofixes or loads a project config file.

## Agent Diagnosis and Repair Decision Tree

1. **Locate the evidence.** Inspect `Top site:` in grouped text, or `sinkSites[]` and `chokePointFile` / `chokePointLine` in JSON, then use `why` to trace the affected chains. Use reachability counts to choose what to inspect, not to predict how many findings an edit will clear.
2. **Inspect intent and callers.** Read the operation's purpose and its synchronous and asynchronous callers, including exported APIs, CLI entrypoints, background threads, and dynamic integrations. `syncCallerCount` / `sampleSyncCaller` describe only the analyzed graph; zero or missing counts do not establish complete external/dynamic caller coverage or permission to convert `def` to `async def`.
3. **Check ownership and library contracts.** Inspect transaction scope, resource ownership, thread/loop affinity, cancellation, timeouts, and resource lifetime. For a call such as `session.execute`, trace where the handle is created, used, committed or rolled back, and closed. Check the installed library's contracts rather than inferring safe thread transfer or async migration from a blocking diagnosis.
4. **Choose an evidence-supported outcome.** Justify why a proposed repair fits the intent, callers, ownership, and contracts you inspected. Neither `asyncio.to_thread` nor async client/driver migration is a default repair. If suitability cannot be established, retain and report the diagnosis and the missing evidence instead of making an unproven edit or suppressing it. Do not apply a blanket rewrite.
5. **Verify justified edits.** After `--changed` checks, run a full scan from the repository/package root and relevant tests for the affected behavior, including ownership, cancellation, and callers. Report remaining findings and analysis gaps; a withheld repair is not a clean scan.

## After editing async Python

Run from the repository/package root so cross-module helpers and callers are available. A single-file scan can diagnose a local site but is not a caller audit. Activate the project venv first (or set `VIRTUAL_ENV`) so third-party imports resolve; inspect the reported `environment` for missing dependencies. Then run:

```bash
uvx async-doctor --changed .      # or: uvx async-doctor --diff .
```

`uvx` fetches the package from PyPI; an already-installed `async-doctor` on `PATH` works the same. `--changed` (`--diff`) filters reporting to changed lines and affected definition/body spans since the merge-base, including uncommitted and untracked files. It short-circuits when no Python or Python config/lock files changed; it is a regression check, not a full audit.

Findings and unused suppressions fail the exit code unless `--fail-on none` is passed for advisory runs. Candidates and advice never affect the exit code.

- **Exit 0:** no reported findings or unused suppressions, or `--fail-on none`.
- **Exit 1:** findings or unused suppressions.
- **Exit 2:** tool error.

Incomplete analysis sets `complete=false` with skipped-work evidence, not exit 2. Check this separately from the exit code.

## For a full audit

```bash
async-doctor .
async-doctor --format json --max-findings 50 .  # compact JSON on pipes; sinkSites/score stay full
async-doctor --expand .                        # show all chains, groups, and advice
```

For JSON, prefer `--format json --max-findings 50 .`: piped stdout emits compact JSON automatically, and `omittedFindings`, `omittedCandidates`, and `omittedAdvice` record what was cut. Expand or inspect omitted groups when the affected sites need more evidence.

JSON retains `schemaVersion` 2 and includes `environment` (interpreter/venv/site-packages), `complete`, and the findings/candidates/advice arrays even when empty. SARIF maps advice to level `note`.

## Understanding a diagnosis

Each finding, candidate, and advice item keeps its diagnostic evidence and `fix` object:

- **chain** — async entry → … → sink / offload site, with `file:line` per hop
- **fix.hint** — always a string; `""` means no recommended edit, not a clean result. Only `sync-with-async-cm` and `sync-for-async-iterable` may suggest a local correction to the supported async-only protocol on the same object. Where async syntax is legal, that is `async with` or `async for`; it neither authorizes changing a containing function's API nor claims a globally optimal repair.
- **fix.fixGroupId** — stable diagnostic grouping even when the hint is empty; shared grouping is not a guarantee that one edit resolves every member
- **rule** / **docsAnchor** — stable id into `docs/rules.md`
- **why** — runnable locator (`async-doctor why file:line --path .`) that deduplicates rule docs and caps >5 chains per sink/rule unless `--expand`; location-less candidates have an empty locator

```bash
async-doctor explain blocking-sleep
async-doctor explain offload-has-async-twin
async-doctor explain nested-event-loop
async-doctor explain offload-coroutine
async-doctor why path/to/file.py:42 --path .
async-doctor why path/to/file.py:42 --path . --expand
async-doctor why path/to/file.py:42 --path . --format json
```

### Advice (non-blocking)

- **offload-has-async-twin** — same-package async symbols and required dependencies resolve in the scanned environment for every unconditionally reached sink behind an offload cut (`to_thread`, executors, anyio/starlette threadpool, `sync_to_async`, decorator offloads, or inferred wrappers). For example, a sync `httpx` call maps to available `httpx.AsyncClient`. Offload-site try/if/loop guards do not suppress advice; observed sync-only callers exempt the target. Excluded families include pymongo / requests / boto3 / file IO / subprocess / CPU. Its `performance` category remains for compatibility, not as a measured gain or a recommendation to replace a safe offload. Availability does not establish migration suitability.
- **dangling-task** — a bare `asyncio.create_task` / `loop.create_task` / `asyncio.ensure_future` statement discards the returned `Task` (the loop's weak reference does not keep it alive).
- **ephemeral-async-client** — `httpx.AsyncClient` / `aiohttp.ClientSession` is constructed and closed per route request or loop iteration, discarding its connection pool at that scope.
- **shared-async-session** — one stateful `AsyncSession` / `AsyncConnection` / `asyncpg.Connection` / `psycopg.AsyncConnection` is shared concurrently across tasks in `asyncio.gather` or `TaskGroup`.
- **offload-stateful-session** — a bound method of a non-thread-safe sync `Session` / `Connection` / `Cursor` is passed directly to `to_thread` / `run_in_executor` across threads.
- **useless-local-async-lock** — `async with asyncio.Lock():` / `Semaphore(...)` is freshly created per call in `async def` without escaping to shared tasks or closures (serializes nothing).
- **swallowed-cancelled-error** — `except asyncio.CancelledError:` (or bare `except:` / `except BaseException:`) in a non-Task/Lifecycle `async def` exits without re-raising cancellation.

### Error findings

- **nested-event-loop** — `asyncio.run` / `Runner.run` / `run_until_complete` / `run_forever` / `anyio.run` / `trio.run` is reachable from async without an offload. Loop-state guards (`if loop is None`, `except RuntimeError` after `get_running_loop`, `not loop.is_running()`, …) are dropped. Other conditional sites stay findings.
- **offload-coroutine** — an offload target is `async def` (coroutine never awaited); async-generator functions are excluded.
- **sync-with-async-cm** — sync `with` enters an async-only context manager (`__aenter__` without `__enter__`).
- **sync-for-async-iterable** — sync `for` or sync comprehension iterates an async-only iterable (`__aiter__` without `__iter__`/`__getitem__`).
- **eager-offload-call** — offload callable slot is a call `to_thread(fn(...))` returning a non-callable value (runs on loop + `TypeError`).
- **unawaited-coroutine** — a proven coroutine object is discarded by a bare call or used directly as an iteration/context object; `async for` and `async with` do not implicitly await it.

## Suppressions (no config file)

For an independently justified intentional exception, use an inline suppression with a reason. Supports comma-separated rules (`ignore[rule-a, rule-b]: reason`) and stacking after `# type: ignore[...]` or `# noqa: ...` on the same line. Trailing ignores apply only to their own statement; standalone comment lines apply to `line + 1`.

```python
time.sleep(1)  # type: ignore[arg-type]  # async-doctor: ignore[blocking-sleep]: intentional loop stall in regression test
```

Unused suppressions fail the run (tracked per rule).

## CI / regression

```bash
async-doctor --write-baseline baseline.json .
async-doctor --baseline baseline.json --changed .
```

Fingerprints are stable across line shifts (rule + entry/sink/chain symbol paths + occurrence, not lines).

## SARIF (GitHub code scanning)

```bash
async-doctor --format sarif . > async-doctor.sarif
```
