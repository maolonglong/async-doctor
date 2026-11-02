//! Test-only helpers that scrub foreign venv env vars for hermetic scans.
//!
//! Production discovery prefers `VIRTUAL_ENV` / `CONDA_PREFIX` over an
//! in-project `.venv`. Parallel `cargo test` under an activated venv would
//! otherwise pick the caller's environment and flake. A shared
//! `XDG_CACHE_HOME` can also serve a stale whole-scan report across tests.

use std::cell::Cell;
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());
thread_local! {
    static HOLDING_ENV_LOCK: Cell<bool> = const { Cell::new(false) };
}

struct SavedEnv {
    virtual_env: Option<std::ffi::OsString>,
    conda_prefix: Option<std::ffi::OsString>,
    xdg_cache_home: Option<std::ffi::OsString>,
    temp_cache: Option<std::path::PathBuf>,
}

fn take_foreign_venv_env() -> SavedEnv {
    let saved = SavedEnv {
        virtual_env: std::env::var_os("VIRTUAL_ENV"),
        conda_prefix: std::env::var_os("CONDA_PREFIX"),
        xdg_cache_home: std::env::var_os("XDG_CACHE_HOME"),
        temp_cache: None,
    };
    let cache = std::env::temp_dir().join(format!(
        "async-doctor-hermetic-cache-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::create_dir_all(&cache);
    // SAFETY: held under ENV_LOCK; only used from `#[cfg(test)]` scan entry points.
    unsafe {
        std::env::remove_var("VIRTUAL_ENV");
        std::env::remove_var("CONDA_PREFIX");
        std::env::set_var("XDG_CACHE_HOME", &cache);
    }
    SavedEnv {
        temp_cache: Some(cache),
        ..saved
    }
}

fn restore_foreign_venv_env(saved: SavedEnv) {
    // SAFETY: paired with take_foreign_venv_env under the same ENV_LOCK.
    unsafe {
        match saved.virtual_env {
            Some(v) => std::env::set_var("VIRTUAL_ENV", v),
            None => std::env::remove_var("VIRTUAL_ENV"),
        }
        match saved.conda_prefix {
            Some(v) => std::env::set_var("CONDA_PREFIX", v),
            None => std::env::remove_var("CONDA_PREFIX"),
        }
        match saved.xdg_cache_home {
            Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
            None => std::env::remove_var("XDG_CACHE_HOME"),
        }
    }
    if let Some(cache) = saved.temp_cache {
        let _ = std::fs::remove_dir_all(cache);
    }
}

/// Run `f` with `VIRTUAL_ENV` / `CONDA_PREFIX` cleared and an isolated
/// `XDG_CACHE_HOME`, restoring afterward. Re-entrant on the same thread so
/// cold/warm timing tests can hold the lock across both scans without measuring
/// `ENV_LOCK` queue wait time.
pub(crate) fn with_clean_venv_env<R>(f: impl FnOnce() -> R) -> R {
    if HOLDING_ENV_LOCK.with(|c| c.get()) {
        return f();
    }
    let _guard: MutexGuard<'_, ()> = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    HOLDING_ENV_LOCK.with(|c| c.set(true));
    let saved = take_foreign_venv_env();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    restore_foreign_venv_env(saved);
    HOLDING_ENV_LOCK.with(|c| c.set(false));
    match result {
        Ok(v) => v,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}
