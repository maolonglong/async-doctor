# async-doctor rules

Stable rule ids for findings. Use `async-doctor explain <rule-id>` for the same
text in the terminal. Inline suppressions: `# async-doctor: ignore[<rule-id>]: <reason>`.

## blocking-sleep

`time.sleep` (and similar) freezes every task sharing the asyncio event loop.

**Fix:** `await asyncio.sleep(...)`, or `await asyncio.to_thread(time.sleep, seconds)`.

## blocking-socket

Sync socket IO, blocking DNS (`getaddrinfo`), or SSL handshake on the loop thread.

**Fix:** asyncio connection/server APIs, or offload with `asyncio.to_thread`.

## blocking-http

Sync HTTP clients (`urllib`, `requests`, sync `httpx` / `urllib3`).

**Fix:** `httpx.AsyncClient` / `aiohttp`, or `await asyncio.to_thread(...)`.

## blocking-subprocess

`subprocess.run` / `Popen.wait` / `os.system` wait on the event-loop thread.

**Fix:** `asyncio.create_subprocess_*` and await communicate/wait, or offload.

## blocking-file

`open`, path read/write, directory walks, and similar filesystem calls.

**Fix:** async file helpers, or `await asyncio.to_thread(...)`.

## blocking-lock

Threading locks, `queue.Queue`, and blocking `Future.result` on the loop.

**Fix:** asyncio primitives, `asyncio.wrap_future`, or confine waits to workers.

## blocking-db

Sync database drivers and sync cloud/AI SDKs (SQLAlchemy Session, redis, boto3, …).

**Fix:** async client twin when available, or `await asyncio.to_thread(...)`.

## unresolved-call

Dynamic dispatch that could not be proven. Candidates only — never fails the
exit code by itself. Inspect, add types, or offload if the call is sync IO.

## offload-has-async-twin

**Advice (performance)** — never fails the exit code.

An offload boundary (`asyncio.to_thread`, `run_in_executor`,
`anyio.to_thread.run_sync`, `starlette.concurrency.run_in_threadpool`,
`asgiref.sync.sync_to_async`, inferred wrappers) cuts a sync call that only
reaches sinks with a high-confidence same-package async twin
(`httpx.AsyncClient`, `redis.asyncio`, `openai.AsyncOpenAI`,
`anthropic.AsyncAnthropic`, `elasticsearch.AsyncElasticsearch` needing
`aiohttp`, `psycopg.AsyncConnection`). The twin and its `needs` must resolve
in the scanned venv. Skipped when the offloaded function is also called from
sync-only code, or when any reached sink is excluded (pymongo, requests,
boto3, file IO, subprocess, CPU work).

**Why:** the offload already avoids blocking the loop, but the async twin
removes the thread hop and keeps cancellation/context on the event loop.

**Fix:** call the async client/API directly from `async def` instead of
offloading the sync client.

## nested-event-loop

Calling `asyncio.run`, `asyncio.Runner.run`, `loop.run_until_complete` /
`run_forever`, `anyio.run`, or `trio.run` from async code without an offload
raises `RuntimeError` at runtime (loop already running). Behind an offload
boundary this is not reported.

**Fix:** await the coroutine directly; do not nest event-loop runners.

## offload-coroutine

An offload target resolves to an `async def`. Offload APIs expect a sync
callable; passing a coroutine function builds a coroutine object that is never
awaited, so the work never runs.

**Fix:** await the coroutine directly, or pass a sync function to the offload.
