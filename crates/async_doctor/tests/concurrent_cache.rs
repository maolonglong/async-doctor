//! Concurrent cold scans sharing one cache dir must match a solo cold run.
//!
//! Regression for a race where sticky package-cache misses let a process
//! materialize a peer-written sibling without its cross-package follow_targets,
//! silently dropping findings (e.g. 5 blocking-sleep → 1–2, blocking-file gone).
//! Fix: re-probe before snapshot-inconsistency fallback to ty extract so results
//! never depend on peer cache writes (hit/miss may change speed only).
//!
//! Children are truly concurrent (`.spawn()` then `wait_with_output()`). A
//! debug-only internal env widens the miss→materialize window so CI debug
//! builds catch a regression reliably; it is inert in release.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde_json::Value;

/// Debug-only race-widening hook (must match `sitepkgs/batch.rs`). Passed only
/// via `Command::env` on children — never `set_var` in this process.
#[cfg(debug_assertions)]
const TEST_CACHE_RACE_ENV: &str = "__AD_TEST_CACHE_RACE";

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

fn run_json(bin: &Path, project: &Path, cache: &Path) -> String {
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

/// Spawn two scanners that overlap, then wait. Optionally enable the debug
/// race-widening hook on children only.
fn run_pair(bin: &Path, project: &Path, cache: &Path, widen_race: bool) -> [String; 2] {
    let mut children = Vec::with_capacity(2);
    for _ in 0..2 {
        let mut cmd = Command::new(bin);
        cmd.args(["--format", "json"])
            .arg(project)
            .env("XDG_CACHE_HOME", cache)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(debug_assertions)]
        if widen_race {
            cmd.env(TEST_CACHE_RACE_ENV, "1");
        }
        #[cfg(not(debug_assertions))]
        let _ = widen_race;
        children.push(cmd.spawn().expect("spawn async-doctor"));
    }
    let mut jsons = Vec::with_capacity(2);
    for (proc_i, child) in children.into_iter().enumerate() {
        let out = child.wait_with_output().expect("wait async-doctor");
        assert!(
            out.status.success() || out.status.code() == Some(1),
            "proc {proc_i} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        jsons.push(String::from_utf8(out.stdout).unwrap());
    }
    [jsons.pop().unwrap(), jsons.pop().unwrap()]
}

fn run_concurrent_trials(fixture: &str, trials: usize, label: &str) {
    let bin = async_doctor_bin();
    let project = clone_fixture(fixture);
    let solo_cache = fresh_cache(&format!("solo-{label}"));
    let solo = run_json(&bin, &project, &solo_cache);

    // Debug CI: widen the race window. Release: hook is compiled inert.
    let widen = cfg!(debug_assertions);

    let mut mismatches = 0usize;
    for trial in 0..trials {
        let cache = fresh_cache(&format!("pair-{label}-{trial}"));
        let outs = run_pair(&bin, &project, &cache, widen);
        for (proc_i, json) in outs.into_iter().enumerate() {
            if json != solo {
                mismatches += 1;
                assert_same_report(&solo, &json, &format!("trial {trial} proc {proc_i}"));
            }
        }
        let _ = fs::remove_dir_all(&cache);
    }
    assert_eq!(mismatches, 0, "expected 0/{trials} concurrent mismatches");
    let _ = fs::remove_dir_all(&project);
    let _ = fs::remove_dir_all(&solo_cache);
}

/// Fast CI check (N=20). With true parallel spawn + debug race hook, a sticky-miss
/// regression fails this test on the order of ≥9/10 debug runs (measured); with
/// the fix it passes 10/10. The hook is inert in release.
#[test]
fn concurrent_cold_scans_match_solo_multipkg_large() {
    let t0 = Instant::now();
    run_concurrent_trials("m2_cache_multipkg_large", 20, "large");
    eprintln!(
        "concurrent_cold_scans_match_solo_multipkg_large took {:.2}s",
        t0.elapsed().as_secs_f64()
    );
}

/// Fast CI check for the smaller multipkg fixture.
#[test]
fn concurrent_cold_scans_match_solo_multipkg() {
    run_concurrent_trials("m2_cache_multipkg", 15, "multi");
}

/// Stress (N=100). Not run in default CI — see AGENTS.md.
///
/// ```text
/// cargo test -p async_doctor --test concurrent_cache -- --ignored
/// ```
#[test]
#[ignore = "slow stress; run with: cargo test -p async_doctor --test concurrent_cache -- --ignored"]
fn concurrent_cold_scans_match_solo_multipkg_large_stress() {
    run_concurrent_trials("m2_cache_multipkg_large", 100, "large-stress");
}

/// Stress (N=100) for the smaller multipkg fixture.
#[test]
#[ignore = "slow stress; run with: cargo test -p async_doctor --test concurrent_cache -- --ignored"]
fn concurrent_cold_scans_match_solo_multipkg_stress() {
    run_concurrent_trials("m2_cache_multipkg", 100, "multi-stress");
}
