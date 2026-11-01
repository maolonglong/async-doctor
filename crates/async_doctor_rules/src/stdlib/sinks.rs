//! Stdlib sink rows.

use crate::{
    BuiltinSink, RULE_BLOCKING_DB, RULE_BLOCKING_FILE, RULE_BLOCKING_HTTP, RULE_BLOCKING_LOCK,
    RULE_BLOCKING_SLEEP, RULE_BLOCKING_SOCKET, RULE_BLOCKING_SUBPROCESS,
};

/// Stdlib sink catalog.
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
    // --- email / ftp ---
    BuiltinSink::finding(
        "smtplib.SMTP",
        RULE_BLOCKING_SOCKET,
        "Use `aiosmtplib`, or offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "smtplib.SMTP.connect",
        RULE_BLOCKING_SOCKET,
        "Use `aiosmtplib`, or offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "smtplib.SMTP.login",
        RULE_BLOCKING_SOCKET,
        "Use `aiosmtplib`, or offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "smtplib.SMTP.sendmail",
        RULE_BLOCKING_SOCKET,
        "Use `aiosmtplib`, or offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "smtplib.SMTP.send_message",
        RULE_BLOCKING_SOCKET,
        "Use `aiosmtplib`, or offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "smtplib.SMTP_SSL",
        RULE_BLOCKING_SOCKET,
        "Use `aiosmtplib`, or offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "ftplib.FTP",
        RULE_BLOCKING_SOCKET,
        "Use an async FTP client, or offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "ftplib.FTP.connect",
        RULE_BLOCKING_SOCKET,
        "Use an async FTP client, or offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "ftplib.FTP.login",
        RULE_BLOCKING_SOCKET,
        "Use an async FTP client, or offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "ftplib.FTP.retrbinary",
        RULE_BLOCKING_SOCKET,
        "Use an async FTP client, or offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "ftplib.FTP.storbinary",
        RULE_BLOCKING_SOCKET,
        "Use an async FTP client, or offload with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "ftplib.FTP_TLS",
        RULE_BLOCKING_SOCKET,
        "Use an async FTP client, or offload with `asyncio.to_thread`.",
    ),
    // --- archives ---
    BuiltinSink::finding(
        "zipfile.ZipFile",
        RULE_BLOCKING_FILE,
        "Offload zip open/extract/read/write with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "zipfile.ZipFile.open",
        RULE_BLOCKING_FILE,
        "Offload zip IO with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "zipfile.ZipFile.read",
        RULE_BLOCKING_FILE,
        "Offload zip IO with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "zipfile.ZipFile.write",
        RULE_BLOCKING_FILE,
        "Offload zip IO with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "zipfile.ZipFile.writestr",
        RULE_BLOCKING_FILE,
        "Offload zip IO with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "zipfile.ZipFile.extract",
        RULE_BLOCKING_FILE,
        "Offload zip extract with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "zipfile.ZipFile.extractall",
        RULE_BLOCKING_FILE,
        "Offload zip extract with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "tarfile.open",
        RULE_BLOCKING_FILE,
        "Offload tar open/extract/read/write with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "tarfile.TarFile.open",
        RULE_BLOCKING_FILE,
        "Offload tar open/extract/read/write with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "tarfile.TarFile.extract",
        RULE_BLOCKING_FILE,
        "Offload tar extract with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "tarfile.TarFile.extractall",
        RULE_BLOCKING_FILE,
        "Offload tar extract with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "tarfile.TarFile.add",
        RULE_BLOCKING_FILE,
        "Offload tar write with `asyncio.to_thread`.",
    ),
    BuiltinSink::finding(
        "tarfile.TarFile.addfile",
        RULE_BLOCKING_FILE,
        "Offload tar write with `asyncio.to_thread`.",
    ),
    // --- glob / pathlib directory walks (tier with os.listdir) ---
    BuiltinSink::finding(
        "glob.glob",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread(glob.glob, ...)`.",
    ),
    BuiltinSink::finding(
        "glob.iglob",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`, or use async directory helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.iterdir",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`, or use async directory helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.glob",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`, or use async directory helpers.",
    ),
    BuiltinSink::finding(
        "pathlib.Path.rglob",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread`, or use async directory helpers.",
    ),
    // --- stdin ---
    BuiltinSink::finding(
        "builtins.input",
        RULE_BLOCKING_FILE,
        "Offload with `asyncio.to_thread(input, ...)`, or use async stdin APIs.",
    ),
    // --- concurrent.futures blocking waits ---
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
];
