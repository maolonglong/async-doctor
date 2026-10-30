//! Concurrent cold scans sharing one cache dir must match a solo cold run.
//!
//! Regression for a race where sticky package-cache misses let a process
//! materialize a peer-written sibling without its cross-package follow_targets,
//! silently dropping findings (e.g. 5 blocking-sleep → 1–2, blocking-file gone).
//! Fix: re-probe before snapshot-inconsistency fallback to ty extract so results
//! never depend on peer cache writes (hit/miss may change speed only).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

fn async_doctor_bin() -> PathBuf {
    // Binary name is `async-doctor`; Cargo's CARGO_BIN_EXE_* key keeps the hyphen
    // and cannot be read via env!() — use the runtime var instead.
    std::env::var_os("CARGO_BIN_EXE_async-doctor")
        .or_else(|| std::env::var_os("CARGO_BIN_EXE_async_doctor"))
        .map(PathBuf::from)
        .expect("CARGO_BIN_EXE_async-doctor not set (run via cargo test)")
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), to).unwrap();
        }
    }
}

fn clone_fixture(name: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let i = N.fetch_add(1, Ordering::Relaxed);
    let dst = std::env::temp_dir().join(format!(
        "async-doctor-concurrent-{name}-{}-{i}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dst);
    copy_dir(&fixtures_dir().join(name), &dst);
    dst
}

fn fresh_cache(label: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let i = N.fetch_add(1, Ordering::Relaxed);
    let cache = std::env::temp_dir().join(format!(
        "async-doctor-concurrent-cache-{label}-{}-{i}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&cache);
    fs::create_dir_all(&cache).unwrap();
    cache
}

fn run_json(bin: &str, project: &Path, cache: &Path) -> String {
    let out = Command::new(bin)
        .args(["--format", "json"])
        .arg(project)
        .env("XDG_CACHE_HOME", cache)
        .output()
        .expect("spawn async-doctor");
    assert!(
        out.status.success() || out.status.code() == Some(1),
        "async-doctor failed: status={:?} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8 json")
}

fn assert_same_report(solo: &str, concurrent: &str, ctx: &str) {
    assert_eq!(
        solo, concurrent,
        "{ctx}: concurrent cold JSON must equal solo cold\nsolo={solo}\nconcurrent={concurrent}"
    );
    let solo_v: Value = serde_json::from_str(solo).unwrap();
    let conc_v: Value = serde_json::from_str(concurrent).unwrap();
    assert_eq!(
        solo_v["findings"].as_array().map(|a| a.len()),
        conc_v["findings"].as_array().map(|a| a.len()),
        "{ctx}: findings count"
    );
    assert_eq!(
        solo_v["candidates"].as_array().map(|a| a.len()),
        conc_v["candidates"].as_array().map(|a| a.len()),
        "{ctx}: candidates count"
    );
}

/// CI-sized loop: two processes, shared cache, many trials.
///
/// Pre-fix (b341 sticky miss, no snapshot fallback): ~2–5% wrong on
/// m2_cache_multipkg_large (e.g. 2/80, 4/80). N=100 keeps CI false-pass risk low;
/// local stress uses ≥300 iterations.
#[test]
fn concurrent_cold_scans_match_solo_multipkg_large() {
    let bin = async_doctor_bin();
    let project = clone_fixture("m2_cache_multipkg_large");
    let solo_cache = fresh_cache("solo-large");
    let solo = run_json(bin.to_str().unwrap(), &project, &solo_cache);

    const TRIALS: usize = 100;
    let mut mismatches = 0usize;
    for trial in 0..TRIALS {
        let cache = fresh_cache(&format!("pair-large-{trial}"));
        let mut children = Vec::new();
        for _ in 0..2 {
            children.push(
                Command::new(&bin)
                    .args(["--format", "json"])
                    .arg(&project)
                    .env("XDG_CACHE_HOME", &cache)
                    .output()
                    .expect("spawn"),
            );
        }
        for (proc_i, out) in children.into_iter().enumerate() {
            assert!(
                out.status.success() || out.status.code() == Some(1),
                "trial {trial} proc {proc_i} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let json = String::from_utf8(out.stdout).unwrap();
            if json != solo {
                mismatches += 1;
                assert_same_report(&solo, &json, &format!("trial {trial} proc {proc_i}"));
            }
        }
        let _ = fs::remove_dir_all(&cache);
    }
    assert_eq!(mismatches, 0, "expected 0/{TRIALS} concurrent mismatches");
    let _ = fs::remove_dir_all(&project);
    let _ = fs::remove_dir_all(&solo_cache);
}

#[test]
fn concurrent_cold_scans_match_solo_multipkg() {
    let bin = async_doctor_bin();
    let project = clone_fixture("m2_cache_multipkg");
    let solo_cache = fresh_cache("solo-multi");
    let solo = run_json(bin.to_str().unwrap(), &project, &solo_cache);

    const TRIALS: usize = 80;
    for trial in 0..TRIALS {
        let cache = fresh_cache(&format!("pair-multi-{trial}"));
        let outs: Vec<_> = (0..2)
            .map(|_| {
                Command::new(&bin)
                    .args(["--format", "json"])
                    .arg(&project)
                    .env("XDG_CACHE_HOME", &cache)
                    .output()
                    .expect("spawn")
            })
            .collect();
        for (proc_i, out) in outs.into_iter().enumerate() {
            assert!(
                out.status.success() || out.status.code() == Some(1),
                "trial {trial} proc {proc_i} failed"
            );
            let json = String::from_utf8(out.stdout).unwrap();
            assert_same_report(&solo, &json, &format!("trial {trial} proc {proc_i}"));
        }
        let _ = fs::remove_dir_all(&cache);
    }
    let _ = fs::remove_dir_all(&project);
    let _ = fs::remove_dir_all(&solo_cache);
}
