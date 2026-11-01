//! PR #36 review follow-ups: off-loop guards, genexpr memo, archives, boto meta.

use std::fs;

use super::helpers::clone_fixture;
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
