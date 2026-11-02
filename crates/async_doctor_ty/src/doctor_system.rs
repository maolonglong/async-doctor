//! [`OsSystem`] wrapper that decodes Python sources per PEP 263 / BOM.
//!
//! Ty/ruff require UTF-8 `read_to_string`. This adapter reads bytes and runs
//! [`crate::encoding::decode_python_source`] so latin-1 (etc.) files with a
//! coding cookie are analysed instead of silently becoming empty sources.

use std::any::Any;

use ruff_db::system::walk_directory::WalkDirectoryBuilder;
use ruff_db::system::{
    DirectoryEntry, Metadata, OsSystem, Result, System, SystemPath, SystemPathBuf,
    SystemVirtualPath, WhichResult, WritableSystem,
};
use ruff_notebook::{Notebook, NotebookError};

use crate::encoding::decode_python_source;

/// OS filesystem with PEP 263-aware `read_to_string`.
#[derive(Debug, Clone)]
pub struct DoctorSystem {
    inner: OsSystem,
    read_paths: std::sync::Arc<std::sync::Mutex<Vec<SystemPathBuf>>>,
}

impl DoctorSystem {
    pub fn new(cwd: impl AsRef<SystemPath>) -> Self {
        Self {
            inner: OsSystem::new(cwd),
            read_paths: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub(crate) fn snapshot_read_paths(&self) -> Vec<SystemPathBuf> {
        self.read_paths.lock().unwrap().clone()
    }

    pub(crate) fn fresh(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            read_paths: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn read_decoded(path: &std::path::Path) -> Result<String> {
        let bytes = std::fs::read(path)?;
        decode_python_source(&bytes)
            .map_err(|msg| std::io::Error::new(std::io::ErrorKind::InvalidData, msg))
    }
}

impl System for DoctorSystem {
    fn path_metadata(&self, path: &SystemPath) -> Result<Metadata> {
        self.inner.path_metadata(path)
    }

    fn canonicalize_path(&self, path: &SystemPath) -> Result<SystemPathBuf> {
        self.inner.canonicalize_path(path)
    }

    fn is_same_file(&self, first: &SystemPath, second: &SystemPath) -> Result<bool> {
        self.inner.is_same_file(first, second)
    }

    fn which(&self, binary_name: &str) -> WhichResult {
        self.inner.which(binary_name)
    }

    fn read_to_string(&self, path: &SystemPath) -> Result<String> {
        let res = Self::read_decoded(path.as_std_path());
        if res.is_ok() {
            self.read_paths.lock().unwrap().push(path.to_path_buf());
        }
        res
    }

    fn read_to_notebook(&self, path: &SystemPath) -> std::result::Result<Notebook, NotebookError> {
        self.inner.read_to_notebook(path)
    }

    fn read_virtual_path_to_string(&self, path: &SystemVirtualPath) -> Result<String> {
        self.inner.read_virtual_path_to_string(path)
    }

    fn read_virtual_path_to_notebook(
        &self,
        path: &SystemVirtualPath,
    ) -> std::result::Result<Notebook, NotebookError> {
        self.inner.read_virtual_path_to_notebook(path)
    }

    fn path_exists(&self, path: &SystemPath) -> bool {
        self.inner.path_exists(path)
    }

    fn current_directory(&self) -> &SystemPath {
        self.inner.current_directory()
    }

    fn user_config_directory(&self) -> Option<SystemPathBuf> {
        self.inner.user_config_directory()
    }

    fn cache_dir(&self) -> Option<SystemPathBuf> {
        self.inner.cache_dir()
    }

    fn read_directory<'a>(
        &'a self,
        path: &SystemPath,
    ) -> Result<Box<dyn Iterator<Item = Result<DirectoryEntry>> + 'a>> {
        self.inner.read_directory(path)
    }

    fn walk_directory(&self, path: &SystemPath) -> WalkDirectoryBuilder {
        self.inner.walk_directory(path)
    }

    fn env_var(&self, name: &str) -> std::result::Result<String, std::env::VarError> {
        self.inner.env_var(name)
    }

    fn as_writable(&self) -> Option<&dyn WritableSystem> {
        // Writes go straight to the OS; decoding is read-only.
        self.inner.as_writable()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn dyn_clone(&self) -> Box<dyn System> {
        Box::new(self.clone())
    }

    fn command_executor(&self) -> Option<&dyn ruff_db::system::CommandExecutor> {
        self.inner.command_executor()
    }
}
