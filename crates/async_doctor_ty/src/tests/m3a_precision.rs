//! M3a higher-order kwargs, type narrowing, and in-memory IO tests.

use std::fs;

use super::helpers::{clone_fixture, fixtures_dir, fresh_cache_dir, temp_project};
use crate::scan_path;
use crate::scan_path_with_cache;

#[test]
fn m3a_higher_order_kwargs_partial_bound() {
    let src = fs::read_to_string(fixtures_dir().join("m3a_higher_order_kwargs.py")).unwrap();
    let dir = temp_project(&[("app.py", &src)]);
    let report = scan_path(&dir).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for expected in [
        "positional_ok",
        "keyword_fn",
        "keyword_partial",
        "bound_method_positional",
        "bound_method_keyword",
    ] {
        assert!(
            names.contains(&expected),
            "missing {expected}: findings={names:?}"
        );
    }
    assert!(
        !names.contains(&"safe_no_call"),
        "non-blocking lambda must stay clean: {names:?}"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn m3a_type_narrowing_isinstance_and_not_none() {
    let src = fs::read_to_string(fixtures_dir().join("m3a_type_narrowing.py")).unwrap();
    let dir = temp_project(&[("app.py", &src)]);
    let report = scan_path(&dir).unwrap();
    let names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for expected in ["narrowed_not_none", "narrowed_isinstance"] {
        assert!(
            names.contains(&expected),
            "missing {expected}: findings={names:?}"
        );
    }
    for banned in ["not_narrowed_ok", "none_branch_ok"] {
        assert!(
            !names.contains(&banned),
            "{banned} must stay clean: {names:?}"
        );
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn m3a_inmemory_buffer_not_file_io() {
    let dir = clone_fixture("m3a_inmemory_io");
    let cache = fresh_cache_dir("m3a-inmem");
    let report = scan_path_with_cache(&dir, Some(&cache)).unwrap();
    let finding_names: Vec<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for banned in ["image_open_bytesio_ok", "image_save_bytesio_ok"] {
        assert!(
            !finding_names.contains(&banned),
            "{banned} must not be a finding: {finding_names:?}"
        );
    }
    // No blocking-file candidates either for proven in-memory buffers.
    for c in &report.candidates {
        if matches!(
            c.entry.name.as_str(),
            "image_open_bytesio_ok" | "image_save_bytesio_ok"
        ) {
            assert_ne!(
                c.rule, "blocking-file",
                "in-memory must not be blocking-file candidate: {c:?}"
            );
        }
    }
    for expected in ["open_path_blocks", "path_read_blocks"] {
        assert!(
            finding_names.contains(&expected),
            "path IO must still fire ({expected}): {finding_names:?}"
        );
    }
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}
