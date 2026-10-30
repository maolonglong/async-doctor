//! Built-in rule / sink tables for async-doctor.
//!
//! Stdlib sinks (M1) plus third-party type-based rules (M2).

mod docs;
mod third_party;

pub use docs::{RULE_DOCS, RuleDoc, docs_anchor_for, rule_doc};
pub use third_party::{BuiltinSinkTable, THIRD_PARTY_SINKS, is_async_counterpart};

use async_doctor_core::{SinkHit, SinkTable};

pub const RULE_BLOCKING_SLEEP: &str = "blocking-sleep";
pub const RULE_BLOCKING_SOCKET: &str = "blocking-socket";
pub const RULE_BLOCKING_HTTP: &str = "blocking-http";
pub const RULE_BLOCKING_SUBPROCESS: &str = "blocking-subprocess";
pub const RULE_BLOCKING_FILE: &str = "blocking-file";
pub const RULE_BLOCKING_LOCK: &str = "blocking-lock";
pub const RULE_BLOCKING_DB: &str = "blocking-db";

pub fn blocking_sleep_fix_hint() -> &'static str {
    "Replace `time.sleep(...)` with `await asyncio.sleep(...)` \
     (or offload with `await asyncio.to_thread(...)`)."
}

/// One stdlib (or builtins) sink primitive.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinSink {
    pub qualname: &'static str,
    pub rule: &'static str,
    pub fix_hint: &'static str,
    pub candidate_only: bool,
    pub candidate_reason: Option<&'static str>,
    /// Only fire for `with` enter edges (e.g. `with Engine.begin()`).
    pub with_only: bool,
}

impl BuiltinSink {
    pub const fn finding(
        qualname: &'static str,
        rule: &'static str,
        fix_hint: &'static str,
    ) -> Self {
        Self {
            qualname,
            rule,
            fix_hint,
            candidate_only: false,
            candidate_reason: None,
            with_only: false,
        }
    }

    pub const fn finding_with_only(
        qualname: &'static str,
        rule: &'static str,
        fix_hint: &'static str,
    ) -> Self {
        Self {
            qualname,
            rule,
            fix_hint,
            candidate_only: false,
            candidate_reason: None,
            with_only: true,
        }
    }

    pub const fn candidate(
        qualname: &'static str,
        rule: &'static str,
        fix_hint: &'static str,
        reason: &'static str,
    ) -> Self {
        Self {
            qualname,
            rule,
            fix_hint,
            candidate_only: true,
            candidate_reason: Some(reason),
            with_only: false,
        }
    }

    pub fn hit(self) -> SinkHit {
        SinkHit {
            rule: self.rule,
            sink_qualname: self.qualname,
            fix_hint: self.fix_hint,
            candidate_only: self.candidate_only,
            candidate_reason: self.candidate_reason,
            with_only: self.with_only,
        }
    }
}

/// Stdlib sink catalog for M1.
pub static STDLIB_SINKS: &[BuiltinSink] = &[
    // --- sleep ---
    BuiltinSink::finding(
        "time.sleep",
        RULE_BLOCKING_SLEEP,
        "Replace with `await asyncio.sleep(...)` or offload via `asyncio.to_thread`.",
    ),
    // --- socket / ssl / select ---
    BuiltinSink::finding(
        "socket.socket.connect",
        RULE_BLOCKING_SOCKET,
        "Use `asyncio.open_connection` / async sockets, or offload.",
    ),
    BuiltinSink::finding(
        "socket.socket.accept",
        RULE_BLOCKING_SOCKET,
        "Use asyncio server APIs, or offload.",
    ),
    BuiltinSink::finding(
        "socket.socket.recv",
        RULE_BLOCKING_SOCKET,
        "Use asyncio streams, or offload.",
    ),
    BuiltinSink::finding(
        "socket.socket.recv_into",
        RULE_BLOCKING_SOCKET,
        "Use asyncio streams, or offload.",
    ),
    BuiltinSink::finding(
        "socket.socket.send",
        RULE_BLOCKING_SOCKET,
        "Use asyncio streams, or offload.",
    ),
    BuiltinSink::finding(
        "socket.socket.sendall",
        RULE_BLOCKING_SOCKET,
        "Use asyncio streams, or offload.",
    ),
    BuiltinSink::finding(
        "socket.create_connection",
        RULE_BLOCKING_SOCKET,
        "Use `asyncio.open_connection`, or offload.",
    ),
    BuiltinSink::finding(
        "socket.getaddrinfo",
        RULE_BLOCKING_SOCKET,
        "Use `asyncio.getaddrinfo` / loop.getaddrinfo, or offload.",
    ),
    BuiltinSink::finding(
        "socket.gethostbyname",
        RULE_BLOCKING_SOCKET,
        "Use asyncio DNS helpers, or offload.",
    ),
    BuiltinSink::finding(
        "ssl.SSLSocket.do_handshake",
        RULE_BLOCKING_SOCKET,
        "Use `asyncio` TLS APIs (`open_connection` with ssl), or offload.",
    ),
    BuiltinSink::finding(
        "select.select",
        RULE_BLOCKING_SOCKET,
        "Use asyncio wait/selectors integration, or offload.",
    ),
    // --- urllib ---
    BuiltinSink::finding(
        "urllib.request.urlopen",
        RULE_BLOCKING_HTTP,
        "Use an async HTTP client (`httpx.AsyncClient`, `aiohttp`), or offload.",
    ),
    BuiltinSink::finding(
        "urllib.request.urlretrieve",
        RULE_BLOCKING_HTTP,
        "Use an async HTTP client, or offload.",
    ),
    // --- subprocess / os.system ---
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
    // --- file IO (blocking findings) ---
    BuiltinSink::finding(
        "builtins.open",
        RULE_BLOCKING_FILE,
        "Use `aiofiles` / `anyio` file APIs, or offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "io.open",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload.",
    ),
    BuiltinSink::finding(
        "io.TextIOWrapper.read",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload.",
    ),
    BuiltinSink::finding(
        "io.TextIOWrapper.write",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload.",
    ),
    BuiltinSink::finding(
        "io.TextIOWrapper.readline",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload.",
    ),
    BuiltinSink::finding(
        "io.TextIOWrapper.readlines",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload.",
    ),
    BuiltinSink::finding(
        "io.BufferedReader.read",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload.",
    ),
    BuiltinSink::finding(
        "io.BufferedWriter.write",
        RULE_BLOCKING_FILE,
        "Use async file APIs, or offload.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.read_text",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread(path.read_text)` or use async file helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.write_text",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread(path.write_text)` or use async file helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.read_bytes",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`, or use async file helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.write_bytes",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`, or use async file helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.open",
        RULE_BLOCKING_FILE,
        "Offload file IO, or use async file helpers.",
    ),
    BuiltinSink::finding(
        "os.listdir",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread(os.listdir, ...)`.",
    ),
    BuiltinSink::finding(
        "os.walk",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`, or use async directory helpers.",
    ),
    BuiltinSink::finding(
        "os.scandir",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.remove",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.unlink",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.rename",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.mkdir",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.makedirs",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "os.rmdir",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "shutil.copy",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "shutil.copy2",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "shutil.copytree",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "shutil.move",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "shutil.rmtree",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`.",
    ),
    // --- small metadata → candidates ---
    BuiltinSink::candidate(
        "os.stat",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if contended or on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "os.lstat",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if contended or on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "os.path.exists",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "os.path.isfile",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "os.path.isdir",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "os.path.getsize",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "pathlib.Path.exists",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "pathlib.Path.stat",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "pathlib.Path.is_file",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS.",
        "metadata",
    ),
    BuiltinSink::candidate(
        "pathlib.Path.is_dir",
        RULE_BLOCKING_FILE,
        "Usually cheap; offload if on a slow FS.",
        "metadata",
    ),
    // --- threading / queue locks ---
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
    // uncontended short locks → candidates (same sinks, softer grade when flagged)
    BuiltinSink::candidate(
        "threading.Lock.__enter__",
        RULE_BLOCKING_LOCK,
        "Short uncontended locks are often fine; prefer asyncio.Lock if held across awaits.",
        "short-lock",
    ),
    // --- sqlite3 ---
    BuiltinSink::finding(
        "sqlite3.connect",
        RULE_BLOCKING_DB,
        "Use `aiosqlite` / run DB work in `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "sqlite3.Connection.execute",
        RULE_BLOCKING_DB,
        "Use `aiosqlite` / run DB work in `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "sqlite3.Connection.executemany",
        RULE_BLOCKING_DB,
        "Use `aiosqlite` / run DB work in `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "sqlite3.Connection.cursor",
        RULE_BLOCKING_DB,
        "Use `aiosqlite` / run DB work in `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "sqlite3.Cursor.execute",
        RULE_BLOCKING_DB,
        "Use `aiosqlite` / run DB work in `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "sqlite3.Cursor.fetchone",
        RULE_BLOCKING_DB,
        "Use `aiosqlite` / run DB work in `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "sqlite3.Cursor.fetchall",
        RULE_BLOCKING_DB,
        "Use `aiosqlite` / run DB work in `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "sqlite3.Cursor.fetchmany",
        RULE_BLOCKING_DB,
        "Use `aiosqlite` / run DB work in `asyncio.to_thread`.",
    ),
];

/// Sink table backed by [`STDLIB_SINKS`], with alias normalization.
#[derive(Debug, Default, Clone)]
pub struct StdlibSinkTable;

impl StdlibSinkTable {
    pub fn new() -> Self {
        Self
    }

    fn normalize(qualname: &str) -> String {
        let mut q = qualname.to_string();
        // Apply in order; some need multiple passes (e.g. `_thread.lock.acquire`).
        let replacements = [
            ("_io.", "io."),
            ("_sqlite3.", "sqlite3."),
            ("_thread.lock.", "threading.Lock."),
            ("_thread.RLock.", "threading.RLock."),
            ("_thread.", "threading."),
            ("pathlib._local.Path.", "pathlib.Path."),
            ("pathlib._local.", "pathlib."),
            ("sqlite3.dbapi2.", "sqlite3."),
        ];
        for (from, to) in replacements {
            if let Some(rest) = q.strip_prefix(from) {
                q = format!("{to}{rest}");
            }
        }
        if q == "open" {
            return "builtins.open".to_string();
        }
        // `_thread.lock` is the runtime type behind `threading.Lock()`.
        q = q.replace("_thread.lock.", "threading.Lock.");
        q
    }
}

impl SinkTable for StdlibSinkTable {
    fn lookup(&self, qualname: &str) -> Option<SinkHit> {
        let normalized = Self::normalize(qualname);
        for sink in STDLIB_SINKS {
            if sink.qualname == normalized || sink.qualname == qualname {
                return Some(sink.hit());
            }
        }
        // Exact aliases for common inference shapes (Class.method without module).
        let aliases: &[(&str, &str)] = &[
            ("socket.connect", "socket.socket.connect"),
            ("socket.accept", "socket.socket.accept"),
            ("socket.recv", "socket.socket.recv"),
            ("socket.send", "socket.socket.send"),
            ("socket.sendall", "socket.socket.sendall"),
            ("Popen.wait", "subprocess.Popen.wait"),
            ("Popen.communicate", "subprocess.Popen.communicate"),
            ("Lock.acquire", "threading.Lock.acquire"),
            ("RLock.acquire", "threading.RLock.acquire"),
            ("Event.wait", "threading.Event.wait"),
            ("Thread.join", "threading.Thread.join"),
            ("Queue.get", "queue.Queue.get"),
            ("Queue.put", "queue.Queue.put"),
            ("Future.result", "concurrent.futures.Future.result"),
            ("Connection.execute", "sqlite3.Connection.execute"),
            ("Cursor.execute", "sqlite3.Cursor.execute"),
            ("Cursor.fetchall", "sqlite3.Cursor.fetchall"),
            ("Cursor.fetchone", "sqlite3.Cursor.fetchone"),
            ("Path.read_text", "pathlib.Path.read_text"),
            ("Path.write_text", "pathlib.Path.write_text"),
            ("Path.read_bytes", "pathlib.Path.read_bytes"),
            ("Path.write_bytes", "pathlib.Path.write_bytes"),
            ("Path.open", "pathlib.Path.open"),
            ("Path.exists", "pathlib.Path.exists"),
            ("Path.stat", "pathlib.Path.stat"),
            ("TextIOWrapper.read", "io.TextIOWrapper.read"),
            ("TextIOWrapper.write", "io.TextIOWrapper.write"),
        ];
        for (alias, canonical) in aliases {
            if (normalized == *alias || qualname == *alias)
                && let Some(sink) = STDLIB_SINKS.iter().find(|s| s.qualname == *canonical)
            {
                return Some(sink.hit());
            }
        }
        None
    }
}
