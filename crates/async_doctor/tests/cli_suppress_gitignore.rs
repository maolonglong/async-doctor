//! Regression: suppressions must not walk gitignored `.py` trees.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use tempfile::TempDir;

fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_async-doctor"));
    cmd.env_remove("VIRTUAL_ENV").env_remove("CONDA_PREFIX");
    // Isolated cache: caller XDG_CACHE_HOME / prior runs must not affect results.
    let cache = std::env::temp_dir().join(format!(
        "async-doctor-test-cache-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&cache);
    cmd.env("XDG_CACHE_HOME", cache);
    cmd
}

fn git(dir: &Path, args: &[&str]) {
    let st = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "async-doctor")
        .env("GIT_AUTHOR_EMAIL", "async-doctor@users.noreply.github.com")
        .env("GIT_COMMITTER_NAME", "async-doctor")
        .env(
            "GIT_COMMITTER_EMAIL",
            "async-doctor@users.noreply.github.com",
        )
        .status()
        .expect("git");
    assert!(st.success(), "git {args:?} failed");
}

/// Ignore comments inside a gitignored tree must not become unused suppressions
/// and must not be read at all (behavioral + cheap scale check).
#[test]
fn gitignored_py_tree_not_scanned_for_suppressions() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-b", "main"]);
    git(root, &["config", "user.email", "a@b.c"]);
    git(root, &["config", "user.name", "a"]);

    fs::write(root.join(".gitignore"), "vendor/\n").unwrap();
    fs::write(root.join("app.py"), "async def handler():\n    return 1\n").unwrap();

    let vendor = root.join("vendor");
    fs::create_dir_all(&vendor).unwrap();
    // Enough files that a naive recursive walk+read is clearly slower than a
    // gitignore-aware discovery, without making CI painful.
    const N: usize = 4_000;
    for i in 0..N {
        fs::write(
            vendor.join(format!("gen_{i:04}.py")),
            format!("# async-doctor: ignore[blocking-sleep]: should not be seen {i}\nx = {i}\n"),
        )
        .unwrap();
    }

    git(root, &["add", ".gitignore", "app.py"]);
    git(root, &["commit", "-m", "init"]);

    let t0 = Instant::now();
    let out = bin().args(["--format", "json"]).arg(root).output().unwrap();
    let elapsed = t0.elapsed();

    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr={} stdout={}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let unused = v
        .get("unusedSuppressions")
        .and_then(|u| u.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[]);
    assert!(
        unused.is_empty(),
        "gitignored ignore comments must not be reported as unused: {unused:?}"
    );
    // Naive walk of 4k ignored files was hundreds of ms; gitignore-aware stays
    // near a tiny project scan. Bound loosely for slow CI hosts.
    assert!(
        elapsed.as_millis() < 5_000,
        "suppress walk appears to be reading the gitignored tree: {elapsed:?}"
    );
}
