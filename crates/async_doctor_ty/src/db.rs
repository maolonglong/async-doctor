//! Salsa database implementing the traits ty_python_semantic needs.
//!
//! Pattern follows the public TestDb in ty_python_semantic and the documented
//! hydroid adapter approach (crates.io pins + one adapter crate). APIs are
//! those of ty/ruff 0.0.20.

use std::sync::Arc;

use anyhow::{Context, bail};
use ruff_db::Db as SourceDb;
use ruff_db::diagnostic::Diagnostic;
use ruff_db::files::{File, Files};
use ruff_db::system::{OsSystem, System, SystemPath, SystemPathBuf};
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
    system: OsSystem,
    rules: Arc<RuleSelection>,
    analysis: Arc<AnalysisSettings>,
    settings: Arc<ProgramSettings>,
    root: SystemPathBuf,
}

impl AsyncDoctorDb {
    /// Build a database rooted at `root`. Optionally point at a venv / interpreter via `python`.
    pub fn new(root: &SystemPath, python: Option<&SystemPath>) -> anyhow::Result<Self> {
        let system = OsSystem::new(root);
        if !system.is_directory(root) {
            bail!("`{root}` is not a directory");
        }

        let environment = match python {
            Some(path) => {
                let path = if system.is_file(path) {
                    // `.venv/bin/python` → `.venv` (sys.prefix).
                    path.parent()
                        .and_then(SystemPath::parent)
                        .unwrap_or(path)
                        .to_path_buf()
                } else {
                    path.to_path_buf()
                };
                Some(
                    PythonEnvironment::new(&path, SysPrefixPathOrigin::PythonCliFlag, &system)
                        .with_context(|| format!("invalid Python environment `{path}`"))?,
                )
            }
            None => PythonEnvironment::discover(Some(root), &system)
                .context("failed to discover a Python environment")?,
        };

        let site_packages = match &environment {
            Some(env) => env
                .site_packages_paths(&system)
                .context("failed to find site-packages")?
                .into_vec(),
            None => Vec::new(),
        };

        let python_version = environment
            .as_ref()
            .and_then(|env| env.python_version_from_metadata().cloned())
            .unwrap_or_default();

        let mut src_roots = vec![root.to_path_buf()];
        let src = root.join("src");
        if system.is_directory(&src) {
            src_roots.push(src);
        }

        let mut db = Self {
            storage: salsa::Storage::new(None),
            files: Files::default(),
            system,
            rules: Arc::new(RuleSelection::from_registry(default_lint_registry())),
            analysis: Arc::new(AnalysisSettings::default()),
            settings: Arc::new(ProgramSettings::empty(ty_vendored::file_system())),
            root: root.to_path_buf(),
        };

        let search_paths = SearchPathSettings {
            src_roots,
            site_packages_paths: site_packages,
            ..SearchPathSettings::empty()
        }
        .to_search_paths(db.system(), db.vendored(), &FallibleStrategy)
        .context("invalid module search paths")?;

        let settings = ProgramSettings {
            python_version,
            python_platform: Default::default(),
            search_paths,
            python_environment: Ok(environment),
        };
        settings.search_paths.try_register_static_roots(&db);
        db.settings = Arc::new(settings);
        Ok(db)
    }

    pub fn program(&self) -> Program<'_> {
        Program::from_settings(self, &self.settings)
    }

    pub fn root(&self) -> &SystemPath {
        &self.root
    }

    pub fn system_ref(&self) -> &OsSystem {
        &self.system
    }
}

#[salsa::db]
impl salsa::Database for AsyncDoctorDb {}

#[salsa::db]
impl ty_module_resolver::Db for AsyncDoctorDb {}

#[salsa::db]
impl SourceDb for AsyncDoctorDb {
    fn vendored(&self) -> &VendoredFileSystem {
        ty_vendored::file_system()
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
    fn should_check_file(&self, file: File) -> bool {
        !file.path(self).is_vendored_path()
    }
}

#[salsa::db]
impl ty_python_semantic::Db for AsyncDoctorDb {
    fn check_file(&self, _file: File) -> Vec<Diagnostic> {
        Vec::new()
    }

    fn program_file(&self, file: File) -> ProgramFile<'_> {
        self.program().program_file(self, file)
    }

    fn python_version_with_source(&self, _file: File) -> &PythonVersionWithSource {
        &self.settings.python_version
    }

    fn rule_selection(&self, _file: File) -> &RuleSelection {
        &self.rules
    }

    fn lint_registry(&self) -> &LintRegistry {
        default_lint_registry()
    }

    fn analysis_settings(&self, _file: File) -> &AnalysisSettings {
        &self.analysis
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
