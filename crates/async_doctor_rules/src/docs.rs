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
    /// Diagnostic explanation and limits of the available evidence.
    pub body: &'static str,
    /// Proven local protocol correction, or empty when no edit is established.
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
    /// Omits the repair section when [`Self::fix`] is empty.
    pub fn explain_text(&self) -> String {
        let correction = if self.fix.is_empty() {
            String::new()
        } else {
            format!("\n\nHow to fix\n{}", self.fix)
        };
        format!(
            "{id} — {title}\n\n{summary}\n\nWhy it matters\n{body}{correction}\n\nDocs: {url}\n",
            id = self.id,
            title = self.title,
            summary = self.summary,
            body = self.body,
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
so other tasks sharing that thread are delayed for the duration of the sleep.\n\
This can increase latency and cause timeouts in otherwise unrelated work.",
        fix: "",
    },
    RuleDoc {
        id: "blocking-socket",
        title: "Blocking socket / DNS / SSL",
        summary: "Sync socket, DNS, `SSLContext.wrap_socket` / `SSLSocket`, or `smtplib` / `ftplib` stalls the loop.",
        body: "\
BSD sockets, blocking DNS, TLS handshakes/reads/writes, and sync `smtplib` /\n\
`ftplib` clients perform their waits on the calling thread. When reached on the\n\
event-loop thread, those waits prevent it from scheduling other tasks until\n\
the synchronous call returns.",
        fix: "",
    },
    RuleDoc {
        id: "blocking-http",
        title: "Blocking HTTP / RPC / cloud client",
        summary: "Sync HTTP, RPC, messaging, and cloud/AI/vector SDKs (`urllib`, `http.client`, `requests`, sync `httpx`, `boto3`, `openai`, `anthropic`, `groq`, `ollama`, `qdrant_client`, `elasticsearch`, `kafka`, `confluent_kafka`, sync `grpc`) block the loop.",
        body: "\
Network and service round-trips are often tens to hundreds of milliseconds. Sync\n\
HTTP clients, RPC stubs, and cloud/AI/search/vector/messaging SDKs hold the event\n\
loop for the duration of a blocking wait when called on its thread. Other tasks\n\
cannot make progress on that loop during the wait.",
        fix: "",
    },
    RuleDoc {
        id: "blocking-subprocess",
        title: "Blocking subprocess",
        summary: "`subprocess.run` / `Popen.wait` / `os.system` / `os.waitpid` block until exit.",
        body: "\
Waiting on a child process from the event-loop thread prevents other tasks from\n\
running for the duration of that blocking wait. The delay depends on the child's\n\
state and the operation being performed.",
        fix: "",
    },
    RuleDoc {
        id: "blocking-file",
        title: "Blocking file / path IO",
        summary: "`open`, `io` / `os` file IO, `pathlib.Path`, `importlib.resources`, `shutil`, walks, archives, and `input` block on IO.",
        body: "\
Disk IO (including `io.BufferedReader` / `FileIO`, `os.read` / `write` / `fsync`,\n\
`pathlib.Path` reads/writes/walks/mutations, `importlib.resources` `Traversable`\n\
reads, `shutil` copies/archives, `glob`, `zipfile` / `tarfile`, and blocking\n\
`input()`) can stall under load or on network filesystems. When performed on\n\
the event-loop thread, these operations delay every task sharing that loop\n\
until the synchronous call returns.",
        fix: "",
    },
    RuleDoc {
        id: "blocking-lock",
        title: "Blocking thread lock / queue / future",
        summary: "Threading locks, queues, `Future.result`/`exception`, and `Executor.map`/`wait`.",
        body: "\
A contended lock, blocking queue get, `concurrent.futures.Future.result()` or\n\
`exception()`, `Executor.map` consumption, or `wait()` on the event-loop thread\n\
freezes every task sharing the loop while the wait lasts. Actual delay depends\n\
on contention, queue state, and whether the awaited thread work is complete.\n\
Completed Future reads and literal nonpositive-timeout polls do not wait for\n\
completion; this does not make their receiver or argument evaluation safe.",
        fix: "",
    },
    RuleDoc {
        id: "blocking-db",
        title: "Blocking database / cache driver",
        summary: "Sync SQL, document, and KV database drivers (`sqlite3`, `psycopg`, `pymysql`, `pymongo`, sync `redis` / `RedisCluster`, sync `valkey`, sync `sqlalchemy` / `sqlmodel`) block on queries.",
        body: "\
Sync SQL, MongoDB, and Redis/Valkey drivers perform wire or disk IO on the calling\n\
thread. When reached on the event-loop thread, that work prevents other tasks\n\
from running for its duration. Stateful sessions and connections also carry\n\
transaction state and thread or task ownership constraints; the blocking\n\
diagnosis does not establish a safe driver, ownership, or transaction change.",
        fix: "",
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
        fix: "",
    },
    RuleDoc {
        id: "unresolved-call",
        title: "Unresolved call (candidate only)",
        summary: "Dynamic dispatch could not be proven; reported as a candidate.",
        body: "\
When the callee cannot be resolved statically, async-doctor does not invent a\n\
finding. Candidates need human or agent judgment and never fail the exit code\n\
by themselves.",
        fix: "",
    },
    RuleDoc {
        id: "offload-has-async-twin",
        title: "Offload has an async twin (advice)",
        summary: "Offloaded sync client work has a same-package async twin available.",
        body: "\
The call is already behind an offload cut, so this rule does not report blocking\n\
work on the loop at that site. Every unconditionally reached sink has a\n\
high-confidence same-package async twin available in the scanned venv, with\n\
its dependency requirements resolved. This records API availability, not\n\
semantic equivalence or a recommendation to change clients or remove the\n\
offload. It is advice only and never fails the exit code.",
        fix: "",
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
        fix: "",
    },
    RuleDoc {
        id: "offload-coroutine",
        title: "Offload of a coroutine function",
        summary: "Offload target creates a coroutine object instead of executing its body in the worker.",
        body: "\
`to_thread` / `run_in_executor` / similar APIs invoke a sync callable in a worker.\n\
Passing a coroutine function (`async def` without `yield`) only creates a coroutine\n\
object; the offload does not await it. Async-generator functions are excluded:\n\
they create async iterators whose bodies execute during later async iteration,\n\
not during offload.",
        fix: "",
    },
    RuleDoc {
        id: "sync-with-async-cm",
        title: "Sync with on an async-only context manager",
        summary: "Sync `with` enters an async-only context manager (`__aenter__` without `__enter__`).",
        body: "\
Async-only context managers (`httpx.AsyncClient`, `aiohttp.ClientSession`,\n\
`asyncio.Lock` / `Semaphore` / `TaskGroup` / `timeout`, `AsyncSession`, or classes\n\
that implement `__aenter__` without `__enter__`) do not implement the synchronous\n\
context protocol and raise `TypeError` at runtime when entered with `with`.\n\
A coroutine object is not a context manager; those sites report\n\
`unawaited-coroutine`, not this async-only protocol rule.",
        fix: "\
- Where async syntax is already legal, use `async with ...:` on the same object\n\
  to invoke its supported async context protocol. This addresses only the\n\
  protocol mismatch; it does not authorize converting the containing API or\n\
  establish the surrounding resource lifetime or control flow.",
    },
    RuleDoc {
        id: "sync-for-async-iterable",
        title: "Sync iteration over an async-only iterable",
        summary: "Sync `for` or comprehension iterates over an async-only iterable (`__aiter__` without `__iter__`).",
        body: "\
Async generators (`async def` with `yield`), async streams (`httpx.Response.aiter_*`,\n\
`redis.asyncio.Redis.scan_iter`, `aiohttp.StreamReader`, `aiokafka.AIOKafkaConsumer`),\n\
and classes implementing `__aiter__` without `__iter__` or `__getitem__` cannot be\n\
iterated synchronously and raise `TypeError` at runtime. Coroutine objects do\n\
not implement either iteration protocol; those sites report `unawaited-coroutine`.",
        fix: "\
- Where async syntax is already legal, use `async for ... in ...:` or an async\n\
  comprehension (`[... async for ... in ...]`) on the same object to invoke its\n\
  supported async iteration protocol. This addresses only the protocol mismatch;\n\
  it does not authorize converting the containing API or establish control flow.",
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
        fix: "",
    },
    RuleDoc {
        id: "unawaited-coroutine",
        title: "Unawaited coroutine object",
        summary: "A coroutine is discarded or used directly as an iteration/context protocol without awaiting its result.",
        body: "\
Calling an `async def` coroutine function as a standalone statement only constructs\n\
a coroutine object and immediately drops it. The body never executes, and Python\n\
emits `RuntimeWarning: coroutine '...' was never awaited` when garbage-collected.\n\
Using that object directly in a loop, comprehension, or `with` raises `TypeError`.\n\
`async for` / `async with` do not implicitly await a coroutine to obtain an\n\
iterable or context manager. The diagnostic does not establish the intended\n\
scheduling or resource lifetime.",
        fix: "",
    },
    RuleDoc {
        id: "dangling-task",
        title: "Fire-and-forget task without a stored reference (advice)",
        summary: "Bare `asyncio.create_task` / `ensure_future` discards the returned `Task`.",
        body: "\
The asyncio event loop only holds weak references to scheduled tasks. A task\n\
created via a bare `asyncio.create_task(coro())` statement with no variable\n\
reference can be garbage-collected mid-execution, and unhandled task exceptions\n\
can go unobserved. The discarded handle does not establish the intended\n\
background-task ownership or lifetime. This is correctness advice and never\n\
fails the exit code.",
        fix: "",
    },
    RuleDoc {
        id: "ephemeral-async-client",
        title: "Per-request or per-iteration async HTTP client (advice)",
        summary: "`httpx.AsyncClient` / `aiohttp.ClientSession` is created and torn down per route call or loop turn.",
        body: "\
Opening and closing a fresh `httpx.AsyncClient` or `aiohttp.ClientSession` on every\n\
route invocation or loop iteration prevents reuse of that client's connection\n\
pool across those calls. Repeated connections can add TCP/TLS handshake work and\n\
port pressure under load. The scan does not measure connection reuse or\n\
establish the appropriate client lifetime. This is performance advice and never\n\
fails the exit code.",
        fix: "",
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
        fix: "",
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
passes an existing handle across a thread boundary. That call does not establish\n\
the handle's origin thread, overlapping use, or intended transaction ownership.\n\
This is correctness advice and never fails the exit code.",
        fix: "",
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
        fix: "",
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
and can interfere with enclosing timeouts and `TaskGroup`s. Top-level `Task` and `Lifecycle`\n\
entries and `Task.uncancel()` scopes are excluded. This is correctness advice and\n\
never fails the exit code.",
        fix: "",
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
    fn explain_keeps_diagnosis_and_url_without_an_unsupported_fix() {
        let text = rule_doc("blocking-sleep").unwrap().explain_text();
        assert!(text.contains("time.sleep"));
        assert!(text.contains("Why it matters"));
        assert!(!text.contains("How to fix"));
        assert!(text.contains("#blocking-sleep"));
        assert!(
            text.contains("https://github.com/maolonglong/async-doctor/blob/main/docs/rules.md")
        );
    }

    #[test]
    fn only_async_only_protocol_rules_offer_local_corrections() {
        for doc in RULE_DOCS {
            assert!(!doc.summary.is_empty(), "{} needs a summary", doc.id);
            assert!(!doc.body.is_empty(), "{} needs an explanation", doc.id);
            let text = doc.explain_text();
            let protocol = match doc.id {
                "sync-with-async-cm" => "async with",
                "sync-for-async-iterable" => "async for",
                _ => {
                    assert!(doc.fix.is_empty(), "{} must not prescribe a repair", doc.id);
                    assert!(!text.contains("How to fix"), "{}: {text}", doc.id);
                    continue;
                }
            };
            assert!(doc.fix.contains(protocol), "{}: {}", doc.id, doc.fix);
            assert!(doc.fix.contains("same object"), "{}: {}", doc.id, doc.fix);
            assert!(text.contains("How to fix"), "{}: {text}", doc.id);
        }
    }
}
