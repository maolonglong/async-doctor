//! Stdlib socket, TLS, HTTP, email, and FTP blocking sinks.

use crate::{BuiltinSink, RULE_BLOCKING_HTTP, RULE_BLOCKING_SOCKET};

pub(super) const NETWORK_SINKS: &[BuiltinSink] = &[
    // --- socket / ssl / select ---
    BuiltinSink::finding("socket.socket.connect", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("socket.socket.accept", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("socket.socket.recv", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("socket.socket.recv_into", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("socket.socket.recvfrom", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("socket.socket.recvfrom_into", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("socket.socket.send", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("socket.socket.sendall", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("socket.socket.sendto", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("socket.socket.sendfile", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("socket.create_connection", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("socket.getaddrinfo", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("socket.gethostbyname", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("ssl.SSLContext.wrap_socket", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("ssl.SSLSocket.read", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("ssl.SSLSocket.write", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("ssl.SSLSocket.recv", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("ssl.SSLSocket.send", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("ssl.SSLSocket.do_handshake", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("ssl.SSLSocket.unwrap", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("select.select", RULE_BLOCKING_SOCKET),
    // --- urllib / http.client ---
    BuiltinSink::finding("urllib.request.urlopen", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("urllib.request.urlretrieve", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("http.client.HTTPConnection.connect", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("http.client.HTTPConnection.request", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("http.client.HTTPConnection.getresponse", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("http.client.HTTPSConnection.connect", RULE_BLOCKING_HTTP),
    BuiltinSink::finding("http.client.HTTPSConnection.request", RULE_BLOCKING_HTTP),
    BuiltinSink::finding(
        "http.client.HTTPSConnection.getresponse",
        RULE_BLOCKING_HTTP,
    ),
    // --- email / ftp ---
    BuiltinSink::finding("smtplib.SMTP", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("smtplib.SMTP.connect", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("smtplib.SMTP.login", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("smtplib.SMTP.sendmail", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("smtplib.SMTP.send_message", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("smtplib.SMTP_SSL", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("ftplib.FTP", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("ftplib.FTP.connect", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("ftplib.FTP.login", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("ftplib.FTP.retrbinary", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("ftplib.FTP.storbinary", RULE_BLOCKING_SOCKET),
    BuiltinSink::finding("ftplib.FTP_TLS", RULE_BLOCKING_SOCKET),
];
