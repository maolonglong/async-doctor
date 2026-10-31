//! Embed a build identity so whole-scan report cache keys differ across
//! binaries that share the same Cargo package version (dev rebuilds).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{fs, io};

fn main() {
    let git_sha = git_stdout(&["rev-parse", "--short=12", "HEAD"]);
    let dirty_hash = git_diff_hash();

    let build_id = match (&git_sha, dirty_hash) {
        (Some(sha), Some(dirty)) => format!("{sha}-dirty-{dirty:x}"),
        (Some(sha), None) => sha.clone(),
        (None, _) => {
            // No `.git` (wheel / shallow copy): fingerprint sources so edits
            // invalidate whole-scan report reuse across rebuilds.
            format!("nogit-{:x}", source_tree_hash())
        }
    };

    println!("cargo:rustc-env=ASYNC_DOCTOR_BUILD_ID={build_id}");
    println!("cargo:rerun-if-changed=build.rs");

    // Worktree-safe: `--git-dir` is the worktree git dir; `--git-common-dir` holds
    // shared packed-refs / refs/heads (and is equal to git-dir for a normal repo).
    if let Some(git_dir) = git_stdout(&["rev-parse", "--git-dir"]).map(PathBuf::from) {
        emit_rerun_if_exists(&git_dir.join("HEAD"));
    }
    if let Some(common) = git_stdout(&["rev-parse", "--git-common-dir"]).map(PathBuf::from) {
        emit_rerun_if_exists(&common.join("HEAD"));
        emit_rerun_if_exists(&common.join("packed-refs"));
        emit_rerun_if_exists(&common.join("refs/heads"));
    }

    // Dirty-tree / non-git: rebuild when first-party sources change.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    emit_rerun_if_exists(&manifest.join("src"));
    emit_rerun_if_exists(&manifest.join("Cargo.toml"));
}

fn emit_rerun_if_exists(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn git_stdout(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Hash of `git diff HEAD` (tracked + staged); `None` when clean or not a git tree.
fn git_diff_hash() -> Option<u64> {
    let output = Command::new("git")
        .args(["diff", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    if output.stdout.is_empty() {
        // Also catch staged-only / unstaged empty but untracked? Prefer porcelain
        // for "dirty" including untracked that affect the build — but build.rs
        // only cares about source that cargo will compile; `git diff HEAD` covers
        // tracked edits. Fall back to porcelain for unstaged empty with staged.
        let porcelain = Command::new("git")
            .args(["status", "--porcelain"])
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        if porcelain.stdout.is_empty() {
            return None;
        }
        return Some(fnv1a64(&porcelain.stdout));
    }
    Some(fnv1a64(&output.stdout))
}

fn source_tree_hash() -> u64 {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    let _ = collect_rs_files(&root.join("src"), &mut paths);
    paths.push(root.join("build.rs"));
    paths.push(root.join("Cargo.toml"));
    paths.sort();
    let mut h = 0xcbf29ce484222325u64;
    for path in paths {
        h = mix_path_hash(h, &path);
    }
    h
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
    Ok(())
}

fn mix_path_hash(mut h: u64, path: &Path) -> u64 {
    for b in path.to_string_lossy().as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    if let Ok(bytes) = fs::read(path) {
        h = fnv1a64_seeded(h, &bytes);
    }
    h
}

fn fnv1a64(data: &[u8]) -> u64 {
    fnv1a64_seeded(0xcbf29ce484222325, data)
}

fn fnv1a64_seeded(mut h: u64, data: &[u8]) -> u64 {
    for b in data {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
