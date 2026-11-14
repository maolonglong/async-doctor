---
name: async-doctor
description: >-
  Use after changing async Python code or before committing. Run
  `async-doctor --changed` for a fast regression check; run `async-doctor .`
  for a full audit. Detects sync/blocking IO reachable from async def (sleep,
  HTTP, DB, file, subprocess, locks), nested event-loop runners, offloaded
  async defs, and non-blocking async-twin/task advice. Use explain/why and the
  Agent Fix Decision Tree to remediate safely.
version: "1.3.0"
---

# async-doctor

Your agent blocks the event loop. This catches it.

Zero-config static analysis for Python: finds sync/blocking IO reachable from
`async def`, with a full evidence chain and a concrete fix hint. Also reports
correctness errors (`nested-event-loop`, `offload-coroutine`) and non-blocking
**advice** when an offload could be replaced by a same-package async twin.

## Agent Fix Decision Tree

Follow these 5 steps in order whenever remediating findings:

1. **Work from `sinkSites` choke points first** — Inspect `Top fix:` in grouped
   text output or `sinkSites[]` (and `chokePointFile` / `chokePointLine`) in
   JSON. One shared sync helper or sink site often clears dozens of route
   findings at once; fix the choke point instead of patching each route.
2. **Check sync callers before converting `def` → `async def`** — Inspect
   `syncCallerCount` / `sampleSyncCaller` on `sinkSites[]` (or the sync-caller
   note in grouped text and `why` output), and grep for synchronous callers
   (`def`, CLI entrypoints, WSGI, background threads). If sync callers exist,
   keep a sync API or wrap the sync unit of work at the async boundary rather
   than breaking sync callers.
3. **NEVER pass stateful `session.execute` / `conn.execute` to `asyncio.to_thread`** —
   SQLAlchemy `Session`, DB-API `Connection`, and cursors are not thread-safe
   and must never cross threads mid-transaction. Either migrate to the native
   async driver (`AsyncSession`, `AsyncConnection`, `asyncpg`) or offload a
   **self-contained unit-of-work function** that opens, commits/rolls back, and
   closes its own connection inside the worker thread.
4. **Always scan the project root `.`, never a single `.py` file** — Scanning a
   single file misses cross-module callers, shared sync helpers, and project
   layout roots. Always run `async-doctor .` or `async-doctor --changed .` from
   the repository/package root.
5. **Verify with a full `async-doctor .` after `--changed` edits** — `--changed`
   is a fast diff-scoped check (short-circuiting in `<20ms` when no Python or
   config files changed), so signature/type changes that expose untouched lines
   require a final `async-doctor .` (or `--baseline`) check before finishing.

## After editing async Python

Activate the project venv first (or set `VIRTUAL_ENV`) so third-party imports
resolve — without it, findings silently drop because site-packages follow cannot
see project dependencies. Then run:

```bash
uvx async-doctor --changed        # or: uvx async-doctor --diff
```

(`uvx` fetches the package from PyPI; an already-installed `async-doctor` on
`PATH` works the same.) `--changed` (`--diff`) reports only findings whose async
entry or a chain hop line touches lines changed since the merge-base, including
uncommitted and untracked files — it is a regression check, not a full audit.

Fix every **finding** before finishing. Candidates and advice do not fail the
exit code; findings do (unless `--fail-on none` is passed for advisory runs).

| Exit | Meaning |
| ---- | ------- |
| 0 | Clean (no findings / no new findings vs baseline, or `--fail-on none`) |
| 1 | Findings (or unused suppressions) |
| 2 | Tool error |

## For a full cleanup

```bash
async-doctor .
async-doctor --format json --max-findings 50 .  # compact JSON on pipes; sinkSites/score stay full
async-doctor --expand .                         # show all fix groups and advice
```

For agents reading JSON, prefer `--format json --max-findings 50 .` — piped
stdout emits compact JSON automatically while `omittedFindings`,
`omittedCandidates`, and `omittedAdvice` record what was cut.

JSON includes `environment` (interpreter/venv/site-packages) and additive
`advice[]` (`schemaVersion` 2). Advice never changes the exit code. SARIF maps
advice to level `note`.

## Understanding a finding or advice

Each finding/advice includes:

- **chain** — async entry → … → sink / offload site, with `file:line` per hop
- **fix.hint** — await the async twin, `asyncio.to_thread(f, args)`, or switch client
- **rule** / **docsAnchor** — stable id into `docs/rules.md`
- **why** — runnable locator (`async-doctor why file:line --path .`) that
  deduplicates rule docs and caps >5 chains per sink/rule unless `--expand`

```bash
async-doctor explain blocking-sleep
async-doctor explain offload-has-async-twin
async-doctor explain nested-event-loop
async-doctor explain offload-coroutine
async-doctor why path/to/file.py:42 --path .
async-doctor why path/to/file.py:42 --path . --expand
async-doctor why path/to/file.py:42 --path . --format json
```

### Advice: `offload-has-async-twin` / `dangling-task` / `ephemeral-async-client` / `shared-async-session` / `offload-stateful-session` / `useless-local-async-lock` / `swallowed-cancelled-error`

- **offload-has-async-twin** — fires only at offload cut sites (`to_thread`, `run_in_executor`, anyio/starlette
  threadpool, `asgiref.sync.sync_to_async`, inferred wrappers) when every
  unconditionally reached sink has a high-confidence same-package async twin
  installed in the scanned venv (`httpx.AsyncClient` including module `httpx.get`,
  `redis.asyncio`, `AsyncOpenAI` / `AsyncAzureOpenAI`, `AsyncAnthropic`,
  `AsyncElasticsearch`+`aiohttp`, `psycopg.AsyncConnection`, `grpc.aio`,
  `sqlalchemy.ext.asyncio.AsyncSession`/`AsyncEngine` with `greenlet` + async driver). Offload-site try/if/loop
  guards do not suppress advice. Sync-caller exemption when the offloaded fn is
  also called from sync-only code. Excludes pymongo / requests / boto3 / file IO /
  subprocess / CPU.
- **dangling-task** — bare `asyncio.create_task` / `loop.create_task` / `asyncio.ensure_future`
  statement that discards the returned `Task` (weak loop reference can be GC'd mid-execution).
- **ephemeral-async-client** — `httpx.AsyncClient` / `aiohttp.ClientSession` constructed and closed per
  route request or loop iteration instead of reusing a shared pool.
- **shared-async-session** — one stateful `AsyncSession` / `AsyncConnection` / `asyncpg.Connection` /
  `psycopg.AsyncConnection` shared concurrently across tasks in `asyncio.gather` or `TaskGroup`.
- **offload-stateful-session** — passing a bound method of a non-thread-safe sync `Session` / `Connection` / `Cursor`
  directly to `to_thread` / `run_in_executor` across threads.
- **useless-local-async-lock** — `async with asyncio.Lock():` / `Semaphore(...)` freshly created per call in `async def`
  without escaping to shared tasks or closures (serializes nothing).
- **swallowed-cancelled-error** — `except asyncio.CancelledError:` (or bare `except:` / `except BaseException:`) in a
  non-Task/Lifecycle `async def` that exits without re-raising cancellation.

### Errors: `nested-event-loop` / `offload-coroutine` / `sync-with-async-cm` / `sync-for-async-iterable` / `eager-offload-call` / `unawaited-coroutine`

- **nested-event-loop** — `asyncio.run` / `Runner.run` / `run_until_complete` /
  `run_forever` / `anyio.run` / `trio.run` reachable from async without an
  offload. Loop-state guards (`if loop is None`, `except RuntimeError` after
  `get_running_loop`, `not loop.is_running()`, …) are dropped. Other conditional
  sites stay findings.
- **offload-coroutine** — offload target is `async def` (coroutine never awaited).
- **sync-with-async-cm** — sync `with` enters an async-only context manager (`__aenter__` without `__enter__`).
- **sync-for-async-iterable** — sync `for` or sync comprehension iterates an async-only iterable (`__aiter__` without `__iter__`/`__getitem__`).
- **eager-offload-call** — offload callable slot is a call `to_thread(fn(...))` returning a non-callable value (runs on loop + `TypeError`).
- **unawaited-coroutine** — bare `fn(...)` statement discards a proven coroutine without `await`.

## Suppressions (no config file)

Only when a sync call is intentionally on the loop thread — reason required.
Supports comma-separated rules (`ignore[rule-a, rule-b]: reason`) and stacking
after `# type: ignore[...]` or `# noqa: ...` on the same line. Trailing ignores
apply only to their own statement; standalone comment lines apply to `line + 1`.

```python
time.sleep(1)  # type: ignore[arg-type]  # async-doctor: ignore[blocking-sleep]: brief sync boundary
```

Unused suppressions fail the run (tracked per rule). Prefer fixing over ignoring.

## CI / regression

```bash
async-doctor --write-baseline baseline.json .
async-doctor --baseline baseline.json --changed
```

Fingerprints are stable across line shifts (rule + symbol path + sink, not lines).

## SARIF (GitHub code scanning)

```bash
async-doctor --format sarif . > async-doctor.sarif
```
