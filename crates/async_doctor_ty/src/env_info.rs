//! Lightweight Python environment summary for reports (cold extract + warm cache).

use async_doctor_core::ScanEnvironment;
use ruff_db::system::{OsSystem, SystemPath};

use crate::env_discovery::{DiscoveredEnvironment, discover_environment};

/// Discover the interpreter/venv/site-packages used for `project_root`.
///
/// Cheap enough to re-run on whole-scan cache hits so `environment.origin`
/// reflects the live `VIRTUAL_ENV` / `.venv` choice rather than a stale cached
/// label.
pub fn discover_scan_environment(project_root: &SystemPath) -> anyhow::Result<ScanEnvironment> {
    discover_scan_environment_with_python(project_root, None)
}

/// Discover the interpreter/venv/site-packages used for `project_root` with an
/// optional explicit `--python` path.
pub fn discover_scan_environment_with_python(
    project_root: &SystemPath,
    python: Option<&SystemPath>,
) -> anyhow::Result<ScanEnvironment> {
    let system = OsSystem::new(project_root);
    let discovered = discover_environment(&system, project_root, python)?;
    Ok(summarize_discovered_environment(&discovered))
}

pub(crate) fn summarize_discovered_environment(
    discovered: &DiscoveredEnvironment,
) -> ScanEnvironment {
    let site_packages: Vec<String> = discovered
        .site_packages
        .iter()
        .map(|p| p.as_str().to_string())
        .collect();
    let frameworks = detect_frameworks(&site_packages);
    discovered.to_scan_environment(frameworks)
}

/// Async frameworks / runtimes worth surfacing in the environment banner.
/// Client and database libraries (httpx, redis, sqlalchemy, …) are omitted —
/// they are dependencies, not frameworks.
const KNOWN_FRAMEWORKS: &[&str] = &[
    "aiohttp",
    "anyio",
    "arq",
    "celery",
    "channels",
    "django",
    "dramatiq",
    "fastapi",
    "litestar",
    "quart",
    "sanic",
    "starlette",
    "tornado",
    "trio",
    "uvloop",
];

fn detect_frameworks(site_packages: &[String]) -> Vec<String> {
    let mut found = std::collections::BTreeMap::<String, String>::new();
    for sp in site_packages {
        let Ok(entries) = std::fs::read_dir(sp) else {
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            let Some(stem) = name.strip_suffix(".dist-info") else {
                continue;
            };
            let Some((pkg, ver)) = stem.split_once('-') else {
                continue;
            };
            let norm = pkg.to_ascii_lowercase().replace('_', "-");
            if KNOWN_FRAMEWORKS.binary_search(&norm.as_str()).is_ok() {
                found
                    .entry(norm.clone())
                    .or_insert_with(|| format!("{norm} {ver}"));
            }
        }
    }
    found.into_values().collect()
}
