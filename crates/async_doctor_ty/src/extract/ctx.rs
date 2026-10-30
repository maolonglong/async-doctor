//! Per-file extraction context and shared keys.

use ruff_db::files::File;
use ruff_source_file::OneIndexed;
use ruff_text_size::{TextRange, TextSize};
use ty_python_core::ProgramFile;

use async_doctor_core::FunctionId;

use crate::db::AsyncDoctorDb;

/// Key for looking up a project function by definition site.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DefKey {
    pub(crate) file: String,
    /// Start offset of the function name.
    pub(crate) name_start: u32,
}

pub(crate) struct FileCtx<'db> {
    pub(crate) db: &'db AsyncDoctorDb,
    pub(crate) program_file: ProgramFile<'db>,
    #[allow(dead_code)]
    pub(crate) file: File,
    pub(crate) rel: String,
    pub(crate) module: String,
    pub(crate) source: String,
    pub(crate) index: ruff_source_file::LineIndex,
}

impl<'db> FileCtx<'db> {
    pub(crate) fn line_of(&self, offset: TextSize) -> u32 {
        let line: OneIndexed = self.index.line_index(offset);
        line.get() as u32
    }

    pub(crate) fn snippet(&self, range: TextRange) -> String {
        let start = usize::from(range.start());
        let end = usize::from(range.end()).min(self.source.len());
        let start = start.min(end);
        self.source[start..end].trim().to_string()
    }
}

#[derive(Default)]
pub(crate) struct TaskTargets(pub(crate) std::collections::HashSet<FunctionId>);

pub(crate) fn scope_qualname(class_stack: &[String], fn_stack: &[String], name: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for c in class_stack {
        parts.push(c.as_str());
    }
    for f in fn_stack {
        parts.push(f.as_str());
    }
    parts.push(name);
    parts.join(".")
}
