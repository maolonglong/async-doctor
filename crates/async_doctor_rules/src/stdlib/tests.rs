//! Unit tests for stdlib sinks, slow-KDF candidates, and zero-FP guardrails.

use async_doctor_core::SinkTable;

use crate::{
    BuiltinSinkTable, RULE_BLOCKING_DB, RULE_BLOCKING_FILE, RULE_BLOCKING_HTTP,
    RULE_BLOCKING_SOCKET, RULE_BLOCKING_SUBPROCESS, rule_doc,
};

#[test]
fn sqlite3_extended_sinks_and_aliases_resolve() {
    let t = BuiltinSinkTable::new();
    for q in [
        "sqlite3.Connection.executescript",
        "sqlite3.Connection.commit",
        "sqlite3.Connection.rollback",
        "sqlite3.Connection.backup",
        "sqlite3.Cursor.executescript",
        "sqlite3.Cursor.executemany",
        "sqlite3.Cursor.__iter__",
        "_sqlite3.Connection.commit",
        "_sqlite3.Cursor.__iter__",
        "sqlite3.dbapi2.Connection.backup",
    ] {
        let hit = t
            .lookup(q)
            .unwrap_or_else(|| panic!("expected sink for {q}"));
        assert_eq!(hit.rule, RULE_BLOCKING_DB, "wrong rule for {q}");
        assert!(!hit.candidate_only, "{q} must be a finding");
        assert!(
            !hit.fix_hint.contains("to_thread(conn.execute")
                && !hit.fix_hint.contains("to_thread(session.execute"),
            "sqlite3 fix_hint must not advise sharing Connection across threads: {}",
            hit.fix_hint
        );
        assert!(
            hit.fix_hint.contains("aiosqlite") && hit.fix_hint.contains("sqlite3.Connection"),
            "sqlite3 fix_hint should mention aiosqlite and worker-owned Connection: {}",
            hit.fix_hint
        );
    }
}

#[test]
fn socket_and_ssl_extended_sinks_resolve_without_wrap_bio_fp() {
    let t = BuiltinSinkTable::new();
    for q in [
        "socket.socket.recvfrom",
        "socket.socket.recvfrom_into",
        "socket.socket.sendto",
        "socket.socket.sendfile",
        "_socket.socket.recvfrom",
        "socket.recvfrom",
        "socket.sendto",
        "socket.sendfile",
        "ssl.SSLContext.wrap_socket",
        "ssl.SSLSocket.read",
        "ssl.SSLSocket.write",
        "ssl.SSLSocket.recv",
        "ssl.SSLSocket.send",
        "ssl.SSLSocket.do_handshake",
        "ssl.SSLSocket.unwrap",
        "_ssl.SSLSocket.read",
    ] {
        let hit = t
            .lookup(q)
            .unwrap_or_else(|| panic!("expected sink for {q}"));
        assert_eq!(hit.rule, RULE_BLOCKING_SOCKET, "wrong rule for {q}");
        assert!(!hit.candidate_only, "{q} must be a finding");
    }
    // Non-blocking memory BIO TLS used by async HTTP stacks must never fire.
    assert!(t.lookup("ssl.SSLContext.wrap_bio").is_none());
    assert!(t.lookup("ssl.MemoryBIO.read").is_none());
    assert!(t.lookup("ssl.MemoryBIO.write").is_none());
}

#[test]
fn io_os_shutil_pathlib_and_http_client_sinks_resolve() {
    let t = BuiltinSinkTable::new();
    for q in [
        "io.BufferedReader.readline",
        "io.BufferedReader.readlines",
        "io.BufferedReader.readinto",
        "_io.BufferedReader.readline",
        "io.FileIO.read",
        "io.FileIO.readall",
        "io.FileIO.readinto",
        "io.FileIO.write",
        "_io.FileIO.readall",
        "os.read",
        "os.write",
        "os.pread",
        "os.pwrite",
        "os.fsync",
        "os.fdatasync",
        "pathlib.Path.walk",
        "pathlib.PosixPath.walk",
        "Path.walk",
        "shutil.copyfile",
        "shutil.copyfileobj",
        "shutil.make_archive",
        "shutil.unpack_archive",
    ] {
        let hit = t
            .lookup(q)
            .unwrap_or_else(|| panic!("expected sink for {q}"));
        assert_eq!(hit.rule, RULE_BLOCKING_FILE, "wrong rule for {q}");
        assert!(!hit.candidate_only, "{q} must be a finding");
    }

    let waitpid = t.lookup("os.waitpid").expect("expected os.waitpid sink");
    assert_eq!(waitpid.rule, RULE_BLOCKING_SUBPROCESS);
    assert!(!waitpid.candidate_only);

    for q in [
        "http.client.HTTPConnection.connect",
        "http.client.HTTPConnection.request",
        "http.client.HTTPConnection.getresponse",
        "http.client.HTTPSConnection.connect",
        "http.client.HTTPSConnection.request",
        "http.client.HTTPSConnection.getresponse",
        "HTTPConnection.request",
        "HTTPSConnection.getresponse",
    ] {
        let hit = t
            .lookup(q)
            .unwrap_or_else(|| panic!("expected sink for {q}"));
        assert_eq!(hit.rule, RULE_BLOCKING_HTTP, "wrong rule for {q}");
        assert!(!hit.candidate_only, "{q} must be a finding");
    }
}

#[test]
fn stdlib_zero_fp_guardrails_stay_clean() {
    let t = BuiltinSinkTable::new();
    for q in [
        "typing.IO.read",
        "typing.IO.write",
        "typing.IO.readline",
        "typing.BinaryIO.read",
        "typing.BinaryIO.write",
        "typing.TextIO.read",
        "typing.TextIO.write",
        "subprocess.Popen",
        "subprocess.Popen.__init__",
        "tempfile.NamedTemporaryFile",
        "tempfile.TemporaryFile",
        "tempfile.SpooledTemporaryFile",
        "tempfile.TemporaryDirectory",
        "tempfile.mkstemp",
        "tempfile.mkdtemp",
        "io.BytesIO.read",
        "io.BytesIO.write",
        "io.StringIO.read",
        "io.StringIO.write",
    ] {
        assert!(t.lookup(q).is_none(), "{q} must NOT be a sink");
    }
}

#[test]
fn slow_kdf_apis_grade_strictly_as_candidates_never_findings() {
    let t = BuiltinSinkTable::new();
    for q in [
        "bcrypt.hashpw",
        "bcrypt.checkpw",
        "bcrypt._bcrypt.hashpw",
        "argon2.PasswordHasher.hash",
        "argon2.PasswordHasher.verify",
        "argon2._password_hasher.PasswordHasher.verify",
        "PasswordHasher.hash",
        "PasswordHasher.verify",
        "hashlib.pbkdf2_hmac",
        "hashlib.scrypt",
        "_hashlib.pbkdf2_hmac",
        "_hashlib.scrypt",
    ] {
        let hit = t
            .lookup(q)
            .unwrap_or_else(|| panic!("expected candidate for {q}"));
        assert!(
            hit.candidate_only,
            "{q} must be a candidate (never a finding)"
        );
        assert_eq!(hit.rule, "blocking-cpu", "unexpected rule for {q}");
        assert!(
            hit.candidate_reason.is_some(),
            "{q} candidate must carry a reason"
        );
        assert!(
            t.async_twin(q).is_none(),
            "{q} CPU KDF must never have an async twin"
        );
    }
    assert!(
        rule_doc("blocking-cpu").is_some(),
        "blocking-cpu must be documented in RULE_DOCS"
    );

    // Fast non-KDF helpers and single-pass digests must not match.
    for fast in [
        "bcrypt.gensalt",
        "argon2.PasswordHasher",
        "argon2.PasswordHasher.check_needs_rehash",
        "hashlib.sha256",
        "hashlib.md5",
        "hashlib.blake2b",
    ] {
        assert!(t.lookup(fast).is_none(), "{fast} must not be a sink");
    }
}
