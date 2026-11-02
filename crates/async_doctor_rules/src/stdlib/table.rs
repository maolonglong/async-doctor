//! Stdlib sink table with qualname normalization.

use std::borrow::Cow;

use async_doctor_core::{SinkHit, SinkTable};

use super::sinks::STDLIB_SINKS;

/// Sink table backed by [`STDLIB_SINKS`], with alias normalization.
#[derive(Debug, Default, Clone)]
pub struct StdlibSinkTable;

impl StdlibSinkTable {
    pub fn new() -> Self {
        Self
    }

    fn normalize(qualname: &str) -> Cow<'_, str> {
        let mut q = Cow::Borrowed(qualname);
        // Apply in order; some need multiple passes (e.g. `_thread.lock.acquire`).
        let replacements = [
            ("_io.", "io."),
            ("_sqlite3.", "sqlite3."),
            // CPython runtime types behind `threading.Lock()` / `RLock()`.
            ("_thread.lock.", "threading.Lock."),
            ("_thread.RLock.", "threading.RLock."),
            ("_thread.", "threading."),
            ("pathlib._local.Path.", "pathlib.Path."),
            ("pathlib._local.", "pathlib."),
            ("pathlib.PosixPath.", "pathlib.Path."),
            ("pathlib.WindowsPath.", "pathlib.Path."),
            (
                "importlib_resources.abc.Traversable.",
                "importlib.resources.abc.Traversable.",
            ),
            ("importlib_resources._common.", "importlib.resources."),
            ("importlib_resources._functional.", "importlib.resources."),
            ("importlib_resources.", "importlib.resources."),
            (
                "importlib.resources.simple.SimpleReader.",
                "importlib.resources.abc.Traversable.",
            ),
            (
                "importlib.resources.readers.",
                "importlib.resources.abc.Traversable.",
            ),
            (
                "importlib.abc.Traversable.",
                "importlib.resources.abc.Traversable.",
            ),
            (
                "importlib._abc.Traversable.",
                "importlib.resources.abc.Traversable.",
            ),
            ("importlib.resources._common.", "importlib.resources."),
            ("importlib.resources._functional.", "importlib.resources."),
            ("sqlite3.dbapi2.", "sqlite3."),
            // CPython hides Future behind concurrent.futures._base.
            (
                "concurrent.futures._base.Future.",
                "concurrent.futures.Future.",
            ),
            ("concurrent.futures._base.", "concurrent.futures."),
            (
                "concurrent.futures.thread.ThreadPoolExecutor.",
                "concurrent.futures.ThreadPoolExecutor.",
            ),
            (
                "concurrent.futures.process.ProcessPoolExecutor.",
                "concurrent.futures.ProcessPoolExecutor.",
            ),
        ];
        for (from, to) in replacements {
            if let Some(rest) = q.strip_prefix(from) {
                q = Cow::Owned(format!("{to}{rest}"));
            }
        }
        if q == "open" {
            return Cow::Borrowed("builtins.open");
        }
        // `_thread.lock` is the runtime type behind `threading.Lock()`.
        if q.contains("_thread.lock.") {
            q = Cow::Owned(q.replace("_thread.lock.", "threading.Lock."));
        }
        // ty may surface `LockType.acquire` / `LockType.release` for
        // `threading.Lock()` instances — map only those methods (not
        // `__enter__`, which would flood short-lock candidates).
        for (from, to) in [
            ("_thread.LockType.acquire", "threading.Lock.acquire"),
            ("_thread.LockType.release", "threading.Lock.release"),
            ("threading.LockType.acquire", "threading.Lock.acquire"),
            ("threading.LockType.release", "threading.Lock.release"),
        ] {
            if q == from {
                return Cow::Borrowed(to);
            }
        }
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
            (
                "concurrent.futures._base.Future.result",
                "concurrent.futures.Future.result",
            ),
            ("Connection.execute", "sqlite3.Connection.execute"),
            ("Cursor.execute", "sqlite3.Cursor.execute"),
            ("Cursor.fetchall", "sqlite3.Cursor.fetchall"),
            ("Cursor.fetchone", "sqlite3.Cursor.fetchone"),
            ("Path.read_text", "pathlib.Path.read_text"),
            ("Path.write_text", "pathlib.Path.write_text"),
            ("Path.read_bytes", "pathlib.Path.read_bytes"),
            ("Path.write_bytes", "pathlib.Path.write_bytes"),
            ("Path.open", "pathlib.Path.open"),
            ("Path.mkdir", "pathlib.Path.mkdir"),
            ("Path.rmdir", "pathlib.Path.rmdir"),
            ("Path.unlink", "pathlib.Path.unlink"),
            ("Path.rename", "pathlib.Path.rename"),
            ("Path.replace", "pathlib.Path.replace"),
            ("Path.touch", "pathlib.Path.touch"),
            ("Path.exists", "pathlib.Path.exists"),
            ("Path.stat", "pathlib.Path.stat"),
            ("Path.is_file", "pathlib.Path.is_file"),
            ("Path.is_dir", "pathlib.Path.is_dir"),
            ("Path.iterdir", "pathlib.Path.iterdir"),
            ("Path.glob", "pathlib.Path.glob"),
            ("Path.rglob", "pathlib.Path.rglob"),
            (
                "Traversable.read_text",
                "importlib.resources.abc.Traversable.read_text",
            ),
            (
                "Traversable.read_bytes",
                "importlib.resources.abc.Traversable.read_bytes",
            ),
            (
                "Traversable.open",
                "importlib.resources.abc.Traversable.open",
            ),
            (
                "Traversable.iterdir",
                "importlib.resources.abc.Traversable.iterdir",
            ),
            (
                "Traversable.is_file",
                "importlib.resources.abc.Traversable.is_file",
            ),
            (
                "Traversable.is_dir",
                "importlib.resources.abc.Traversable.is_dir",
            ),
            ("ZipFile.extractall", "zipfile.ZipFile.extractall"),
            ("ZipFile.extract", "zipfile.ZipFile.extract"),
            ("TarFile.extractall", "tarfile.TarFile.extractall"),
            ("SMTP.sendmail", "smtplib.SMTP.sendmail"),
            ("FTP.login", "ftplib.FTP.login"),
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
