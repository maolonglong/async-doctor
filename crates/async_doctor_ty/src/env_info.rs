//! Lightweight Python environment discovery for reports (cold extract + warm cache).

use anyhow::Context;
use async_doctor_core::{EnvironmentKind, ScanEnvironment};
use ruff_db::system::{OsSystem, SystemPath, SystemPathBuf};
use ty_python_semantic::PythonEnvironment;
use ty_site_packages::SysPrefixPathOrigin;

use crate::db::resolve_environment_for_project;

/// Discover the interpreter/venv/site-packages used for `project_root`.
///
/// Cheap enough to re-run on whole-scan cache hits so `environment.origin`
/// reflects the live `VIRTUAL_ENV` / `.venv` choice rather than a stale cached
/// label.
pub fn discover_scan_environment(project_root: &SystemPath) -> anyhow::Result<ScanEnvironment> {
    let system = OsSystem::new(project_root);
    let environment = resolve_environment_for_project(&system, project_root)?;
    let site_packages = match &environment {
        Some(env) => env
            .site_packages_paths(&system)
            .context("failed to resolve site-packages paths")?
            .into_vec(),
        None => Vec::new(),
    };
    Ok(summarize_environment(environment.as_ref(), &site_packages))
}

pub(crate) fn summarize_environment(
    environment: Option<&PythonEnvironment>,
    site_packages: &[SystemPathBuf],
) -> ScanEnvironment {
    let site_packages: Vec<String> = site_packages
        .iter()
        .map(|p| p.as_str().to_string())
        .collect();
    let Some(env) = environment else {
        let mut out = ScanEnvironment::none();
        out.site_packages = site_packages;
        out.refresh_note();
        return out;
    };

    let prefix = env.sys_prefix().as_str().to_string();
    let origin = format_origin(env.origin());
    let kind = if env.is_virtual() {
        EnvironmentKind::Venv
    } else {
        EnvironmentKind::System
    };
    let interpreter = guess_interpreter(&prefix);
    let frameworks = detect_frameworks(&site_packages);
    let mut out = ScanEnvironment {
        kind,
        origin,
        prefix: Some(prefix),
        interpreter,
        site_packages,
        frameworks,
        unresolved_third_party: 0,
        resolved_third_party: 0,
        note: String::new(),
    };
    out.refresh_note();
    out
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

fn format_origin(origin: &SysPrefixPathOrigin) -> String {
    match origin {
        SysPrefixPathOrigin::VirtualEnvVar => "VIRTUAL_ENV".into(),
        SysPrefixPathOrigin::LocalVenv => "local .venv".into(),
        SysPrefixPathOrigin::CondaPrefixVar => "CONDA_PREFIX".into(),
        SysPrefixPathOrigin::PythonCliFlag => "--python".into(),
        SysPrefixPathOrigin::PythonBinary => "PATH python".into(),
        SysPrefixPathOrigin::SelfEnvironment => "self".into(),
        SysPrefixPathOrigin::UvMetadata => "uv".into(),
        SysPrefixPathOrigin::DerivedFromPyvenvCfg => "pyvenv.cfg".into(),
        SysPrefixPathOrigin::Editor => "editor".into(),
        SysPrefixPathOrigin::ScriptMetadataSetting => "script metadata".into(),
        SysPrefixPathOrigin::ConfigFileSetting(..) => "config".into(),
    }
}

/// Prefer `<prefix>/bin/python` without resolving the leaf symlink to the
/// base interpreter (e.g. keep `.venv/bin/python`, not `/usr/bin/python3.13`).
fn guess_interpreter(prefix: &str) -> Option<String> {
    let root = std::path::Path::new(prefix);
    for rel in ["bin/python", "bin/python3", "Scripts/python.exe"] {
        let p = root.join(rel);
        // `is_file` follows the symlink for existence but we keep the venv path.
        if p.is_file() || std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_symlink()) {
            return Some(p.display().to_string());
        }
    }
    None
}
