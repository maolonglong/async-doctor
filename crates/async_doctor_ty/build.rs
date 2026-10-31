//! Embed a fallback build identity when the running executable cannot be hashed
//! at runtime (see [`async_doctor_ty::cache::build_id`]). Prefer the runtime
//! exe hash for whole-scan cache keys — it tracks every linked crate.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let git_sha = git_stdout(&["rev-parse", "--short=12", "HEAD"]);
    let dirty = git_stdout(&["status", "--porcelain"]).filter(|s| !s.is_empty());
    let build_id = match (&git_sha, dirty) {
        (Some(sha), Some(_)) => format!("{sha}-dirty"),
        (Some(sha), None) => sha.clone(),
        (None, _) => format!("nogit-{}", env!("CARGO_PKG_VERSION")),
    };

    println!("cargo:rustc-env=ASYNC_DOCTOR_BUILD_ID={build_id}");
    println!("cargo:rerun-if-changed=build.rs");

    if let Some(git_dir) = git_stdout(&["rev-parse", "--git-dir"]).map(PathBuf::from) {
        emit_rerun_if_exists(&git_dir.join("HEAD"));
    }
    if let Some(common) = git_stdout(&["rev-parse", "--git-common-dir"]).map(PathBuf::from) {
        emit_rerun_if_exists(&common.join("HEAD"));
        emit_rerun_if_exists(&common.join("packed-refs"));
        emit_rerun_if_exists(&common.join("refs/heads"));
    }

    // Fallback stamp only — runtime exe hash is authoritative. Still watch the
    // workspace so ASYNC_DOCTOR_BUILD_ID moves when sources change without a
    // full clean rebuild of this crate alone.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest.join("../..");
    emit_rerun_if_exists(&workspace.join("Cargo.toml"));
    emit_rerun_if_exists(&workspace.join("Cargo.lock"));
    for crate_name in [
        "async_doctor",
        "async_doctor_core",
        "async_doctor_rules",
        "async_doctor_ty",
    ] {
        let root = workspace.join("crates").join(crate_name);
        emit_rerun_if_exists(&root.join("src"));
        emit_rerun_if_exists(&root.join("Cargo.toml"));
    }
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
