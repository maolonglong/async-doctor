//! Record unresolved `import` / `from … import` sites (bounded, first-party).

use std::path::{Path, PathBuf};

use async_doctor_core::UnresolvedImport;
use ruff_python_ast::{self as ast, Stmt};
use ruff_text_size::Ranged;

use super::CallCollector;

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(crate) fn note_import_stmt(&mut self, stmt: &ast::StmtImport) {
        if !self.record_unresolved_imports {
            return;
        }
        for alias in &stmt.names {
            let module = alias.name.as_str();
            if self.model.resolve_module(Some(module), 0).is_none() {
                self.classify_failed_import(alias.range(), module, /*relative=*/ false);
            } else {
                self.note_resolved_third_party_if_applicable(head_module(module));
            }
        }
    }

    pub(crate) fn note_import_from_stmt(&mut self, stmt: &ast::StmtImportFrom) {
        if !self.record_unresolved_imports {
            return;
        }
        let module = stmt.module.as_ref().map(|m| m.as_str());
        let level = stmt.level;
        if self.model.resolve_module(module, level).is_some() {
            if level == 0
                && let Some(m) = module
            {
                self.note_resolved_third_party_if_applicable(head_module(m));
            }
            return;
        }
        // When scanning a package dir itself, ty often fails relative imports
        // even though the target file exists. Check the filesystem against the
        // importing file's package path before emitting unresolved-import.
        if level > 0 && self.relative_import_exists_on_disk(level, module) {
            return;
        }
        let key = import_from_module_key(module, level);
        self.classify_failed_import(stmt.range(), &key, level > 0);
    }

    /// `from .…` / `from ..…` target present on disk relative to this file.
    fn relative_import_exists_on_disk(&self, level: u32, module: Option<&str>) -> bool {
        let Some(base) = relative_import_base(
            Path::new(self.ctx.db.project_root().as_str()),
            &self.ctx.rel,
            level,
        ) else {
            return false;
        };
        match module {
            None | Some("") => package_dir_exists(&base),
            Some(m) => module_exists_under(&base, m),
        }
    }

    fn classify_failed_import(
        &mut self,
        range: ruff_text_size::TextRange,
        module: &str,
        relative: bool,
    ) {
        let head = head_module(module);
        // Relative imports are always a first-party concern.
        if relative || self.head_exists_as_first_party(head) {
            self.push_unresolved_import(range, module);
        } else if !head.is_empty() {
            self.note_missing_third_party(head);
        }
    }

    fn head_exists_as_first_party(&self, head: &str) -> bool {
        let project = Path::new(self.ctx.db.project_root().as_str());
        if first_party_head_at(project, head) {
            return true;
        }
        let src_dir = project.join("src");
        if src_dir.is_dir() && first_party_head_at(&src_dir, head) {
            return true;
        }
        for root in self.ctx.db.extra_src_roots() {
            if first_party_head_at(Path::new(root.as_str()), head) {
                return true;
            }
        }
        // Scan root *is* the package named `head`.
        project.file_name().and_then(|s| s.to_str()) == Some(head)
            && project.join("__init__.py").is_file()
    }

    fn note_missing_third_party(&mut self, head: &str) {
        if self
            .facts
            .unresolved_third_party_modules
            .iter()
            .any(|m| m == head)
        {
            return;
        }
        self.facts
            .unresolved_third_party_modules
            .push(head.to_string());
    }

    fn note_resolved_third_party_if_applicable(&mut self, head: &str) {
        if head.is_empty() || self.head_exists_as_first_party(head) {
            return;
        }
        if self
            .facts
            .resolved_third_party_modules
            .iter()
            .any(|m| m == head)
        {
            return;
        }
        self.facts
            .resolved_third_party_modules
            .push(head.to_string());
    }

    fn push_unresolved_import(&mut self, range: ruff_text_size::TextRange, module: &str) {
        let line = self.ctx.line_of(range.start());
        let text = self.ctx.snippet(range);
        let file = self.ctx.rel.clone();
        // Dedupe per (file, module) — top-level + function-local same miss → 1.
        if self
            .facts
            .unresolved_imports
            .iter()
            .any(|u| u.file == file && u.module == module)
        {
            return;
        }
        self.facts.unresolved_imports.push(UnresolvedImport {
            file,
            line,
            module: module.to_string(),
            text,
        });
    }
}

fn head_module(module: &str) -> &str {
    module
        .trim_start_matches('.')
        .split('.')
        .next()
        .unwrap_or("")
}

fn first_party_head_at(root: &Path, head: &str) -> bool {
    if head.is_empty() {
        return false;
    }
    let as_dir = root.join(head);
    let as_py = root.join(format!("{head}.py"));
    as_dir.is_dir() || as_py.is_file()
}

/// Package directory that a relative import with `level` dots starts from,
/// given the importing file's project-relative path.
fn relative_import_base(project_root: &Path, file_rel: &str, level: u32) -> Option<PathBuf> {
    if level == 0 {
        return None;
    }
    let file = project_root.join(file_rel.replace('\\', "/"));
    // Package containing this module = parent of the file (works for both
    // `pkg/mod.py` and `pkg/__init__.py`).
    let mut base = file.parent()?.to_path_buf();
    // level=1 → stay; level=2 → one parent up; …
    for _ in 1..level {
        base = base.parent()?.to_path_buf();
    }
    Some(base)
}

fn package_dir_exists(dir: &Path) -> bool {
    // Regular packages (`__init__.py`) and PEP 420 namespace packages (bare dirs).
    dir.is_dir()
}

fn module_exists_under(base: &Path, dotted: &str) -> bool {
    if dotted.is_empty() {
        return package_dir_exists(base);
    }
    let rel = dotted.replace('.', "/");
    let as_pkg = base.join(&rel);
    if package_dir_exists(&as_pkg) {
        return true;
    }
    let as_py = base.join(format!("{rel}.py"));
    let as_pyi = base.join(format!("{rel}.pyi"));
    as_py.is_file() || as_pyi.is_file()
}

fn import_from_module_key(module: Option<&str>, level: u32) -> String {
    let dots = ".".repeat(level as usize);
    match module {
        Some(m) if !m.is_empty() => format!("{dots}{m}"),
        _ if dots.is_empty() => "<unknown>".into(),
        _ => dots,
    }
}

/// Dispatch import recording from the visitor.
pub(crate) fn note_if_import(collector: &mut CallCollector<'_, '_>, stmt: &Stmt) {
    match stmt {
        Stmt::Import(i) => collector.note_import_stmt(i),
        Stmt::ImportFrom(i) => collector.note_import_from_stmt(i),
        _ => {}
    }
}
