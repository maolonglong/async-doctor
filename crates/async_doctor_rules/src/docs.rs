//! Long-form rule help for `async-doctor explain` and docs anchors.
//!
//! Absolute docs URLs point at the public GitHub tree so installed binaries
//! still have a clickable link.

/// Canonical rules document on GitHub (main branch).
pub const DOCS_RULES_BASE: &str =
    "https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md";

/// Stable documentation for one built-in rule.
#[derive(Debug, Clone, Copy)]
pub struct RuleDoc {
    pub id: &'static str,
    pub title: &'static str,
    /// One-line summary for agents and compact CLI output.
    pub summary: &'static str,
    /// Longer explanation: why it blocks the event loop and how to fix it.
    pub body: &'static str,
    /// Typical fix patterns (async twin, `to_thread`, client swap).
    pub fix: &'static str,
}

impl RuleDoc {
    /// Markdown / URL fragment, e.g. `#blocking-sleep`.
    pub fn docs_anchor(&self) -> String {
        format!("#{}", self.id)
    }

    /// Absolute docs URL for agents and installed binaries.
    pub fn docs_url(&self) -> String {
        format!("{DOCS_RULES_BASE}{}", self.docs_anchor())
    }

    /// Full text printed by `async-doctor explain <rule-id>`.
    pub fn explain_text(&self) -> String {
        format!(
            "{id} — {title}\n\n{summary}\n\nWhy it matters\n{body}\n\nHow to fix\n{fix}\n\nDocs: {url}\n",
            id = self.id,
            title = self.title,
            summary = self.summary,
            body = self.body,
            fix = self.fix,
            url = self.docs_url(),
        )
    }
}

/// Built-in rules that agents and `explain` can look up.
pub static RULE_DOCS: &[RuleDoc] = &[
    RuleDoc {
        id: "blocking-sleep",
        title: "Blocking sleep on the event loop",
        summary: "`time.sleep` (and similar) freezes every task sharing the loop.",
        body: "\
While `time.sleep` runs, the asyncio event loop cannot schedule other coroutines,\n\
so latency spikes and timeouts pile up across the process. Prefer the async twin\n\
`asyncio.sleep`, or offload truly sync work with `asyncio.to_thread`.",
        fix: "\
- Await `asyncio.sleep(...)` instead of `time.sleep(...)`.\n\
- Or offload: `await asyncio.to_thread(time.sleep, seconds)`.",
    },
    RuleDoc {
        id: "blocking-socket",
        title: "Blocking socket / DNS / SSL",
        summary: "Sync socket, DNS, `SSLContext.wrap_socket` / `SSLSocket`, or `smtplib` / `ftplib` stalls the loop.",
        body: "\
BSD sockets, blocking DNS, TLS handshakes/reads/writes, and sync `smtplib` /\n\
`ftplib` clients resolve on the calling thread. Inside `async def`, that thread\n\
is the event loop. Use asyncio helpers / `aiosmtplib`, or run the sync call in\n\
a worker thread.",
        fix: "\
- Prefer `asyncio.open_connection(..., ssl=...)` / asyncio servers / `loop.getaddrinfo`.\n\
- Or `await asyncio.to_thread(socket.create_connection, ...)`.",
    },
    RuleDoc {
        id: "blocking-http",
        title: "Blocking HTTP / RPC / cloud client",
        summary: "Sync HTTP, RPC, messaging, and cloud/AI/vector SDKs (`urllib`, `http.client`, `requests`, sync `httpx`, `boto3`, `openai`, `anthropic`, `groq`, `ollama`, `qdrant_client`, `elasticsearch`, `kafka`, `confluent_kafka`, sync `grpc`) block the loop.",
        body: "\
Network and service round-trips are often tens to hundreds of milliseconds. Sync\n\
HTTP clients, RPC stubs, and cloud/AI/search/vector/messaging SDKs hold the event\n\
loop for the whole wait. Switch to the async client twin, or offload the call.",
        fix: "\
- Use `httpx.AsyncClient`, `aiohttp`, `AsyncOpenAI`, `AsyncAnthropic` /\n\
  `AsyncAnthropicBedrock` / `AsyncAnthropicVertex`, `AsyncGroq`, `ollama.AsyncClient`,\n\
  `AsyncQdrantClient`, `AsyncElasticsearch`, `aioboto3`, `aiokafka`, or `grpc.aio`.\n\
- Or `await asyncio.to_thread(requests.get, url)`.",
    },
    RuleDoc {
        id: "blocking-subprocess",
        title: "Blocking subprocess",
        summary: "`subprocess.run` / `Popen.wait` / `os.system` / `os.waitpid` block until exit.",
        body: "\
Waiting on a child process from the event-loop thread prevents other tasks from\n\
running for the entire child lifetime.",
        fix: "\
- Prefer `asyncio.create_subprocess_exec` / `create_subprocess_shell` and await\n\
  `communicate` / `wait`.\n\
- Or offload the sync subprocess helper with `asyncio.to_thread`.",
    },
    RuleDoc {
        id: "blocking-file",
        title: "Blocking file / path IO",
        summary: "`open`, `io` / `os` file IO, `pathlib.Path`, `importlib.resources`, `shutil`, walks, archives, and `input` block on IO.",
        body: "\
Disk IO (including `io.BufferedReader` / `FileIO`, `os.read` / `write` / `fsync`,\n\
`pathlib.Path` reads/writes/walks/mutations, `importlib.resources` `Traversable`\n\
reads, `shutil` copies/archives, `glob`, `zipfile` / `tarfile`, and blocking\n\
`input()`) can stall under load or on network filesystems. Inside `async def`,\n\
use async helpers or offload sync calls.",
        fix: "\
- Use `aiofiles` / AnyIO file APIs where practical.\n\
- Or `await asyncio.to_thread(path.read_text)` / `to_thread(open, ...)`.",
    },
    RuleDoc {
        id: "blocking-lock",
        title: "Blocking thread lock / queue / future",
        summary: "Threading locks, queues, `Future.result`, and `Executor.map`/`wait`.",
        body: "\
A contended lock, blocking queue get, `concurrent.futures.Future.result()`,\n\
`Executor.map` consumption, or `wait()` on the event-loop thread freezes every\n\
task. Prefer asyncio primitives, or confine blocking waits to worker threads.",
        fix: "\
- Prefer `asyncio.Lock` / `Event` / `Queue`.\n\
- Await `asyncio.wrap_future` instead of `Future.result()`.\n\
- Hold threading locks only inside `to_thread` workers.",
    },
    RuleDoc {
        id: "blocking-db",
        title: "Blocking database / cache driver",
        summary: "Sync SQL, document, and KV database drivers (`sqlite3`, `psycopg`, `pymysql`, `pymongo`, sync `redis` / `RedisCluster`, sync `valkey`, sync `sqlalchemy` / `sqlmodel`) block on queries.",
        body: "\
Sync SQL, MongoDB, and Redis/Valkey drivers perform wire or disk IO on the calling\n\
thread. From `async def`, that blocks the event loop for every query or command.\n\
Prefer async drivers/sessions, or offload a self-contained sync unit-of-work\n\
function that opens, commits, and closes its own `Session` or `Connection` inside\n\
the worker thread. Never dispatch bound methods of a shared/request sync\n\
`Session` or `sqlite3.Connection` (`check_same_thread=True`) across pool threads.",
        fix: "\
- Switch to an async driver or session (`AsyncSession`, `sqlmodel` `AsyncSession`,\n\
  `aiosqlite`, `psycopg.AsyncConnection`, `asyncpg`, `aiomysql`, `motor`,\n\
  `pymongo.AsyncMongoClient`, `redis.asyncio`, `valkey.asyncio.Valkey`).\n\
- Or offload a self-contained sync unit-of-work function: `await asyncio.to_thread(run_sync_query, ...)`\n\
  where `run_sync_query` opens, commits, and closes its own `Session` / `Connection` in the worker thread.",
    },
    RuleDoc {
        id: "blocking-cpu",
        title: "Slow CPU-bound password KDF (candidate only)",
        summary: "Password KDFs (`bcrypt`, `argon2`, `hashlib.pbkdf2_hmac`, `hashlib.scrypt`) are intentionally slow; reported as candidates.",
        body: "\
Password hashing functions are tuned to burn tens to hundreds of milliseconds of\n\
CPU per call while holding or contending on the event-loop thread. Because cost\n\
parameters and call frequency vary by environment, async-doctor reports them\n\
strictly as `slow-kdf` candidates — never findings, and never failing the exit\n\
code by default.",
        fix: "\
- Offload password hashing/verification with `await asyncio.to_thread(bcrypt.checkpw, ...)`\n\
  or `await asyncio.to_thread(ph.verify, ...)` (or a `ProcessPoolExecutor` if GIL-bound).",
    },
    RuleDoc {
        id: "unresolved-call",
        title: "Unresolved call (candidate only)",
        summary: "Dynamic dispatch could not be proven; reported as a candidate.",
        body: "\
When the callee cannot be resolved statically, async-doctor does not invent a\n\
finding. Candidates need human or agent judgment and never fail the exit code\n\
by themselves.",
        fix: "\
- Inspect the call; add types or simplify indirection so the sink is visible.\n\
- If it is truly sync IO, await an async API or offload with `to_thread`.",
    },
    RuleDoc {
        id: "offload-has-async-twin",
        title: "Offload has an async twin (advice)",
        summary: "Offloaded sync client work has a same-package async twin available.",
        body: "\
The call is already behind an offload cut, so it does not block the loop. When\n\
every unconditionally reached sink has a high-confidence same-package async twin\n\
installed in the scanned venv, prefer the async API to avoid the thread hop and\n\
keep cancellation on the loop. This is advice only — it never fails the exit code.\n\
Explicit exception to design decision #2 (twins as fix hints only): advice fires\n\
at cut offload sites so nothing is double-reported with blocking findings.",
        fix: "\
- Replace the sync client with its async twin (`httpx.AsyncClient`,\n\
  `redis.asyncio`, `valkey.asyncio.Valkey`, `AsyncOpenAI`, `AsyncAnthropic` /\n\
  `AsyncAnthropicBedrock` / `AsyncAnthropicVertex`, `AsyncGroq`,\n\
  `ollama.AsyncClient`, `AsyncQdrantClient`, `AsyncElasticsearch`,\n\
  `psycopg.AsyncConnection`, `grpc.aio`, `AsyncSession` / `AsyncEngine`)\n\
  and await it directly.\n\
- Keep the offload if the function must also run from sync-only callers.",
    },
    RuleDoc {
        id: "nested-event-loop",
        title: "Nested event loop",
        summary: "`asyncio.run` / `run_until_complete` / `anyio.run` / `trio.run` inside async code.",
        body: "\
These APIs expect to own the event loop. Calling them from a running loop raises\n\
`RuntimeError` at runtime. Offloaded (thread) calls are not reported.\n\
\n\
Sites under a proven \"no running loop\" guard are dropped (not even candidates):\n\
`if loop is None`, early-return `if loop is not None: return`, `except RuntimeError` after `get_running_loop` / `get_event_loop`,\n\
`not loop.is_running()`, and similar helpers. Other conditional nested-loop sites\n\
remain findings with normal branch conditions.",
        fix: "\
- Await the coroutine directly instead of nesting a runner.\n\
- Or confine a sync nested-run helper to `asyncio.to_thread` if you must.",
    },
    RuleDoc {
        id: "offload-coroutine",
        title: "Offload of async def",
        summary: "Offload target is an `async def` — the coroutine never runs.",
        body: "\
`to_thread` / `run_in_executor` / similar APIs invoke a sync callable in a worker.\n\
Passing an `async def` creates a coroutine object that is never awaited, so the\n\
work does not execute.",
        fix: "\
- Await the coroutine directly from async code.\n\
- Or pass a sync function into the offload.",
    },
    RuleDoc {
        id: "sync-with-async-cm",
        title: "Sync with on an async-only context manager",
        summary: "Sync `with` enters an async-only context manager (`__aenter__` without `__enter__`).",
        body: "\
Async-only context managers (`httpx.AsyncClient`, `aiohttp.ClientSession`,\n\
`asyncio.Lock` / `Semaphore` / `TaskGroup` / `timeout`, `AsyncSession`, or classes\n\
that implement `__aenter__` without `__enter__`) do not implement the synchronous\n\
context protocol and raise `TypeError` at runtime when entered with `with`.",
        fix: "\
- Use `async with ...:` instead of `with ...:`.",
    },
    RuleDoc {
        id: "sync-for-async-iterable",
        title: "Sync iteration over an async-only iterable",
        summary: "Sync `for` or comprehension iterates over an async-only iterable (`__aiter__` without `__iter__`).",
        body: "\
Async generators (`async def` with `yield`), async streams (`httpx.Response.aiter_*`,\n\
`redis.asyncio.Redis.scan_iter`, `aiohttp.StreamReader`, `aiokafka.AIOKafkaConsumer`),\n\
and classes implementing `__aiter__` without `__iter__` or `__getitem__` cannot be\n\
iterated synchronously and raise `TypeError` at runtime.",
        fix: "\
- Use `async for ... in ...:` (or an async comprehension `[... async for ... in ...]`).",
    },
    RuleDoc {
        id: "eager-offload-call",
        title: "Eager call inside offload boundary",
        summary: "Offload receives `fn(...)` instead of `fn`, running `fn` on the loop and passing a non-callable result.",
        body: "\
`asyncio.to_thread`, `loop.run_in_executor`, `anyio.to_thread.run_sync`,\n\
`run_in_threadpool`, and `sync_to_async` evaluate their arguments before dispatching\n\
to a worker thread. Writing `to_thread(fn(x))` runs `fn(x)` synchronously on the\n\
event loop and then passes its non-callable return value to the worker, raising\n\
`TypeError` at runtime.",
        fix: "\
- Pass the callable and its arguments separately: `await asyncio.to_thread(fn, x)`.\n\
- For keyword arguments, use `functools.partial(fn, x, k=v)` or `lambda: fn(x, k=v)`.",
    },
    RuleDoc {
        id: "unawaited-coroutine",
        title: "Unawaited coroutine call",
        summary: "Bare coroutine call discards the returned coroutine without awaiting or scheduling it.",
        body: "\
Calling an `async def` coroutine function as a standalone statement only constructs\n\
a coroutine object and immediately drops it. The body never executes, and Python\n\
emits `RuntimeWarning: coroutine '...' was never awaited` when garbage-collected.",
        fix: "\
- Prefix the call with `await`: `await coro(...)`.\n\
- Or schedule it explicitly with `asyncio.TaskGroup` / `asyncio.create_task` (keeping a reference).",
    },
    RuleDoc {
        id: "dangling-task",
        title: "Fire-and-forget task without a stored reference (advice)",
        summary: "Bare `asyncio.create_task` / `ensure_future` discards the returned `Task`.",
        body: "\
The asyncio event loop only holds weak references to scheduled tasks. A task\n\
created via a bare `asyncio.create_task(coro())` statement with no variable\n\
reference can be garbage-collected mid-execution, and unhandled task exceptions\n\
go unobserved. This is correctness advice and never fails the exit code.",
        fix: "\
- Await the task or `asyncio.TaskGroup` when structured concurrency fits.\n\
- For intentional background tasks, store the task in a set (`bg_tasks.add(t)`)\n\
  and register `t.add_done_callback(bg_tasks.discard)`.",
    },
    RuleDoc {
        id: "ephemeral-async-client",
        title: "Per-request or per-iteration async HTTP client (advice)",
        summary: "`httpx.AsyncClient` / `aiohttp.ClientSession` is created and torn down per route call or loop turn.",
        body: "\
Opening and closing a fresh `httpx.AsyncClient` or `aiohttp.ClientSession` on every\n\
route invocation or loop iteration discards connection pooling, keep-alive sockets,\n\
and TLS session reuse — forcing a new TCP/TLS handshake each time and risking\n\
ephemeral port exhaustion under load. This is performance advice and never fails\n\
the exit code.",
        fix: "\
- Initialize a shared client once in application lifespan / startup and reuse it\n\
  across requests or loop iterations, closing it on shutdown.",
    },
    RuleDoc {
        id: "shared-async-session",
        title: "Concurrent sharing of a stateful async DB session (advice)",
        summary: "A single `AsyncSession` or async DB connection is passed to multiple concurrent tasks.",
        body: "\
`sqlalchemy.ext.asyncio.AsyncSession`, `AsyncConnection`, `asyncpg.Connection`, and\n\
`psycopg.AsyncConnection` hold mutable transaction and protocol state on a single\n\
connection and are not safe for concurrent use across `asyncio.gather` or\n\
`TaskGroup` tasks (raising `InvalidRequestError` or corrupting protocol state at\n\
runtime). This is correctness advice and never fails the exit code.",
        fix: "\
- Open a separate `AsyncSession` / connection per concurrent task from a shared\n\
  `async_sessionmaker` / connection pool, or await the queries sequentially.",
    },
    RuleDoc {
        id: "offload-stateful-session",
        title: "Offload of a bound stateful sync DB session or connection method (advice)",
        summary: "A bound method of `sqlalchemy.orm.Session`, `sqlmodel.Session`, or `sqlite3.Connection` / `Cursor` is dispatched directly to a worker thread.",
        body: "\
Sync `sqlalchemy.orm.Session` and `sqlmodel.Session` instances hold mutable\n\
identity-map and transaction state that is not thread-safe across pool threads,\n\
and `sqlite3.Connection` / `sqlite3.Cursor` default to `check_same_thread=True`\n\
(raising `ProgrammingError` when used on a different thread than the one that\n\
opened the connection). Dispatching `asyncio.to_thread(session.execute, ...)`\n\
crosses thread boundaries on an existing handle instead of isolating the unit of\n\
work inside the worker thread. This is correctness advice and never fails the\n\
exit code.",
        fix: "\
- Offload a self-contained sync unit-of-work function (`await asyncio.to_thread(run_query, engine, ...)`)\n\
  that opens, commits, and closes its own `Session` or `Connection` inside the worker thread.\n\
- Or migrate to `sqlalchemy.ext.asyncio.AsyncSession` / `aiosqlite`.",
    },
    RuleDoc {
        id: "useless-local-async-lock",
        title: "Unshared per-call async lock or semaphore (advice)",
        summary: "`asyncio.Lock` / `Semaphore` is freshly constructed per call inside `async def` without sharing across tasks or closures.",
        body: "\
Constructing `asyncio.Lock()`, `asyncio.Semaphore(...)`, `asyncio.BoundedSemaphore(...)`,\n\
or `anyio.Lock()` / `Semaphore(...)` / `CapacityLimiter(...)` inside an `async def`\n\
and only entering it locally (`async with asyncio.Lock():` or an unescaped local\n\
`lock = asyncio.Lock(); async with lock:`) creates a distinct primitive on every\n\
call. No concurrent caller can contend on that instance, so mutual exclusion or\n\
concurrency limiting is a no-op. Locks shared with nested closures or passed to\n\
helper tasks are excluded. This is correctness advice and never fails the exit code.",
        fix: "\
- Store the `Lock` / `Semaphore` on a shared instance (`self._lock` in `__init__`),\n\
  lifespan state, or module scope so concurrent coroutines contend on the same primitive.",
    },
    RuleDoc {
        id: "swallowed-cancelled-error",
        title: "Swallowed asyncio.CancelledError inside async def (advice)",
        summary: "`except asyncio.CancelledError:` (or bare `except:` / `except BaseException:` over an `await`) exits without re-raising.",
        body: "\
In Python 3.8+, `asyncio.CancelledError` inherits from `BaseException` to drive\n\
structured cancellation (`asyncio.timeout`, `asyncio.wait_for`, `asyncio.TaskGroup`,\n\
and client disconnect cleanup). Catching `CancelledError` (or bare `except:` /\n\
`except BaseException:` without an earlier `CancelledError` re-raise) and returning\n\
or falling through without `raise` leaves the task's cancellation count un-cleared\n\
and breaks enclosing timeouts and `TaskGroup`s. Top-level `Task` and `Lifecycle`\n\
entries and `Task.uncancel()` scopes are excluded. This is correctness advice and\n\
never fails the exit code.",
        fix: "\
- Run cleanup in `finally:` or re-raise `raise` at the end of `except asyncio.CancelledError:`.\n\
- Catch `except Exception:` instead of bare `except:` / `except BaseException:` when you only want application errors.",
    },
];

/// Look up a rule by id (`blocking-sleep`) or docs anchor (`#blocking-sleep`).
pub fn rule_doc(id_or_anchor: &str) -> Option<&'static RuleDoc> {
    let id = id_or_anchor.strip_prefix('#').unwrap_or(id_or_anchor);
    RULE_DOCS.iter().find(|r| r.id == id)
}

/// Docs anchor string for a rule id (always `#<id>`, even if unknown).
pub fn docs_anchor_for(rule: &str) -> String {
    match rule_doc(rule) {
        Some(doc) => doc.docs_anchor(),
        None => format!("#{rule}"),
    }
}

/// Absolute docs URL for a rule id.
pub fn docs_url_for(rule: &str) -> String {
    match rule_doc(rule) {
        Some(doc) => doc.docs_url(),
        None => format!("{DOCS_RULES_BASE}#{rule}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stdlib_rule_constant_has_docs() {
        use crate::{
            RULE_BLOCKING_CPU, RULE_BLOCKING_DB, RULE_BLOCKING_FILE, RULE_BLOCKING_HTTP,
            RULE_BLOCKING_LOCK, RULE_BLOCKING_SLEEP, RULE_BLOCKING_SOCKET,
            RULE_BLOCKING_SUBPROCESS, RULE_DANGLING_TASK, RULE_EAGER_OFFLOAD_CALL,
            RULE_EPHEMERAL_ASYNC_CLIENT, RULE_NESTED_EVENT_LOOP, RULE_OFFLOAD_COROUTINE,
            RULE_OFFLOAD_HAS_ASYNC_TWIN, RULE_OFFLOAD_STATEFUL_SESSION, RULE_SHARED_ASYNC_SESSION,
            RULE_SWALLOWED_CANCELLED_ERROR, RULE_SYNC_FOR_ASYNC_ITERABLE, RULE_SYNC_WITH_ASYNC_CM,
            RULE_UNAWAITED_COROUTINE, RULE_USELESS_LOCAL_ASYNC_LOCK, STDLIB_SINKS,
            THIRD_PARTY_SINKS,
        };
        for id in [
            RULE_BLOCKING_SLEEP,
            RULE_BLOCKING_SOCKET,
            RULE_BLOCKING_HTTP,
            RULE_BLOCKING_SUBPROCESS,
            RULE_BLOCKING_FILE,
            RULE_BLOCKING_LOCK,
            RULE_BLOCKING_DB,
            RULE_BLOCKING_CPU,
            RULE_OFFLOAD_HAS_ASYNC_TWIN,
            RULE_NESTED_EVENT_LOOP,
            RULE_OFFLOAD_COROUTINE,
            RULE_SYNC_WITH_ASYNC_CM,
            RULE_SYNC_FOR_ASYNC_ITERABLE,
            RULE_EAGER_OFFLOAD_CALL,
            RULE_UNAWAITED_COROUTINE,
            RULE_DANGLING_TASK,
            RULE_EPHEMERAL_ASYNC_CLIENT,
            RULE_SHARED_ASYNC_SESSION,
            RULE_OFFLOAD_STATEFUL_SESSION,
            RULE_USELESS_LOCAL_ASYNC_LOCK,
            RULE_SWALLOWED_CANCELLED_ERROR,
        ] {
            assert!(rule_doc(id).is_some(), "missing RuleDoc for {id}");
        }
        for sink in STDLIB_SINKS.iter().chain(THIRD_PARTY_SINKS.iter()) {
            assert!(
                rule_doc(sink.rule).is_some(),
                "sink {} references undocumented rule {}",
                sink.qualname,
                sink.rule
            );
        }

        let rules_md = include_str!("../../../docs/rules.md");
        assert!(
            rules_md.lines().count() <= 500,
            "docs/rules.md must stay <= 500 lines"
        );
        for doc in RULE_DOCS {
            let heading = format!("## {}", doc.id);
            assert!(
                rules_md.lines().any(|line| line.trim() == heading),
                "docs/rules.md missing heading `{heading}`"
            );
        }
    }

    #[test]
    fn explain_mentions_fix_and_absolute_url() {
        let text = rule_doc("blocking-sleep").unwrap().explain_text();
        assert!(text.contains("asyncio.sleep"));
        assert!(text.contains("#blocking-sleep"));
        assert!(
            text.contains("https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md")
        );
    }
}
