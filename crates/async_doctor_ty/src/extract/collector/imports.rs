//! Record unresolved `import` / `from … import` sites (once each).

use async_doctor_core::UnresolvedImport;
use ruff_python_ast::{self as ast, Stmt};
use ruff_text_size::Ranged;

use super::CallCollector;

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(crate) fn note_import_stmt(&mut self, stmt: &ast::StmtImport) {
        for alias in &stmt.names {
            let module = alias.name.as_str();
            if self.model.resolve_module(Some(module), 0).is_none() {
                self.push_unresolved_import(alias.range(), module);
            }
        }
    }

    pub(crate) fn note_import_from_stmt(&mut self, stmt: &ast::StmtImportFrom) {
        let module = stmt.module.as_ref().map(|m| m.as_str());
        let level = stmt.level;
        if self.model.resolve_module(module, level).is_some() {
            return;
        }
        let key = import_from_module_key(module, level);
        self.push_unresolved_import(stmt.range(), &key);
    }

    fn push_unresolved_import(&mut self, range: ruff_text_size::TextRange, module: &str) {
        let line = self.ctx.line_of(range.start());
        let text = self.ctx.snippet(range);
        let file = self.ctx.rel.clone();
        if self
            .facts
            .unresolved_imports
            .iter()
            .any(|u| u.file == file && u.line == line && u.module == module)
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
