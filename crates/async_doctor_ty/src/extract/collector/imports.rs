//! Record unresolved `import` / `from … import` sites (bounded, first-party).

use std::path::Path;

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
            return;
        }
        let key = import_from_module_key(module, level);
        self.classify_failed_import(stmt.range(), &key, level > 0);
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
