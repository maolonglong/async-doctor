//! Stdlib sleep, subprocess, lock/future, sqlite3, and slow-KDF sinks.

use crate::{
    BuiltinSink, RULE_BLOCKING_CPU, RULE_BLOCKING_DB, RULE_BLOCKING_LOCK, RULE_BLOCKING_SLEEP,
    RULE_BLOCKING_SUBPROCESS,
};

const SQLITE_FIX_HINT: &str = "Use `aiosqlite`, or offload a self-contained sync function that opens and closes its own `sqlite3.Connection` inside the worker thread.";

pub(super) const RUNTIME_SINKS: &[BuiltinSink] = &[
    // --- sleep ---
    BuiltinSink::finding(
        "time.sleep",
        RULE_BLOCKING_SLEEP,
        "Replace with `await asyncio.sleep(...)` or offload via `asyncio.to_thread`.",
    ),
    // --- subprocess / os.system / os.waitpid (never subprocess.Popen constructor) ---
    BuiltinSink::finding(
        "subprocess.run",
        RULE_BLOCKING_SUBPROCESS,
        "Use `asyncio.create_subprocess_exec` / `create_subprocess_shell`, or offload.",
    ),
    BuiltinSink::finding(
        "subprocess.call",
        RULE_BLOCKING_SUBPROCESS,
        "Use `asyncio.create_subprocess_*`, or offload.",
    ),
    BuiltinSink::finding(
        "subprocess.check_call",
        RULE_BLOCKING_SUBPROCESS,
        "Use `asyncio.create_subprocess_*`, or offload.",
    ),
    BuiltinSink::finding(
        "subprocess.check_output",
        RULE_BLOCKING_SUBPROCESS,
        "Use `asyncio.create_subprocess_*`, or offload.",
    ),
    BuiltinSink::finding(
        "subprocess.getoutput",
        RULE_BLOCKING_SUBPROCESS,
        "Use `asyncio.create_subprocess_*`, or offload.",
    ),
    BuiltinSink::finding(
        "subprocess.getstatusoutput",
        RULE_BLOCKING_SUBPROCESS,
        "Use `asyncio.create_subprocess_*`, or offload.",
    ),
    BuiltinSink::finding(
        "subprocess.Popen.wait",
        RULE_BLOCKING_SUBPROCESS,
        "Use `asyncio.create_subprocess_*` and await communicate/wait, or offload.",
    ),
    BuiltinSink::finding(
        "subprocess.Popen.communicate",
        RULE_BLOCKING_SUBPROCESS,
        "Use `asyncio.create_subprocess_*` and await communicate, or offload.",
    ),
    BuiltinSink::finding(
        "os.system",
        RULE_BLOCKING_SUBPROCESS,
        "Use `asyncio.create_subprocess_shell`, or offload.",
    ),
    BuiltinSink::finding(
        "os.popen",
        RULE_BLOCKING_SUBPROCESS,
        "Use `asyncio.create_subprocess_*`, or offload.",
    ),
    BuiltinSink::finding(
        "os.waitpid",
        RULE_BLOCKING_SUBPROCESS,
        "Use `asyncio.create_subprocess_*` and `await proc.wait()`, or offload.",
    ),
    // --- threading / queue / concurrent.futures ---
    BuiltinSink::finding(
        "threading.Lock.acquire",
        RULE_BLOCKING_LOCK,
        "Prefer `asyncio.Lock`, or hold locks only inside `to_thread` workers.",
    ),
    BuiltinSink::finding(
        "threading.RLock.acquire",
        RULE_BLOCKING_LOCK,
        "Prefer `asyncio.Lock`, or hold locks only inside `to_thread` workers.",
    ),
    BuiltinSink::finding(
        "threading.Event.wait",
        RULE_BLOCKING_LOCK,
        "Prefer `asyncio.Event`, or wait inside a worker thread.",
    ),
    BuiltinSink::finding(
        "threading.Condition.wait",
        RULE_BLOCKING_LOCK,
        "Prefer asyncio primitives, or wait inside a worker thread.",
    ),
    BuiltinSink::finding(
        "threading.Semaphore.acquire",
        RULE_BLOCKING_LOCK,
        "Prefer `asyncio.Semaphore`, or acquire inside a worker thread.",
    ),
    BuiltinSink::finding(
        "threading.Barrier.wait",
        RULE_BLOCKING_LOCK,
        "Avoid barriers on the event loop; use asyncio coordination.",
    ),
    BuiltinSink::finding(
        "threading.Thread.join",
        RULE_BLOCKING_LOCK,
        "Avoid joining threads on the event loop; use executors / tasks.",
    ),
    BuiltinSink::finding(
        "queue.Queue.get",
        RULE_BLOCKING_LOCK,
        "Prefer `asyncio.Queue`, or call `get` inside a worker thread.",
    ),
    BuiltinSink::finding(
        "queue.Queue.put",
        RULE_BLOCKING_LOCK,
        "Prefer `asyncio.Queue`, or call `put` inside a worker thread.",
    ),
    BuiltinSink::finding(
        "queue.SimpleQueue.get",
        RULE_BLOCKING_LOCK,
        "Prefer `asyncio.Queue`, or call `get` inside a worker thread.",
    ),
    BuiltinSink::finding(
        "concurrent.futures.Future.result",
        RULE_BLOCKING_LOCK,
        "Await the future (`asyncio.wrap_future`) instead of blocking `.result()`.",
    ),
    BuiltinSink::candidate(
        "threading.Lock.__enter__",
        RULE_BLOCKING_LOCK,
        "Short uncontended locks are often fine; prefer asyncio.Lock if held across awaits.",
        "short-lock",
    ),
    BuiltinSink::finding_consume_only(
        "concurrent.futures.Executor.map",
        RULE_BLOCKING_LOCK,
        "Await `asyncio.gather` / wrap futures; avoid blocking `Executor.map` on the loop.",
    ),
    BuiltinSink::finding_consume_only(
        "concurrent.futures.ThreadPoolExecutor.map",
        RULE_BLOCKING_LOCK,
        "Await `asyncio.gather` / wrap futures; avoid blocking `Executor.map` on the loop.",
    ),
    BuiltinSink::finding_consume_only(
        "concurrent.futures.ProcessPoolExecutor.map",
        RULE_BLOCKING_LOCK,
        "Await `asyncio.gather` / wrap futures; avoid blocking `Executor.map` on the loop.",
    ),
    BuiltinSink::finding(
        "concurrent.futures.wait",
        RULE_BLOCKING_LOCK,
        "Await the futures (`asyncio.wrap_future` / `asyncio.wait`) instead of blocking `wait()`.",
    ),
    // --- sqlite3 ---
    BuiltinSink::finding("sqlite3.connect", RULE_BLOCKING_DB, SQLITE_FIX_HINT),
    BuiltinSink::finding(
        "sqlite3.Connection.execute",
        RULE_BLOCKING_DB,
        SQLITE_FIX_HINT,
    ),
    BuiltinSink::finding(
        "sqlite3.Connection.executemany",
        RULE_BLOCKING_DB,
        SQLITE_FIX_HINT,
    ),
    BuiltinSink::finding(
        "sqlite3.Connection.executescript",
        RULE_BLOCKING_DB,
        SQLITE_FIX_HINT,
    ),
    BuiltinSink::finding(
        "sqlite3.Connection.commit",
        RULE_BLOCKING_DB,
        SQLITE_FIX_HINT,
    ),
    BuiltinSink::finding(
        "sqlite3.Connection.rollback",
        RULE_BLOCKING_DB,
        SQLITE_FIX_HINT,
    ),
    BuiltinSink::finding(
        "sqlite3.Connection.backup",
        RULE_BLOCKING_DB,
        SQLITE_FIX_HINT,
    ),
    BuiltinSink::finding(
        "sqlite3.Connection.cursor",
        RULE_BLOCKING_DB,
        SQLITE_FIX_HINT,
    ),
    BuiltinSink::finding("sqlite3.Cursor.execute", RULE_BLOCKING_DB, SQLITE_FIX_HINT),
    BuiltinSink::finding(
        "sqlite3.Cursor.executemany",
        RULE_BLOCKING_DB,
        SQLITE_FIX_HINT,
    ),
    BuiltinSink::finding(
        "sqlite3.Cursor.executescript",
        RULE_BLOCKING_DB,
        SQLITE_FIX_HINT,
    ),
    BuiltinSink::finding("sqlite3.Cursor.fetchone", RULE_BLOCKING_DB, SQLITE_FIX_HINT),
    BuiltinSink::finding("sqlite3.Cursor.fetchall", RULE_BLOCKING_DB, SQLITE_FIX_HINT),
    BuiltinSink::finding(
        "sqlite3.Cursor.fetchmany",
        RULE_BLOCKING_DB,
        SQLITE_FIX_HINT,
    ),
    BuiltinSink::finding("sqlite3.Cursor.__iter__", RULE_BLOCKING_DB, SQLITE_FIX_HINT),
    // --- slow password KDFs → strictly candidates (never findings) ---
    BuiltinSink::candidate(
        "hashlib.pbkdf2_hmac",
        RULE_BLOCKING_CPU,
        "Slow password KDF; offload with `await asyncio.to_thread(hashlib.pbkdf2_hmac, ...)` when iteration count is high.",
        "slow-kdf",
    ),
    BuiltinSink::candidate(
        "hashlib.scrypt",
        RULE_BLOCKING_CPU,
        "Slow password KDF; offload with `await asyncio.to_thread(hashlib.scrypt, ...)` when cost parameters are non-trivial.",
        "slow-kdf",
    ),
];
