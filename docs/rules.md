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
