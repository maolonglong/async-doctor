//! Salsa database that satisfies `ty_python_semantic::Db`.
//!
//! The field layout and trait method bodies follow the public `TestDb` in
//! `ty_python_semantic` 0.0.20 (`src/db.rs`, MIT — Astral / Charlie Marsh),
//! adapted from `TestSystem` to `OsSystem` and wired with
//! `PythonEnvironment::discover` / `SearchPathSettings` the same way that
//! crate's `TestDbBuilder` builds program settings.

use std::sync::Arc;

use anyhow::{Context, bail};
use async_doctor_core::ScanEnvironment;
use ruff_db::Db as SourceDb;
use ruff_db::diagnostic::Diagnostic;
use ruff_db::files::{File, Files};
use ruff_db::system::{System, SystemPath, SystemPathBuf};

use crate::doctor_system::DoctorSystem;
use ruff_db::vendored::VendoredFileSystem;
use ty_module_resolver::SearchPathSettings;
use ty_python_core::ProgramFile;
use ty_python_core::program::{FallibleStrategy, ProgramSettings};
use ty_python_semantic::dependency::DependencyMetadata;
use ty_python_semantic::lint::{LintRegistry, RuleSelection};
use ty_python_semantic::{
    AnalysisSettings, Program, PythonEnvironment, PythonVersionWithSource, default_lint_registry,
};
use ty_site_packages::SysPrefixPathOrigin;

#[salsa::db]
#[derive(Clone)]
pub struct AsyncDoctorDb {
    storage: salsa::Storage<Self>,
    files: Files,
    system: DoctorSystem,
    vendored: VendoredFileSystem,
    rule_selection: Arc<RuleSelection>,
    analysis_settings: Arc<AnalysisSettings>,
    program_settings: ProgramSettings,
    /// Project directory used as the first-party search root.
    project_root: SystemPathBuf,
    /// Extra src roots discovered from config / editable finders (appended
    /// after `project_root`). May point outside the scan root.
    extra_src_roots: Vec<SystemPathBuf>,
    /// Interpreter / venv / site-packages used for this DB.
    scan_environment: ScanEnvironment,
}

impl AsyncDoctorDb {
    /// Open a database for `project_root` (must be a directory).
    ///
    /// When `python` is `None`, environment discovery matches ty
    /// (`PythonEnvironment::discover`). When set, it is treated as a
    /// `--python`-style sys.prefix or interpreter path.
    pub fn open(project_root: &SystemPath, python: Option<&SystemPath>) -> anyhow::Result<Self> {
        let system = DoctorSystem::new(project_root);
        if !system.is_directory(project_root) {
            bail!("`{project_root}` is not a directory");
        }

        let vendored = ty_vendored::file_system().clone();
        let mut db = Self {
            storage: salsa::Storage::new(None),
            files: Files::default(),
            system,
            vendored,
            rule_selection: Arc::new(RuleSelection::default()),
            // Infer unannotated returns so typed receivers like `self.client.get()` resolve;
            // skip `# type: ignore` comment parsing since async-doctor never emits ty diagnostics.
            analysis_settings: Arc::new(AnalysisSettings {
                return_type_inference: ty_python_semantic::ReturnTypeInference::Always,
                respect_type_ignore_comments: false,
                ..AnalysisSettings::default()
            }),
            program_settings: ProgramSettings::empty(ty_vendored::file_system()),
            project_root: project_root.to_path_buf(),
            extra_src_roots: Vec::new(),
            scan_environment: ScanEnvironment::none(),
        };

        let environment = resolve_environment(&db.system, project_root, python)?;
        let site_packages = match &environment {
            Some(env) => env
                .site_packages_paths(&db.system)
                .context("failed to resolve site-packages paths")?
                .into_vec(),
            None => Vec::new(),
        };
        db.scan_environment =
            crate::env_info::summarize_environment(environment.as_ref(), &site_packages);
        let python_version = environment
            .as_ref()
            .and_then(PythonEnvironment::python_version_from_metadata)
            .cloned()
            .unwrap_or_default();

        // First-party roots: project root, plus `src/` when present (common layout).
        let mut src_roots = vec![project_root.to_path_buf()];
        let src_layout = project_root.join("src");
        if db.system.is_directory(&src_layout) {
            src_roots.push(src_layout);
        }

        // Scanning a package directory itself (`pkg/` with `__init__.py`): add
        // the parent so `import pkg.…` self-imports resolve.
        let pkg_init = project_root.join("__init__.py");
        if db.system.is_file(&pkg_init)
            && let Some(parent) = project_root.parent()
        {
            let parent_buf = parent.to_path_buf();
            if !src_roots.iter().any(|r| r == &parent_buf) {
                src_roots.push(parent_buf);
            }
        }

        // Nested packages (`pkg/pkg/`) + editable finders: pull IDE/tool search
        // roots and setuptools `__editable__*_finder.py` MAPPING parents.
        // Append after existing src_roots (never as front-loaded extra_paths)
        // and skip duplicates of roots already present (e.g. ruff `src = ["."]`).
        // Outside-scan-root paths stay on the search path for resolution/follow
        // but are never walked as first-party entry sources.
        let discovered =
            crate::search_paths::discover_extra_paths(project_root, &site_packages, &src_roots);
        db.extra_src_roots = discovered.clone();
        src_roots.extend(discovered);

        let search_paths = SearchPathSettings {
            src_roots,
            site_packages_paths: site_packages,
            ..SearchPathSettings::empty()
        }
        .to_search_paths(db.system(), db.vendored(), &FallibleStrategy)
        .context("invalid module search paths")?;

        let program_settings = ProgramSettings {
            python_version,
            python_platform: Default::default(),
            search_paths,
            python_environment: Ok(environment),
        };
        program_settings.search_paths.try_register_static_roots(&db);
        db.program_settings = program_settings;
        Ok(db)
    }

    pub fn program(&self) -> Program<'_> {
        Program::from_settings(self, &self.program_settings)
    }

    /// New empty salsa storage with the same search paths / settings.
    ///
    /// Used after pass 1 (release the discovery DB before follow), on pass-2
    /// panic recovery, between `ASYNC_DOCTOR_PASS2_BATCH` chunks, and during
    /// third-party follow. Do **not** call this periodically on the default
    /// parallel path: each call allocates a new `Arc<Zalsa>`, and retaining
    /// retired DBs multiplies peak RSS (see
    /// `docs/notes/2026-11-salsa-fresh-empty-rss.md`).
    pub(crate) fn fresh_empty(&self) -> Self {
        #[cfg(test)]
        FRESH_EMPTY_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let db = Self {
            storage: salsa::Storage::new(None),
            files: Files::default(),
            system: self.system.fresh(),
            vendored: self.vendored.clone(),
            rule_selection: Arc::clone(&self.rule_selection),
            analysis_settings: Arc::clone(&self.analysis_settings),
            program_settings: self.program_settings.clone(),
            project_root: self.project_root.clone(),
            extra_src_roots: self.extra_src_roots.clone(),
            scan_environment: self.scan_environment.clone(),
        };
        db.program_settings
            .search_paths
            .try_register_static_roots(&db);
        db
    }

    pub fn project_root(&self) -> &SystemPath {
        &self.project_root
    }

    /// Interpreter / venv / site-packages discovered for this database.
    pub fn scan_environment(&self) -> &ScanEnvironment {
        &self.scan_environment
    }

    /// Extra search roots from config / editable finders (may be outside the
    /// scan root). On the module path for resolution; outside roots are
    /// follow-only (never first-party entries).
    pub fn extra_src_roots(&self) -> &[SystemPathBuf] {
        &self.extra_src_roots
    }

    /// Extra roots that lie outside the scan root — followable like site-packages.
    pub fn followable_extra_roots(&self) -> Vec<SystemPathBuf> {
        let project = std::path::Path::new(self.project_root.as_str());
        self.extra_src_roots
            .iter()
            .filter(|r| !std::path::Path::new(r.as_str()).starts_with(project))
            .cloned()
            .collect()
    }

    /// Clear `ParsedModule` ASTs for all system files read so far on this DB.
    ///
    /// Salsa's `parsed_module` LRU only evicts across revisions, not within a
    /// single scan revision. `ParsedModule::clear` drops the `IndexedModule`
    /// behind its `ArcSwapOption` without invalidating `AstNodeRef` indices.
    pub(crate) fn clear_all_parsed_modules(&self) {
        let paths = self.system.snapshot_read_paths();
        let py_ver = self.program_settings.python_version.version;
        for p in paths {
            if let Some(file) = self.files.try_system(self, &p) {
                let pf = ruff_db::PythonFile::new(self, file, py_ver);
                ruff_db::parsed::parsed_module(self, pf).clear();
            }
        }
        release_freed_memory();
    }
}

/// Hint the platform allocator to mark freed magazine pages reusable after bulk
/// AST or Salsa arena drops.
pub(crate) fn release_freed_memory() {
    #[cfg(target_os = "macos")]
    unsafe {
        unsafe extern "C" {
            fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        }
        malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
}

/// Resolve the Python environment for `project_root` (no `--python` override).
pub(crate) fn resolve_environment_for_project(
    system: &dyn System,
    project_root: &SystemPath,
) -> anyhow::Result<Option<PythonEnvironment>> {
    resolve_environment(system, project_root, None)
}

fn resolve_environment(
    system: &dyn System,
    project_root: &SystemPath,
    python: Option<&SystemPath>,
) -> anyhow::Result<Option<PythonEnvironment>> {
    match python {
        Some(selected) => {
            let prefix = sys_prefix_from_python_arg(system, selected);
            let env = PythonEnvironment::new(&prefix, SysPrefixPathOrigin::PythonCliFlag, system)
                .with_context(|| format!("invalid Python environment `{selected}`"))?;
            Ok(Some(env))
        }
        None => {
            // Prefer VIRTUAL_ENV / conda / `.venv` via ty's discover.
            // uv and poetry in-project virtualenvs typically use `.venv`.
            let discovered = PythonEnvironment::discover(Some(project_root), system)
                .context("Python environment discovery failed")?;

            // discover only auto-probes `.venv`. Also try `venv/` (common alias),
            // and retry `.venv` when discover returned None.
            if discovered.is_some() {
                // When VIRTUAL_ENV is set, keep discover's result.
                if system.env_var("VIRTUAL_ENV").is_ok() {
                    return Ok(discovered);
                }
                // Prefer an in-project `venv/` when present and discover fell
                // through to a non-local environment (`.venv` missing/broken).
                let venv = project_root.join("venv");
                if system.is_directory(&venv)
                    && let Ok(env) =
                        PythonEnvironment::new(&venv, SysPrefixPathOrigin::LocalVenv, system)
                {
                    // Only override when discover did not already pick `.venv`.
                    let dot_venv = project_root.join(".venv");
                    if !system.is_directory(&dot_venv) {
                        return Ok(Some(env));
                    }
                }
                return Ok(discovered);
            }

            for name in ["venv", ".venv"] {
                let path = project_root.join(name);
                if system.is_directory(&path)
                    && let Ok(env) =
                        PythonEnvironment::new(&path, SysPrefixPathOrigin::LocalVenv, system)
                {
                    return Ok(Some(env));
                }
            }
            Ok(None)
        }
    }
}

/// Map a `--python`-style path to a sys.prefix directory.
///
/// Interpreters live at `<prefix>/bin/python`; a directory argument is used as-is.
fn sys_prefix_from_python_arg(system: &dyn System, path: &SystemPath) -> SystemPathBuf {
    if system.is_file(path) {
        path.parent()
            .and_then(SystemPath::parent)
            .unwrap_or(path)
            .to_path_buf()
    } else {
        path.to_path_buf()
    }
}

#[salsa::db]
impl salsa::Database for AsyncDoctorDb {}

#[salsa::db]
impl ty_module_resolver::Db for AsyncDoctorDb {}

#[salsa::db]
impl SourceDb for AsyncDoctorDb {
    fn vendored(&self) -> &VendoredFileSystem {
        &self.vendored
    }

    fn system(&self) -> &dyn System {
        &self.system
    }

    fn files(&self) -> &Files {
        &self.files
    }
}

#[salsa::db]
impl ty_python_core::Db for AsyncDoctorDb {
    fn should_check_file(&self, _file: File) -> bool {
        // async-doctor only queries SemanticModel types/definitions; returning
        // false skips ty's post-inference lint checks and diagnostic allocations.
        false
    }
}

#[salsa::db]
impl ty_python_semantic::Db for AsyncDoctorDb {
    fn check_file(&self, _file: File) -> Vec<Diagnostic> {
        // async-doctor does not run ty's type checker; only the semantic model.
        Vec::new()
    }

    fn program_file(&self, file: File) -> ProgramFile<'_> {
        self.program().program_file(self, file)
    }

    fn python_version_with_source(&self, _file: File) -> &PythonVersionWithSource {
        &self.program_settings.python_version
    }

    fn rule_selection(&self, _file: File) -> &RuleSelection {
        &self.rule_selection
    }

    fn lint_registry(&self) -> &LintRegistry {
        default_lint_registry()
    }

    fn analysis_settings(&self, _file: File) -> &AnalysisSettings {
        &self.analysis_settings
    }

    fn dependency_metadata(&self, _file: File) -> Option<&DependencyMetadata> {
        None
    }

    fn verbose(&self) -> bool {
        false
    }

    fn is_open_file(&self, _file: File) -> bool {
        false
    }

    fn dyn_clone(&self) -> Box<dyn ty_python_semantic::Db> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
static FRESH_EMPTY_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
impl AsyncDoctorDb {
    /// Test seam: how many times [`Self::fresh_empty`] ran in this process.
    pub(crate) fn fresh_empty_count() -> usize {
        FRESH_EMPTY_COUNT.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn reset_fresh_empty_count() {
        FRESH_EMPTY_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}
