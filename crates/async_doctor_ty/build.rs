//! Embed a build identity so whole-scan report cache keys differ across
//! binaries that share the same Cargo package version (dev rebuilds).

use std::process::Command;

fn main() {
    let git = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let build_id = git.unwrap_or_else(|| {
        // Fallback when `.git` is absent (packaged wheel / shallow copy): hash
        // of this build script path + package version so rebuilds of the same
        // sources without git still share a stable id within one tree.
        format!(
            "nogit-{}",
            simple_hash(concat!(
                env!("CARGO_MANIFEST_DIR"),
                env!("CARGO_PKG_VERSION")
            ))
        )
    });

    println!("cargo:rustc-env=ASYNC_DOCTOR_BUILD_ID={build_id}");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");
}

fn simple_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
