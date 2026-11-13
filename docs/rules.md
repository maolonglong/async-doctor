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

Sync HTTP, RPC, messaging, and cloud/AI SDKs (`urllib`, `requests`, sync
`httpx` / `urllib3`, `boto3` / `botocore`, `openai`, `anthropic`,
`elasticsearch`, `kafka`, sync `grpc`).

**Fix:** `httpx.AsyncClient` / `aiohttp` / `AsyncOpenAI` / `AsyncAnthropic` /
`AsyncElasticsearch` / `aioboto3` / `aiokafka` / `grpc.aio`, or
`await asyncio.to_thread(...)`.

**Notes:**

- Non-IO client methods do not report: `httpx.Client.build_request` only
  builds a `Request` object, `httpx.Client.mount` registers transports, and
  `requests.Session.mount` / `requests.Session.get_adapter` are
  adapter-registry lookups.
- `openai` / `anthropic` `with_options` / `copy` return new clients — no
  request. `grpc` credential constructors (`ssl_channel_credentials`, etc.) and
  `.future(...)` calls are not blocking RPCs.
- Sync `boto3` / `botocore` client and resource API calls (including dynamic
  methods such as `get_object` / `get_item`) are reported under `blocking-http`
  with an `aioboto3` / `aiobotocore` fix hint. Meta helpers (`get_paginator`,
  `get_waiter`, `can_paginate`, `generate_presigned_url`, `meta`) are not IO and
  stay clean. `aioboto3.*` / `aiobotocore.*` qualnames (including
  `Session.client` / `Session.resource` and `async with` receivers) are
  classified as async counterparts (never findings). Sync `boto3-stubs.*` /
  `botocore-stubs.*` package roots normalize to `boto3` / `botocore`.
- `kafka-python` consumer local-state mutators and metrics snapshots do no
  broker RPC: `pause`/`resume`/`seek`/`assignment`/`subscription`/`paused`/
  `metrics`/`bootstrap_connected` on `KafkaConsumer`, and `metrics` /
  `bootstrap_connected` on `KafkaProducer` do not report.

## blocking-subprocess

`subprocess.run` / `Popen.wait` / `os.system` wait on the event-loop thread.

**Fix:** `asyncio.create_subprocess_*` and await communicate/wait, or offload.

## blocking-file

`open`, path read/write/mutation (`pathlib.Path.read_text` / `write_text` /
`mkdir` / `unlink` / `rename` / `touch`), `importlib.resources` /
`importlib_resources` `Traversable` reads (`files(...).joinpath(...).read_text` /
`read_bytes` / `open` / `iterdir`), directory walks (`os.listdir` /
`pathlib.Path.iterdir` / `glob`), archives (`zipfile` / `tarfile`), and
blocking `input()` on the loop.

**Fix:** async file helpers, or `await asyncio.to_thread(...)`.

**Notes:**

- Cheap idempotent path ops are `cheap-path-op` candidates, not findings:
  `Path.mkdir(exist_ok=True)`, `os.makedirs(exist_ok=True)`,
  `Path.unlink(missing_ok=True)` (flag must be a literal `True`, keyword or its
  stdlib positional slot) and `Path.touch()`. Plain `mkdir` / `makedirs` /
  `unlink`, `rename` / `replace`, and `os.remove` / `os.unlink` stay findings.
- `open(p).read()` in one expression is one finding (the `open(...)` hop),
  like `with open(p) as f: f.read()`.
- Paths through a `functools.cache` / `functools.lru_cache` decorated helper
  carry `cache_miss_dependent` (the body runs only on a cache miss); they stay
  findings.
- Project class instantiation (`Cls(...)`, `@dataclass` `__post_init__`,
  `__new__`, inherited `__init__`, `super().__init__`, and `@classmethod`
  `cls(...)` factories), `@property` / `@functools.cached_property` getters,
  and `@functools.cache` / `@lru_cache` sync helpers are followed into their
  bodies; `await asyncio.to_thread(Cls, ...)` /
  `run_in_executor(None, Cls, ...)` cuts the constructor call as offloaded.
- In-memory buffers (`io.BytesIO` / `StringIO`, including locals bound to them)
  are not file IO: `open`/`Image.open` and `zipfile`/`tarfile` opened on those
  buffers stay clean for pure in-memory ops (`writestr`/`read`/`namelist`/…).
  Disk-touching methods on a buffer-backed archive (`write(path)`, `extractall`,
  `add`, …) still report.
- `open`/`ZipFile` plus later `read`/`write`/`writestr` on the same call-path
  prefix merge into one finding whose `sinkSites` entry and `why` locator point
  at the `open` hop while retaining both hops in `chain[]` (so `async-doctor why`
  matches either line); `SMTP(...).sendmail(...)` on one line likewise.

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

Sync SQL, document, and KV database drivers (`sqlite3`, `psycopg` / `psycopg2`,
`pymysql`, `pymongo`, sync `redis`, sync `sqlalchemy`).

**Fix:** async driver/session (`AsyncSession`, `psycopg.AsyncConnection`,
`asyncpg`, `aiomysql`, `motor`, `redis.asyncio`), or
`await asyncio.to_thread(...)`.

**Notes:**

- `AsyncSession.run_sync` runs sync work on a greenlet in the event-loop thread
  — it is **not** a thread offload and is not treated as one.
- Pure helpers and handle getters do not report: `pymysql.err.*` /
  `pymysql.constants.*` / `pymysql.converters.*` / `escape_string` / `Binary`,
  and `pymongo` database/collection handle getters (`get_database`,
  `get_collection`, `with_options`, `codec_options`, `read_preference`,
  `write_concern`, `read_concern`) perform no wire IO.

## unresolved-call

Dynamic dispatch that could not be proven. Candidates only — never fails the
exit code by itself. Inspect, add types, or offload if the call is sync IO.

## offload-has-async-twin

**Advice (performance)** — never fails the exit code.

An offload boundary (`asyncio.to_thread`, `run_in_executor`,
`anyio.to_thread.run_sync`, `starlette.concurrency.run_in_threadpool`,
`asgiref.sync.sync_to_async`, `channels.db.database_sync_to_async`, inferred
wrappers) cuts a sync call that only
reaches sinks with a high-confidence same-package async twin
(`httpx.AsyncClient` including module-level `httpx.get`/`post`/…,
`redis.asyncio` including distributed locks, `openai.AsyncOpenAI`,
`openai.AsyncAzureOpenAI`, `anthropic.AsyncAnthropic`,
`elasticsearch.AsyncElasticsearch` needing `aiohttp`,
`psycopg.AsyncConnection`, `grpc.aio`, and `sqlalchemy.ext.asyncio.AsyncSession` /
`AsyncEngine` when `greenlet` plus an async SQL driver such as `asyncpg`,
`psycopg`, `aiosqlite`, `aiomysql`, or `asyncmy` are installed). The twin and
its `needs` / `any_needs` must resolve in the scanned venv. The offload site's
own try/if/loop conditions do not suppress advice. Skipped when the offloaded
function is also called from sync-only code, or when any reached sink is
excluded (pymongo, requests, boto3, file IO, subprocess, CPU work).

**Why:** the offload already avoids blocking the loop, but the async twin
removes the thread hop and keeps cancellation/context on the event loop.

**Fix:** call the async client/API directly from `async def` instead of
offloading the sync client.

**Notes:**

- Decorator offloads count as boundaries too: a function decorated with
  `@sync_to_async` / `@database_sync_to_async` (bare or called form), or with
  a project decorator whose returned nested def passes the wrapped callable
  into an offload primitive / inferred wrapper, runs its body on a worker —
  calls to it never reach its body on the caller's loop. Unknown or
  non-offloading decorators stay transparent.
- Named nested function bodies, like lambda bodies, are analyzed relative to
  their own execution: guards surrounding the definition do not make the
  body's sinks conditional. Conditions inside the body still apply, and
  statically dead definitions stay excluded.
- A wrapper that offloads a local closure capturing an enclosing callable
  parameter is not necessarily inferred as an offload wrapper. Missing advice
  on that shape does not prove there is no async alternative.

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
`Condition` / `TaskGroup` / `timeout`, `anyio.Lock` / `Semaphore` /
`Condition` / `CapacityLimiter` / `create_task_group`,
`sqlalchemy.ext.asyncio.AsyncSession` / `AsyncEngine.begin`,
`aioboto3.Session.client`, `redis.asyncio.Redis.pipeline`, or a project class
that defines `__aenter__` without `__enter__`). Entering an async-only context
manager with sync `with` raises `TypeError` at runtime. Sync anyio timeouts
(`fail_after`, `move_on_after`) are not flagged — they expose `__enter__` and
`with` is correct.

**Fix:** replace `with ...:` with `async with ...:`.

## sync-for-async-iterable

A synchronous `for` loop or sync comprehension iterates over an async-only
iterable (`async def` generator/coroutine call, known async stream such as
`httpx.Response.aiter_*`, `redis.asyncio.Redis.scan_iter`,
`redis.asyncio.PubSub.listen`, `aiohttp.StreamReader`, `asyncpg.Connection.cursor`,
`AsyncResult`, or a base-free project class implementing `__aiter__` without
`__iter__` or `__getitem__`). Sync iteration raises `TypeError` at runtime.

**Fix:** replace `for x in ...:` with `async for x in ...:` (or `[x async for x in ...]`).

## eager-offload-call

An offload helper (`asyncio.to_thread`, `loop.run_in_executor`,
`anyio.to_thread.run_sync`, `run_in_threadpool`, `sync_to_async`) receives a
call expression `fn(...)` in its callable slot (excluding `functools.partial`)
whose return value is provably not callable (`None`, literal/builtin scalar or
collection, coroutine/generator, or non-callable project class). Python
evaluates `fn(...)` eagerly on the event loop before the offload helper is
entered and then raises `TypeError` on the worker thread. Deduplicated when the
inner call already emits a `blocking-*` finding through the same offload site.

**Fix:** pass the callable and its arguments separately (`await asyncio.to_thread(fn, arg)`),
or wrap keyword arguments with `functools.partial(fn, key=val)`.

## unawaited-coroutine

A bare expression statement calls a proven coroutine-returning `async def`
without `await`, discarding the coroutine object immediately so the body never
executes (`RuntimeWarning: coroutine '...' was never awaited`). Excludes async
generators (`yield`), `@asynccontextmanager`, and custom decorators that may
alter call semantics.

**Fix:** `await fn(...)`, or schedule explicitly with `asyncio.TaskGroup` / a
referenced `asyncio.create_task(...)`.

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

## ephemeral-async-client

**Advice (performance)** — never fails the exit code.

`httpx.AsyncClient(...)` or `aiohttp.ClientSession(...)` is constructed and
closed (`async with` or local `.aclose()` / `.close()`) per `Route` invocation
or inside a loop, discarding connection pools and repeating DNS + TLS handshakes
on every call. Lifespan entries, one-shot non-route scripts without loops, lazy
singletons, and custom `transport=` / `mounts=` / `app=` / `connector=` wrappers
are excluded.

**Fix:** create one shared client in your application lifespan (or outside the
loop) and reuse it across requests.

## shared-async-session

**Advice (correctness)** — never fails the exit code.

A single stateful async database session or connection
(`sqlalchemy.ext.asyncio.AsyncSession` / `AsyncConnection`,
`sqlmodel.ext.asyncio.session.AsyncSession`, `asyncpg.Connection`,
`psycopg.AsyncConnection` / `AsyncCursor`) is shared across multiple concurrent
tasks in `asyncio.gather(...)` or `asyncio.TaskGroup` /
`anyio.create_task_group()`. Concurrent operations on one stateful session or
connection are not safe and fail with `InvalidRequestError` or protocol errors.
Concurrency-safe factories and pools (`AsyncEngine`, `async_sessionmaker`,
`asyncpg.Pool`, `AsyncConnectionPool`) and distinct per-task sessions are not
reported.

**Fix:** open a separate session/connection per concurrent task (via
`async_sessionmaker` or `pool.acquire()`), or `await` the operations sequentially.
