//! Stdlib sleep, subprocess, lock/future, sqlite3, and slow-KDF sinks.

use crate::{
    BuiltinSink, RULE_BLOCKING_CPU, RULE_BLOCKING_DB, RULE_BLOCKING_LOCK, RULE_BLOCKING_SLEEP,
    RULE_BLOCKING_SUBPROCESS,
};

pub(super) const RUNTIME_SINKS: &[BuiltinSink] = &[
    // --- sleep ---
    BuiltinSink::finding("time.sleep", RULE_BLOCKING_SLEEP),
    // --- subprocess / os.system / os.waitpid (never subprocess.Popen constructor) ---
    BuiltinSink::finding("subprocess.run", RULE_BLOCKING_SUBPROCESS),
    BuiltinSink::finding("subprocess.call", RULE_BLOCKING_SUBPROCESS),
    BuiltinSink::finding("subprocess.check_call", RULE_BLOCKING_SUBPROCESS),
    BuiltinSink::finding("subprocess.check_output", RULE_BLOCKING_SUBPROCESS),
    BuiltinSink::finding("subprocess.getoutput", RULE_BLOCKING_SUBPROCESS),
    BuiltinSink::finding("subprocess.getstatusoutput", RULE_BLOCKING_SUBPROCESS),
    BuiltinSink::finding("subprocess.Popen.wait", RULE_BLOCKING_SUBPROCESS),
    BuiltinSink::finding("subprocess.Popen.communicate", RULE_BLOCKING_SUBPROCESS),
    BuiltinSink::finding("os.system", RULE_BLOCKING_SUBPROCESS),
    BuiltinSink::finding("os.popen", RULE_BLOCKING_SUBPROCESS),
    BuiltinSink::finding("os.waitpid", RULE_BLOCKING_SUBPROCESS),
    // --- threading / queue / concurrent.futures ---
    BuiltinSink::finding("threading.Lock.acquire", RULE_BLOCKING_LOCK),
    BuiltinSink::finding("threading.RLock.acquire", RULE_BLOCKING_LOCK),
    BuiltinSink::finding("threading.Event.wait", RULE_BLOCKING_LOCK),
    BuiltinSink::finding("threading.Condition.wait", RULE_BLOCKING_LOCK),
    BuiltinSink::finding("threading.Semaphore.acquire", RULE_BLOCKING_LOCK),
    BuiltinSink::finding("threading.Barrier.wait", RULE_BLOCKING_LOCK),
    BuiltinSink::finding("threading.Thread.join", RULE_BLOCKING_LOCK),
    BuiltinSink::finding("queue.Queue.get", RULE_BLOCKING_LOCK),
    BuiltinSink::finding("queue.Queue.put", RULE_BLOCKING_LOCK),
    BuiltinSink::finding("queue.SimpleQueue.get", RULE_BLOCKING_LOCK),
    BuiltinSink::finding("concurrent.futures.Future.result", RULE_BLOCKING_LOCK),
    BuiltinSink::finding("concurrent.futures.Future.exception", RULE_BLOCKING_LOCK),
    BuiltinSink::candidate("threading.Lock.__enter__", RULE_BLOCKING_LOCK, "short-lock"),
    BuiltinSink::finding_consume_only("concurrent.futures.Executor.map", RULE_BLOCKING_LOCK),
    BuiltinSink::finding_consume_only(
        "concurrent.futures.ThreadPoolExecutor.map",
        RULE_BLOCKING_LOCK,
    ),
    BuiltinSink::finding_consume_only(
        "concurrent.futures.ProcessPoolExecutor.map",
        RULE_BLOCKING_LOCK,
    ),
    BuiltinSink::finding("concurrent.futures.wait", RULE_BLOCKING_LOCK),
    // --- sqlite3 ---
    BuiltinSink::finding("sqlite3.connect", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlite3.Connection.execute", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlite3.Connection.executemany", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlite3.Connection.executescript", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlite3.Connection.commit", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlite3.Connection.rollback", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlite3.Connection.backup", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlite3.Connection.cursor", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlite3.Cursor.execute", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlite3.Cursor.executemany", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlite3.Cursor.executescript", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlite3.Cursor.fetchone", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlite3.Cursor.fetchall", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlite3.Cursor.fetchmany", RULE_BLOCKING_DB),
    BuiltinSink::finding("sqlite3.Cursor.__iter__", RULE_BLOCKING_DB),
    // --- slow password KDFs → strictly candidates (never findings) ---
    BuiltinSink::candidate("hashlib.pbkdf2_hmac", RULE_BLOCKING_CPU, "slow-kdf"),
    BuiltinSink::candidate("hashlib.scrypt", RULE_BLOCKING_CPU, "slow-kdf"),
];
