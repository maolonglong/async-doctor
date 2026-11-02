---
name: async-doctor
description: >-
  Use after changing async Python code or before committing. Run
  `async-doctor --changed` for a regression check; run `async-doctor .` for a
  full audit. Detects sync/blocking IO reachable from async def (sleep, HTTP,
  DB, file, subprocess, locks), nested event-loop runners, offloaded async defs,
  and non-blocking offload-twin advice. Use explain/why to fix findings and advice.
version: "1.2.0"
---

# async-doctor

Your agent blocks the event loop. This catches it.

Zero-config static analysis for Python: finds sync/blocking IO reachable from
`async def`, with a full evidence chain and a concrete fix hint. Also reports
correctness errors (`nested-event-loop`, `offload-coroutine`) and non-blocking
**advice** when an offload could be replaced by a same-package async twin.

## After editing async Python

Activate the project venv first (or set `VIRTUAL_ENV`) so third-party imports
resolve — without it, findings silently drop because site-packages follow cannot
see project dependencies. Then run:

```bash
uvx async-doctor --changed
```

(`uvx` fetches the package from PyPI; an already-installed `async-doctor` on
`PATH` works the same.) `--changed` reports only findings whose async entry or
a chain hop line touches lines changed since the merge-base, including
uncommitted and untracked files — it is a regression check, not a full audit.

Fix every **finding** before finishing. Candidates and advice do not fail the
exit code; findings do.

| Exit | Meaning |
| ---- | ------- |
| 0 | Clean (no findings / no new findings vs baseline) |
| 1 | Findings (or unused suppressions) |
| 2 | Tool error |

## For a full cleanup

```bash
async-doctor .
async-doctor --format json --max-findings 50 .  # cap each list; sinkSites/score stay full
async-doctor --expand .                         # show all advice (default text caps the list)
```

For agents reading JSON, prefer `--format json --max-findings 50` — it bounds
the payload while `omittedFindings`/`omittedCandidates`/`omittedAdvice` record
what was cut.

JSON includes `environment` (interpreter/venv/site-packages) and additive
`advice[]` (`schemaVersion` 2). Advice never changes the exit code. SARIF maps
advice to level `note`.

## Understanding a finding or advice

Each finding/advice includes:

- **chain** — async entry → … → sink / offload site, with `file:line` per hop
- **fix.hint** — await the async twin, `asyncio.to_thread(f, args)`, or switch client
- **rule** / **docsAnchor** — stable id into `docs/rules.md`
- **why** — runnable locator (`async-doctor why file:line --path .`) that
  explains findings, candidates, **and** advice at that site

```bash
async-doctor explain blocking-sleep
async-doctor explain offload-has-async-twin
async-doctor explain nested-event-loop
async-doctor explain offload-coroutine
async-doctor why path/to/file.py:42 --path .
```

### Advice: `offload-has-async-twin` / `dangling-task`

- **offload-has-async-twin** — fires only at offload cut sites (`to_thread`, `run_in_executor`, anyio/starlette
  threadpool, `asgiref.sync.sync_to_async`, inferred wrappers) when every
  unconditionally reached sink has a high-confidence same-package async twin
  installed in the scanned venv (`httpx.AsyncClient` including module `httpx.get`,
  `redis.asyncio`, `AsyncOpenAI` / `AsyncAzureOpenAI`, `AsyncAnthropic`,
  `AsyncElasticsearch`+`aiohttp`, `psycopg.AsyncConnection`). Offload-site try/if/loop
  guards do not suppress advice. Sync-caller exemption when the offloaded fn is
  also called from sync-only code. Excludes pymongo / requests / boto3 / file IO /
  subprocess / CPU.
- **dangling-task** — bare `asyncio.create_task` / `loop.create_task` / `asyncio.ensure_future`
  statement that discards the returned `Task` (weak loop reference can be GC'd mid-execution).

### Errors: `nested-event-loop` / `offload-coroutine` / `sync-with-async-cm`

- **nested-event-loop** — `asyncio.run` / `Runner.run` / `run_until_complete` /
  `run_forever` / `anyio.run` / `trio.run` reachable from async without an
  offload. Loop-state guards (`if loop is None`, `except RuntimeError` after
  `get_running_loop`, `not loop.is_running()`, …) are dropped. Other conditional
  sites stay findings.
- **offload-coroutine** — offload target is `async def` (coroutine never awaited).
- **sync-with-async-cm** — sync `with` enters an async-only context manager (`__aenter__` without `__enter__`).

## Suppressions (no config file)

Only when a sync call is intentionally on the loop thread — reason required.
The same `# async-doctor: ignore[rule]: reason` form works for advice rules.

```python
time.sleep(1)  # async-doctor: ignore[blocking-sleep]: brief sync boundary in CLI wrapper
```

Unused suppressions fail the run. Prefer fixing over ignoring.

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
