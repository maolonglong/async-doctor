# async-doctor rules

Stable rule ids for findings. Use `async-doctor explain <rule-id>` for the same
text in the terminal. Inline suppressions: `# async-doctor: ignore[<rule-id>]: <reason>`.

## blocking-sleep

`time.sleep` (and similar) freezes every task sharing the asyncio event loop.

**Fix:** `await asyncio.sleep(...)`, or `await asyncio.to_thread(time.sleep, seconds)`.

## blocking-socket

Sync socket IO, blocking DNS (`getaddrinfo`), SSL handshake, or sync
`smtplib` / `ftplib` clients on the loop thread.

**Fix:** asyncio connection/server APIs, `aiosmtplib` / async FTP, or offload
with `asyncio.to_thread`.

## blocking-http

Sync HTTP clients (`urllib`, `requests`, sync `httpx` / `urllib3`).

**Fix:** `httpx.AsyncClient` / `aiohttp`, or `await asyncio.to_thread(...)`.

## blocking-subprocess

`subprocess.run` / `Popen.wait` / `os.system` wait on the event-loop thread.

**Fix:** `asyncio.create_subprocess_*` and await communicate/wait, or offload.

## blocking-file

`open`, path read/write, directory walks (`os.listdir` / `pathlib.Path.iterdir` /
`glob`), archives (`zipfile` / `tarfile`), and blocking `input()` on the loop.

**Fix:** async file helpers, or `await asyncio.to_thread(...)`.

**Notes:**

- In-memory buffers (`io.BytesIO` / `StringIO`, including locals bound to them)
  are not file IO: `open`/`Image.open` and `zipfile`/`tarfile` opened on those
  buffers stay clean for pure in-memory ops (`writestr`/`read`/`namelist`/…).
  Disk-touching methods on a buffer-backed archive (`write(path)`, `extractall`,
  `add`, …) still report.
- `open`/`ZipFile` plus later `read`/`write`/`writestr` on the same call-path
  prefix merge into one finding; `SMTP(...).sendmail(...)` on one line likewise.

## blocking-lock

Threading locks, `queue.Queue`, blocking `concurrent.futures.Future.result`,
`Executor.map` consumption, and `concurrent.futures.wait` on the loop.

**Fix:** asyncio primitives, `asyncio.wrap_future` / `asyncio.wait`, or confine
waits to workers.

**Notes:**

- Thread-identity off-loop guards drop blocking sites in that branch when the
  other side looks like a loop-thread id (`loop_thread_id`,
  `hass.loop_thread_id`, …) compared with `threading.get_ident()`, or
  `current_thread() is not main_thread()` (and the complementary else of an
  on-loop test). Arbitrary `owner != get_ident()` compares are not off-loop.
- `.done()` on the same receiver (name or attribute chain) guards `.result()`
  in `if`, `and`, and ternary forms.

## blocking-db

Sync database drivers and sync cloud/AI SDKs (SQLAlchemy Session, redis, boto3, …).

**Fix:** async client twin when available, or `await asyncio.to_thread(...)`.

**Notes:**

- `AsyncSession.run_sync` runs sync work on a greenlet in the event-loop thread
  — it is **not** a thread offload and is not treated as one.
- Sync `boto3` / `botocore` client and resource API calls (including dynamic
  methods such as `get_object` / `get_item`) are reported with an `aioboto3` /
  `aiobotocore` fix hint. Meta helpers (`get_paginator`, `get_waiter`,
  `can_paginate`, `generate_presigned_url`, `meta`) are not IO and stay clean.
  There is still no dedicated `aioboto3` sink/follow family; `aioboto3.*` /
  `aiobotocore.*` qualnames (including `Session.client` / `Session.resource`
  and `async with` receivers) are classified as async counterparts (never
  findings), even when boto3 is also installed and types-aioboto3 /
  `aioboto3-stubs` is present. Sync `boto3-stubs.*` / `botocore-stubs.*`
  package roots normalize to `boto3` / `botocore` so typed Session factories
  still report.

## unresolved-call

Dynamic dispatch that could not be proven. Candidates only — never fails the
exit code by itself. Inspect, add types, or offload if the call is sync IO.

## offload-has-async-twin

**Advice (performance)** — never fails the exit code.

An offload boundary (`asyncio.to_thread`, `run_in_executor`,
`anyio.to_thread.run_sync`, `starlette.concurrency.run_in_threadpool`,
`asgiref.sync.sync_to_async`, inferred wrappers) cuts a sync call that only
reaches sinks with a high-confidence same-package async twin
(`httpx.AsyncClient` including module-level `httpx.get`/`post`/…,
`redis.asyncio`, `openai.AsyncOpenAI`, `openai.AsyncAzureOpenAI`,
`anthropic.AsyncAnthropic`, `elasticsearch.AsyncElasticsearch` needing
`aiohttp`, `psycopg.AsyncConnection`). The twin and its `needs` must resolve
in the scanned venv. The offload site's own try/if/loop conditions do not
suppress advice. Skipped when the offloaded function is also called from
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

Sites under a proven “no running loop” guard are dropped entirely:
`if loop is None`, early-return `if loop is not None: return` (rest of the
function), `except RuntimeError` after `get_running_loop` / `get_event_loop`,
`not loop.is_running()`, and similar helpers. Other conditional nested-loop
sites stay findings (with normal branch conditions).

**Fix:** await the coroutine directly; do not nest event-loop runners.

## offload-coroutine

An offload target resolves to an `async def`. Offload APIs expect a sync
callable; passing a coroutine function builds a coroutine object that is never
awaited, so the work never runs.

**Fix:** await the coroutine directly, or pass a sync function to the offload.

## sync-with-async-cm

A synchronous `with` statement enters an async-only context manager
(`httpx.AsyncClient`, `aiohttp.ClientSession`, `asyncio.Lock` / `Semaphore` /
`Condition` / `TaskGroup` / `timeout`, `sqlalchemy.ext.asyncio.AsyncSession` /
`AsyncEngine.begin`, `aioboto3.Session.client`, `redis.asyncio.Redis.pipeline`,
or a project class that defines `__aenter__` without `__enter__`). Entering an
async-only context manager with sync `with` raises `TypeError` at runtime.

**Fix:** replace `with ...:` with `async with ...:`.

## dangling-task

**Advice (correctness)** — never fails the exit code.

A bare `asyncio.create_task(...)`, `loop.create_task(...)`, or
`asyncio.ensure_future(...)` expression statement discards the returned `Task`.
Because the event loop only holds weak references to scheduled tasks, an
unreferenced task may be garbage-collected mid-execution and any unhandled
exception goes unobserved. Assigned, awaited, returned, passed-as-argument, or
`asyncio.TaskGroup.create_task` calls are not reported.

**Fix:** await the task, use `asyncio.TaskGroup`, or keep a strong reference in
a task set (`tasks.add(t); t.add_done_callback(tasks.discard)`).
