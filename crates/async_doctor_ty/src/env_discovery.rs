//! Unified Python environment discovery across `AsyncDoctorDb`, `scan_cache`,
//! `env_info`, and `twin_resolve`.

mod external;

#[cfg(test)]
pub(crate) use external::sha256_base64url8;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use async_doctor_core::{EnvironmentKind, ScanEnvironment};
use ruff_db::system::{OsSystem, System, SystemPath, SystemPathBuf};
use ty_python_semantic::PythonEnvironment;
use ty_site_packages::SysPrefixPathOrigin;

use external::{probe_external_pipenv_venv, probe_external_poetry_venv, probe_pdm_pypackages};

/// Resolved Python environment and its ordered `site-packages` / `dist-packages` roots.
pub(crate) struct DiscoveredEnvironment {
    pub python_environment: Option<PythonEnvironment>,
    pub kind: EnvironmentKind,
    pub origin: String,
    pub prefix: Option<SystemPathBuf>,
    pub interpreter: Option<String>,
    pub site_packages: Vec<SystemPathBuf>,
}

impl DiscoveredEnvironment {
    pub fn none() -> Self {
        Self {
            python_environment: None,
            kind: EnvironmentKind::None,
            origin: String::new(),
            prefix: None,
            interpreter: None,
            site_packages: Vec::new(),
        }
    }

    pub fn to_scan_environment(&self, frameworks: Vec<String>) -> ScanEnvironment {
        let site_packages: Vec<String> = self
            .site_packages
            .iter()
            .map(|p| p.as_str().to_string())
            .collect();
        let mut out = if self.kind == EnvironmentKind::None {
            let mut empty = ScanEnvironment::none();
            empty.site_packages = site_packages;
            empty
        } else {
            ScanEnvironment {
                kind: self.kind,
                origin: self.origin.clone(),
                prefix: self.prefix.as_ref().map(|p| p.as_str().to_string()),
                interpreter: self.interpreter.clone(),
                site_packages,
                frameworks,
                unresolved_third_party: 0,
                resolved_third_party: 0,
                unresolved_third_party_modules: Vec::new(),
                note: String::new(),
                follow_budget_skipped: 0,
            }
        };
        out.refresh_note();
        out
    }
}

/// Discover the Python environment for `scan_root`.
pub(crate) fn discover_environment(
    system: &dyn System,
    scan_root: &SystemPath,
    python: Option<&SystemPath>,
) -> anyhow::Result<DiscoveredEnvironment> {
    if let Some(selected) = python {
        if !system.path_exists(selected) {
            anyhow::bail!("Python environment `{selected}` does not exist");
        }
        let interpreter_override = system
            .is_file(selected)
            .then(|| selected.as_str().to_string());
        let prefix = sys_prefix_from_python_arg(system, selected);
        if let Ok(env) = PythonEnvironment::new(&prefix, SysPrefixPathOrigin::PythonCliFlag, system)
            && let Ok(sp) = env.site_packages_paths(system)
        {
            let roots = merge_prefix_roots(system, env.sys_prefix(), sp.into_vec());
            return Ok(from_ty_environment(env, roots, interpreter_override));
        }
        let roots = site_packages_roots_for_prefix(system, &prefix);
        let is_venv = system.is_file(&prefix.join("pyvenv.cfg"));
        if !roots.is_empty() || is_venv {
            let ty_env =
                PythonEnvironment::new(&prefix, SysPrefixPathOrigin::PythonCliFlag, system).ok();
            return Ok(DiscoveredEnvironment {
                python_environment: ty_env,
                kind: if is_venv {
                    EnvironmentKind::Venv
                } else {
                    EnvironmentKind::System
                },
                origin: "--python".to_string(),
                prefix: Some(prefix.clone()),
                interpreter: interpreter_override.or_else(|| guess_interpreter(prefix.as_str())),
                site_packages: roots,
            });
        }
        anyhow::bail!(
            "invalid Python environment `{selected}` (expected a virtualenv prefix, sys.prefix, or python interpreter)"
        );
    }

    if let Some(venv_var) = non_empty_env(system, "VIRTUAL_ENV")
        && let Some(found) = probe_venv_prefix(
            system,
            &SystemPathBuf::from(venv_var),
            SysPrefixPathOrigin::VirtualEnvVar,
            "VIRTUAL_ENV",
        )
    {
        return Ok(found);
    }

    for anc in scan_root.ancestors() {
        for name in [".venv", "venv"] {
            let cand = anc.join(name);
            if system.is_directory(&cand)
                && let Some(found) =
                    probe_venv_prefix(system, &cand, SysPrefixPathOrigin::LocalVenv, "local .venv")
            {
                return Ok(found);
            }
        }
    }
    for anc in scan_root.ancestors() {
        if let Some(found) = probe_pdm_pypackages(system, anc) {
            return Ok(found);
        }
    }
    for anc in scan_root.ancestors() {
        if let Some(found) = probe_external_poetry_venv(system, anc)
            .or_else(|| probe_external_pipenv_venv(system, anc))
        {
            return Ok(found);
        }
    }

    if let Some(conda_var) = non_empty_env(system, "CONDA_PREFIX")
        && let Some(found) = probe_venv_prefix(
            system,
            &SystemPathBuf::from(conda_var),
            SysPrefixPathOrigin::CondaPrefixVar,
            "CONDA_PREFIX",
        )
    {
        return Ok(found);
    }

    Ok(probe_path_python(system).unwrap_or_else(DiscoveredEnvironment::none))
}

/// Path-level helper so `scan_cache` and `twin_resolve` inspect the exact same
/// environment and `site-packages` / `dist-packages` roots as `AsyncDoctorDb`.
pub(crate) fn discover_site_packages_for_path(
    scan_root: &Path,
    python: Option<&Path>,
) -> (EnvironmentKind, Option<PathBuf>, Vec<PathBuf>) {
    let abs = dunce::canonicalize(scan_root)
        .or_else(|_| std::path::absolute(scan_root))
        .unwrap_or_else(|_| scan_root.to_path_buf());
    let Ok(sys_root) = SystemPathBuf::from_path_buf(abs) else {
        return (EnvironmentKind::None, None, Vec::new());
    };
    let sys_python = python.and_then(|p| {
        let p_abs = dunce::canonicalize(p)
            .or_else(|_| std::path::absolute(p))
            .unwrap_or_else(|_| p.to_path_buf());
        SystemPathBuf::from_path_buf(p_abs).ok()
    });
    if python.is_some() && sys_python.is_none() {
        return (EnvironmentKind::None, None, Vec::new());
    }
    let system = OsSystem::new(&sys_root);
    let Ok(env) = discover_environment(&system, &sys_root, sys_python.as_deref()) else {
        return (EnvironmentKind::None, None, Vec::new());
    };
    let prefix = env.prefix.map(SystemPathBuf::into_std_path_buf);
    let roots = env
        .site_packages
        .into_iter()
        .map(SystemPathBuf::into_std_path_buf)
        .collect();
    (env.kind, prefix, roots)
}

pub(super) fn non_empty_env(system: &dyn System, key: &str) -> Option<String> {
    system.env_var(key).ok().filter(|v| !v.is_empty())
}

fn merge_prefix_roots(
    system: &dyn System,
    prefix: &SystemPath,
    mut primary: Vec<SystemPathBuf>,
) -> Vec<SystemPathBuf> {
    for extra in site_packages_roots_for_prefix(system, prefix) {
        if !primary.contains(&extra) {
            primary.push(extra);
        }
    }
    primary
}

pub(super) fn probe_venv_prefix(
    system: &dyn System,
    cand: &SystemPath,
    origin: SysPrefixPathOrigin,
    fallback_label: &str,
) -> Option<DiscoveredEnvironment> {
    if !system.is_directory(cand) {
        return None;
    }
    if let Ok(env) = PythonEnvironment::new(cand, origin, system)
        && let Ok(sp) = env.site_packages_paths(system)
    {
        let roots = merge_prefix_roots(system, env.sys_prefix(), sp.into_vec());
        return Some(from_ty_environment(env, roots, None));
    }
    let roots = site_packages_roots_for_prefix(system, cand);
    if roots.is_empty() && !system.is_file(&cand.join("pyvenv.cfg")) {
        return None;
    }
    let ty_env = PythonEnvironment::new(cand, SysPrefixPathOrigin::PythonCliFlag, system).ok();
    Some(DiscoveredEnvironment {
        python_environment: ty_env,
        kind: EnvironmentKind::Venv,
        origin: fallback_label.to_string(),
        prefix: Some(cand.to_path_buf()),
        interpreter: guess_interpreter(cand.as_str()),
        site_packages: roots,
    })
}

fn probe_path_python(system: &dyn System) -> Option<DiscoveredEnvironment> {
    let mut candidates: Vec<SystemPathBuf> = Vec::new();
    if let Ok(path_var) = system.env_var("PATH") {
        let names: &[&str] = if cfg!(windows) {
            &["python.exe", "python3.exe"]
        } else {
            &["python3", "python"]
        };
        for dir in std::env::split_paths(&path_var) {
            for name in names {
                if let Ok(sys_exe) = SystemPathBuf::from_path_buf(dir.join(name))
                    && system.is_file(&sys_exe)
                {
                    candidates.push(sys_exe);
                }
            }
        }
    } else {
        for name in ["python3", "python"] {
            if let Ok(exe) = system.which(name) {
                candidates.push(exe);
            }
        }
    }

    let mut seen = BTreeSet::new();
    for exe in candidates {
        if !seen.insert(exe.clone()) {
            continue;
        }
        let raw_prefix = sys_prefix_from_python_arg(system, &exe);
        if is_ephemeral_tool_python(system, &exe, &raw_prefix) {
            continue;
        }
        if let Ok(env) = PythonEnvironment::new(&exe, SysPrefixPathOrigin::PythonBinary, system)
            && let Ok(sp) = env.site_packages_paths(system)
        {
            let roots = merge_prefix_roots(system, env.sys_prefix(), sp.into_vec());
            if !roots.is_empty() {
                return Some(from_ty_environment(
                    env,
                    roots,
                    Some(exe.as_str().to_string()),
                ));
            }
        }
        let fallback_roots = site_packages_roots_for_prefix(system, &raw_prefix);
        if !fallback_roots.is_empty() {
            let is_venv = system.is_file(&raw_prefix.join("pyvenv.cfg"));
            let ty_env =
                PythonEnvironment::new(&raw_prefix, SysPrefixPathOrigin::PythonCliFlag, system)
                    .ok();
            return Some(DiscoveredEnvironment {
                python_environment: ty_env,
                kind: if is_venv {
                    EnvironmentKind::Venv
                } else {
                    EnvironmentKind::System
                },
                origin: "PATH python".into(),
                prefix: Some(raw_prefix),
                interpreter: Some(exe.as_str().to_string()),
                site_packages: fallback_roots,
            });
        }
    }
    None
}

fn is_ephemeral_tool_python(system: &dyn System, exe: &SystemPath, prefix: &SystemPath) -> bool {
    for raw in [exe.as_str(), prefix.as_str()] {
        let norm = raw.replace('\\', "/");
        if norm.contains("/uv/archive-v0/")
            || norm.contains("/uv/tools/")
            || norm.contains("/pipx/venvs/")
        {
            return true;
        }
    }
    let mut has_async_doctor = false;
    let mut has_other_pkg = false;
    for sp in site_packages_roots_for_prefix(system, prefix) {
        let Ok(entries) = std::fs::read_dir(sp.as_std_path()) else {
            continue;
        };
        for ent in entries.flatten() {
            let fname = ent.file_name();
            let Some(stem) = fname.to_str().and_then(|s| s.strip_suffix(".dist-info")) else {
                continue;
            };
            let pkg = stem
                .split('-')
                .next()
                .unwrap_or(stem)
                .to_ascii_lowercase()
                .replace('_', "-");
            if pkg == "async-doctor" {
                has_async_doctor = true;
            } else if !matches!(pkg.as_str(), "pip" | "setuptools" | "wheel" | "-virtualenv") {
                has_other_pkg = true;
            }
        }
    }
    has_async_doctor && !has_other_pkg
}

fn from_ty_environment(
    env: PythonEnvironment,
    site_packages: Vec<SystemPathBuf>,
    interpreter_override: Option<String>,
) -> DiscoveredEnvironment {
    let prefix_buf = env.sys_prefix().to_path_buf();
    let interpreter = interpreter_override.or_else(|| guess_interpreter(prefix_buf.as_str()));
    DiscoveredEnvironment {
        kind: if env.is_virtual() {
            EnvironmentKind::Venv
        } else {
            EnvironmentKind::System
        },
        origin: format_origin(env.origin()),
        prefix: Some(prefix_buf),
        interpreter,
        site_packages,
        python_environment: Some(env),
    }
}

/// Enumerate sorted, deduplicated `site-packages` and `dist-packages` directories under `prefix`.
pub(crate) fn site_packages_roots_for_prefix(
    system: &dyn System,
    prefix: &SystemPath,
) -> Vec<SystemPathBuf> {
    let mut roots = Vec::new();
    for base in [prefix.to_path_buf(), prefix.join("local")] {
        for lib_dir in ["lib", "lib64"] {
            let Ok(entries) = system.read_directory(&base.join(lib_dir)) else {
                continue;
            };
            for ent in entries.flatten().filter(|e| e.file_type().is_directory()) {
                let path = ent.into_path();
                if path
                    .file_name()
                    .is_some_and(|n| n.starts_with("python") || n.starts_with("pypy"))
                {
                    for pkg_dir in ["site-packages", "dist-packages"] {
                        let cand = path.join(pkg_dir);
                        if system.is_directory(&cand) {
                            roots.push(cand);
                        }
                    }
                }
            }
        }
    }
    let win_sp = prefix.join("Lib").join("site-packages");
    if system.is_directory(&win_sp) {
        roots.push(win_sp);
    }
    roots.sort();
    roots.dedup();
    roots
}

fn sys_prefix_from_python_arg(system: &dyn System, path: &SystemPath) -> SystemPathBuf {
    if system.is_file(path)
        && let Some(bin_dir) = path.parent()
    {
        if matches!(bin_dir.file_name(), Some("bin" | "Scripts"))
            && let Some(prefix) = bin_dir.parent()
        {
            return prefix.to_path_buf();
        }
        return bin_dir.to_path_buf();
    }
    path.to_path_buf()
}

fn format_origin(origin: &SysPrefixPathOrigin) -> String {
    match origin {
        SysPrefixPathOrigin::VirtualEnvVar => "VIRTUAL_ENV",
        SysPrefixPathOrigin::LocalVenv => "local .venv",
        SysPrefixPathOrigin::CondaPrefixVar => "CONDA_PREFIX",
        SysPrefixPathOrigin::PythonCliFlag => "--python",
        SysPrefixPathOrigin::PythonBinary => "PATH python",
        SysPrefixPathOrigin::SelfEnvironment => "self",
        SysPrefixPathOrigin::UvMetadata => "uv",
        SysPrefixPathOrigin::DerivedFromPyvenvCfg => "pyvenv.cfg",
        SysPrefixPathOrigin::Editor => "editor",
        SysPrefixPathOrigin::ScriptMetadataSetting => "script metadata",
        SysPrefixPathOrigin::ConfigFileSetting(..) => "config",
    }
    .into()
}

pub(super) fn guess_interpreter(prefix: &str) -> Option<String> {
    let root = Path::new(prefix);
    for rel in ["bin/python", "bin/python3", "Scripts/python.exe"] {
        let p = root.join(rel);
        if p.is_file() || std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_symlink()) {
            return Some(p.display().to_string());
        }
    }
    None
}
