//! Off-loop thread guards, genexpr memo, in-memory archives, and boto meta helpers.

use std::fs;

use super::helpers::{clone_fixture, temp_project};
use crate::scan_path;

#[test]
fn p1_off_loop_thread_and_done_guards() {
    let dir = clone_fixture("off_loop_thread");
    let report = scan_path(&dir).unwrap();
    let names: std::collections::BTreeSet<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    assert!(
        names.contains("unguarded_result_blocks"),
        "bare Future.result must still fire: {names:?} {:?}",
        report.findings
    );
    assert!(
        names.contains("lock_owner_get_ident_still_blocks"),
        "lk.owner != get_ident() must NOT be treated as off-loop: {names:?} {:?}",
        report.findings
    );
    assert!(
        names.contains("done_guard_rebind_still_blocks"),
        "rebind inside if fut.done(): must invalidate the guard: {names:?} {:?}",
        report.findings
    );
    for clean in [
        "off_loop_get_ident_result_ok",
        "off_loop_attr_loop_thread_id_ok",
        "off_loop_main_thread_result_ok",
        "on_loop_else_result_ok",
        "done_guard_result_ok",
        "done_guard_attr_and_ok",
        "done_guard_ternary_ok",
        "done_guard_attr_if_ok",
    ] {
        assert!(
            !names.contains(clean),
            "{clean} must stay clean under off-loop/.done() guards: {names:?} {:?}",
            report.findings
        );
    }
    let proof_findings: std::collections::BTreeSet<_> = names
        .iter()
        .copied()
        .filter(|name| name.starts_with("proof_"))
        .collect();
    assert_eq!(
        proof_findings,
        std::collections::BTreeSet::from([
            "proof_custom_if_blocks",
            "proof_custom_and_blocks",
            "proof_custom_ternary_blocks",
            "proof_overridden_done_blocks",
            "proof_overridden_result_blocks",
            "proof_deferred_parameter_blocks",
            "proof_deferred_closure_blocks",
            "proof_attribute_rebind_blocks",
            "proof_annotated_attribute_rebind_blocks",
            "proof_nested_rebind_blocks",
            "proof_walrus_rebind_blocks",
            "proof_test_rebind_blocks",
            "proof_positive_or_blocks",
            "proof_conditional_return_blocks",
            "proof_fallthrough_blocks",
            "proof_unrelated_receiver_blocks",
            "proof_for_rebind_blocks",
            "proof_while_backedge_blocks",
            "proof_zero_iterations_blocks",
            "proof_exceptional_path_blocks",
            "proof_handler_fallthrough_blocks",
            "proof_finally_blocks",
            "proof_match_sibling_blocks",
            "proof_match_binding_blocks",
            "proof_failed_match_guard_blocks",
            "proof_import_rebind_blocks",
            "proof_suppressed_exception_blocks",
            "proof_comprehension_binding_blocks",
            "proof_comprehension_walrus_blocks",
        ]),
        "only completed same-receiver Future calls may be exempt: {:?}",
        report.findings
    );
    for name in [
        "proof_custom_if_blocks",
        "proof_custom_and_blocks",
        "proof_custom_ternary_blocks",
        "proof_overridden_result_blocks",
    ] {
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.entry.name == name && f.rule == "blocking-sleep"),
            "custom result() bodies must remain reachable: {name}"
        );
    }
    let waits: std::collections::BTreeSet<_> = report
        .findings
        .iter()
        .filter(|f| f.entry.name.starts_with("wait_"))
        .map(|f| (f.entry.name.as_str(), f.rule.as_str()))
        .collect();
    assert_eq!(
        waits,
        std::collections::BTreeSet::from([
            ("wait_default_exception_blocks", "blocking-lock"),
            ("wait_pending_exception_blocks", "blocking-lock"),
            ("wait_inherited_exception_blocks", "blocking-lock"),
            ("wait_rebound_exception_blocks", "blocking-lock"),
            ("wait_misleading_done_exception_blocks", "blocking-lock"),
            ("wait_custom_exception_blocks", "blocking-sleep"),
            ("wait_positive_result_blocks", "blocking-lock"),
            ("wait_dynamic_exception_blocks", "blocking-lock"),
            ("wait_poll_eager_receiver_blocks", "blocking-sleep"),
            ("wait_unbound_exception_blocks", "blocking-lock"),
            ("wait_true_timeout_exception_blocks", "blocking-lock"),
        ]),
        "pending waits and custom/eager work must report; completed/zero-timeout/asyncio calls must not"
    );
    assert!(
        report
            .candidates
            .iter()
            .all(|c| !c.entry.name.starts_with("wait_")),
        "safe Future calls must not be demoted to candidates: {:?}",
        report.candidates
    );
    assert!(report.complete && report.skipped.is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn p1_genexpr_memo_clears_across_consume_branches() {
    let dir = clone_fixture("genexpr-consume-branches");
    let report = scan_path(&dir).unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.entry.name == "consume_else_branch" && f.rule == "blocking-sleep"),
        "else-branch list(g) must report sleep after offloaded consume: {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn p1_inmemory_archive_and_ctor_call_collapse() {
    let dir = clone_fixture("inmemory_archive");
    let report = scan_path(&dir).unwrap();
    let names: std::collections::BTreeSet<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    assert!(
        !names.contains("zip_bytesio_ok"),
        "ZipFile(BytesIO)+writestr must stay clean: {names:?} {:?}",
        report.findings
    );
    assert!(
        !names.contains("zip_bytesio_inline_ok"),
        "{names:?} {:?}",
        report.findings
    );
    assert!(
        !names.contains("tar_bytesio_ok"),
        "{names:?} {:?}",
        report.findings
    );
    for blocked in [
        "zip_bytesio_write_path_blocks",
        "zip_bytesio_extractall_blocks",
        "tar_bytesio_add_blocks",
        "zip_path_blocks",
    ] {
        assert!(
            names.contains(blocked),
            "{blocked} must still fire (disk-touching / path archive): {names:?} {:?}",
            report.findings
        );
    }
    let zip_n = report
        .findings
        .iter()
        .filter(|f| f.entry.name == "zip_path_blocks")
        .count();
    assert_eq!(
        zip_n,
        1,
        "path ZipFile + writestr must merge to one finding: {:?}",
        report
            .findings
            .iter()
            .filter(|f| f.entry.name == "zip_path_blocks")
            .collect::<Vec<_>>()
    );
    let smtp = report
        .findings
        .iter()
        .find(|f| f.entry.name == "ctor_sendmail_one_finding")
        .expect("SMTP finding");
    assert_eq!(
        report
            .findings
            .iter()
            .filter(|f| f.entry.name == "ctor_sendmail_one_finding")
            .count(),
        1,
        "SMTP(...).sendmail must be one finding: {:?}",
        report.findings
    );
    let last = smtp.chain.last().expect("chain");
    assert!(
        last.text.contains("sendmail"),
        "merged finding must terminate on method sink, got {:?}",
        smtp.chain
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn p1_boto_meta_helpers_excluded() {
    let dir = clone_fixture("boto_meta_helpers");
    let report = scan_path(&dir).unwrap();
    let names: std::collections::BTreeSet<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    assert!(
        !names.contains("paginator_waiter_ok"),
        "get_paginator/get_waiter/meta must stay clean: {names:?} {:?}",
        report.findings
    );
    assert!(
        names.contains("get_object_blocks"),
        "get_object must still fire: {names:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn sqlite_in_memory_suppressed_while_disk_sqlite_still_reports() {
    let dir = temp_project(&[(
        "db.py",
        r#"
import sqlite3

async def mem_basic_ok():
    conn = sqlite3.connect(":memory:")
    conn.execute("CREATE TABLE t (x INT)")
    cur = conn.cursor()
    cur.execute("INSERT INTO t VALUES (1)")
    cur.fetchall()
    conn.commit()
    conn.close()

async def mem_uri_ok():
    conn = sqlite3.connect("file:memdb1?mode=memory&cache=shared", uri=True)
    with conn:
        conn.execute("SELECT 1")
    conn2 = sqlite3.connect("file::memory:?cache=shared", uri=True)
    conn2.cursor().execute("SELECT 1")

async def mem_with_inline_ok():
    with sqlite3.connect(":memory:") as conn:
        conn.execute("SELECT 1")
    sqlite3.connect(":memory:").execute("SELECT 1")

async def disk_blocks():
    conn = sqlite3.connect("app.db")
    conn.execute("SELECT 1")

async def uri_false_mode_memory_blocks():
    conn = sqlite3.connect("file:memdb1?mode=memory", uri=False)
    conn.execute("SELECT 1")

async def reassigned_to_disk_blocks():
    conn = sqlite3.connect(":memory:")
    conn = sqlite3.connect("app.db")
    conn.execute("SELECT 1")

async def conditional_mem_only_blocks(flag: bool):
    conn = sqlite3.connect("app.db")
    if flag:
        conn = sqlite3.connect(":memory:")
    conn.execute("SELECT 1")
"#,
    )]);
    let report = scan_path(&dir).unwrap();
    let _ = fs::remove_dir_all(&dir);

    let names: std::collections::BTreeSet<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for clean in ["mem_basic_ok", "mem_uri_ok", "mem_with_inline_ok"] {
        assert!(
            !names.contains(clean),
            "{clean} uses in-memory SQLite and must not emit blocking-db: {:?}",
            report.findings
        );
    }
    for blocked in [
        "disk_blocks",
        "uri_false_mode_memory_blocks",
        "reassigned_to_disk_blocks",
        "conditional_mem_only_blocks",
    ] {
        assert!(
            names.contains(blocked),
            "{blocked} touches disk SQLite and must report blocking-db: {names:?}"
        );
    }
}

#[test]
fn nonblocking_socket_suppressed_and_reblocking_reports() {
    let dir = temp_project(&[(
        "sock.py",
        r#"
import socket

async def setblocking_false_ok(addr: tuple[str, int]):
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setblocking(False)
    sock.connect(addr)
    sock.send(b"ping")
    sock.recv(1024)
    sock.sendto(b"ping", addr)
    sock.recvfrom(1024)
    sock.accept()

async def settimeout_zero_ok():
    s1 = socket.socket()
    s1.settimeout(0)
    s1.recv(64)
    s2 = socket.socket()
    s2.settimeout(0.0)
    s2.send(b"ok")

async def default_socket_blocks():
    sock = socket.socket()
    sock.recv(1024)

async def reblocked_true_blocks():
    sock = socket.socket()
    sock.setblocking(False)
    sock.setblocking(True)
    sock.recv(1024)

async def nonzero_timeout_blocks():
    sock = socket.socket()
    sock.settimeout(0)
    sock.settimeout(2.5)
    sock.recv(1024)

async def branch_only_nonblocking_blocks(flag: bool):
    sock = socket.socket()
    if flag:
        sock.setblocking(False)
    sock.recv(1024)

async def reassigned_socket_blocks():
    sock = socket.socket()
    sock.setblocking(False)
    sock = socket.socket()
    sock.recv(1024)
"#,
    )]);
    let report = scan_path(&dir).unwrap();
    let _ = fs::remove_dir_all(&dir);

    let names: std::collections::BTreeSet<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for clean in ["setblocking_false_ok", "settimeout_zero_ok"] {
        assert!(
            !names.contains(clean),
            "{clean} uses non-blocking socket and must stay clean: {:?}",
            report.findings
        );
    }
    for blocked in [
        "default_socket_blocks",
        "reblocked_true_blocks",
        "nonzero_timeout_blocks",
        "branch_only_nonblocking_blocks",
        "reassigned_socket_blocks",
    ] {
        assert!(
            names.contains(blocked),
            "{blocked} may block and must report blocking-socket: {names:?}"
        );
    }
}

#[test]
fn tarfile_addfile_union_buffers_and_buffered_io_base() {
    let dir = temp_project(&[
        (".venv/pyvenv.cfg", "home = /usr/bin\n"),
        (".venv/lib/python3.12/site-packages/PIL/__init__.py", ""),
        (
            ".venv/lib/python3.12/site-packages/PIL/Image.py",
            "import builtins\ndef open(fp, mode='r'):\n    return builtins.open('header.bin', 'rb')\n",
        ),
        (
            "archives.py",
            r#"
import io
import tarfile
from PIL import Image

async def tar_addfile_inmem_ok():
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tf:
        ti = tarfile.TarInfo("hello.txt")
        tf.addfile(ti)
        tf.addfile(ti, None)
        tf.addfile(ti, io.BytesIO(b"hello"))
        tf.addfile(ti, fileobj=io.BytesIO(b"world"))

async def tar_addfile_disk_fileobj_blocks(stream: io.BufferedIOBase):
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tf:
        ti = tarfile.TarInfo("hello.txt")
        tf.addfile(ti, fileobj=stream)

async def tar_disk_archive_addfile_blocks():
    with tarfile.open("out.tar", mode="w") as tf:
        ti = tarfile.TarInfo("hello.txt")
        tf.addfile(ti, io.BytesIO(b"hello"))

async def union_all_inmem_ok(flag: bool):
    buf: io.BytesIO | io.StringIO = io.BytesIO(b"x") if flag else io.StringIO("x")
    Image.open(buf)
    raw: io.BytesIO | bytearray = io.BytesIO(b"x") if flag else bytearray(b"x")
    Image.open(raw)

async def union_mixed_with_file_blocks(buf: io.BytesIO | io.TextIOWrapper):
    Image.open(buf)

async def buffered_iobase_blocks(stream: io.BufferedIOBase):
    Image.open(stream)

async def buffered_reader_from_open_blocks():
    f = open("image.png", "rb")
    Image.open(f)
"#,
        ),
    ]);
    let report = scan_path(&dir).unwrap();
    let _ = fs::remove_dir_all(&dir);

    let names: std::collections::BTreeSet<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for clean in ["tar_addfile_inmem_ok", "union_all_inmem_ok"] {
        assert!(
            !names.contains(clean),
            "{clean} is strictly in-memory and must stay clean: {:?}",
            report.findings
        );
    }
    for blocked in [
        "tar_addfile_disk_fileobj_blocks",
        "tar_disk_archive_addfile_blocks",
        "union_mixed_with_file_blocks",
        "buffered_iobase_blocks",
    ] {
        assert!(
            names.contains(blocked),
            "{blocked} can perform disk IO and must report: {names:?}"
        );
    }
    let open_findings: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.entry.name == "buffered_reader_from_open_blocks")
        .collect();
    assert!(
        open_findings
            .iter()
            .any(|f| f.chain.iter().any(|s| s.text.contains("Image.open"))),
        "Image.open on BufferedReader from open('rb') must not be skipped as BytesIO: {open_findings:?}"
    );
}
