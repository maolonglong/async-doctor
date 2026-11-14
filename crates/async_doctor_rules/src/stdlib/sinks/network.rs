//! Stdlib socket, TLS, HTTP, email, and FTP blocking sinks.

use crate::{BuiltinSink, RULE_BLOCKING_HTTP, RULE_BLOCKING_SOCKET};

pub(super) const NETWORK_SINKS: &[BuiltinSink] = &[
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
        "socket.socket.recvfrom",
        RULE_BLOCKING_SOCKET,
        "Use `loop.sock_recvfrom` / asyncio datagram transports, or offload.",
    ),
    BuiltinSink::finding(
        "socket.socket.recvfrom_into",
        RULE_BLOCKING_SOCKET,
        "Use `loop.sock_recvfrom_into` / asyncio datagram transports, or offload.",
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
        "socket.socket.sendto",
        RULE_BLOCKING_SOCKET,
        "Use `loop.sock_sendto` / asyncio datagram transports, or offload.",
    ),
    BuiltinSink::finding(
        "socket.socket.sendfile",
        RULE_BLOCKING_SOCKET,
        "Use `loop.sendfile` / `StreamWriter`, or offload with `asyncio.to_thread`.",
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
        "ssl.SSLContext.wrap_socket",
        RULE_BLOCKING_SOCKET,
        "Pass `ssl=` to `asyncio.open_connection` / `start_tls`, or offload.",
    ),
    BuiltinSink::finding(
        "ssl.SSLSocket.read",
        RULE_BLOCKING_SOCKET,
        "Use asyncio TLS streams (`open_connection(..., ssl=...)`), or offload.",
    ),
    BuiltinSink::finding(
        "ssl.SSLSocket.write",
        RULE_BLOCKING_SOCKET,
        "Use asyncio TLS streams (`open_connection(..., ssl=...)`), or offload.",
    ),
    BuiltinSink::finding(
        "ssl.SSLSocket.recv",
        RULE_BLOCKING_SOCKET,
        "Use asyncio TLS streams (`open_connection(..., ssl=...)`), or offload.",
    ),
    BuiltinSink::finding(
        "ssl.SSLSocket.send",
        RULE_BLOCKING_SOCKET,
        "Use asyncio TLS streams (`open_connection(..., ssl=...)`), or offload.",
    ),
    BuiltinSink::finding(
        "ssl.SSLSocket.do_handshake",
        RULE_BLOCKING_SOCKET,
        "Use `asyncio` TLS APIs (`open_connection` with ssl), or offload.",
    ),
    BuiltinSink::finding(
        "ssl.SSLSocket.unwrap",
        RULE_BLOCKING_SOCKET,
        "Use asyncio TLS transport shutdown, or offload.",
    ),
    BuiltinSink::finding(
        "select.select",
        RULE_BLOCKING_SOCKET,
        "Use asyncio wait/selectors integration, or offload.",
    ),
    // --- urllib / http.client ---
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
    BuiltinSink::finding(
        "http.client.HTTPConnection.connect",
        RULE_BLOCKING_HTTP,
        "Use an async HTTP client (`httpx.AsyncClient`, `aiohttp`), or offload.",
    ),
    BuiltinSink::finding(
        "http.client.HTTPConnection.request",
        RULE_BLOCKING_HTTP,
        "Use an async HTTP client (`httpx.AsyncClient`, `aiohttp`), or offload.",
    ),
    BuiltinSink::finding(
        "http.client.HTTPConnection.getresponse",
        RULE_BLOCKING_HTTP,
        "Use an async HTTP client (`httpx.AsyncClient`, `aiohttp`), or offload.",
    ),
    BuiltinSink::finding(
        "http.client.HTTPSConnection.connect",
        RULE_BLOCKING_HTTP,
        "Use an async HTTP client (`httpx.AsyncClient`, `aiohttp`), or offload.",
    ),
    BuiltinSink::finding(
        "http.client.HTTPSConnection.request",
        RULE_BLOCKING_HTTP,
        "Use an async HTTP client (`httpx.AsyncClient`, `aiohttp`), or offload.",
    ),
    BuiltinSink::finding(
        "http.client.HTTPSConnection.getresponse",
        RULE_BLOCKING_HTTP,
        "Use an async HTTP client (`httpx.AsyncClient`, `aiohttp`), or offload.",
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
];
