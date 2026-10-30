//! M0 scanner: report direct `time.sleep(...)` calls inside `async def`.
//!
//! Uses the ty Db + ruff AST (via `parsed_module`). Skips sync `def` bodies,
//! lambda bodies (deferred), and dead branches under `if False` /
//! `TYPE_CHECKING` / `typing.TYPE_CHECKING` (and aliases of the `typing` module).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use async_doctor_core::{
    ChainStep, Entry, EntryLabel, Finding, FindingIdParts, Fix, Report, finding_id, fix_group_id,
};
use async_doctor_rules::{RULE_BLOCKING_SLEEP, blocking_sleep_fix_hint};
use ignore::WalkBuilder;
use ruff_db::files::system_path_to_file;
use ruff_db::parsed::parsed_module;
use ruff_db::source::{line_index, source_text};
use ruff_db::system::{SystemPath, SystemPathBuf};
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_source_file::OneIndexed;
use ruff_text_size::{Ranged, TextRange, TextSize};

use crate::db::AsyncDoctorDb;

/// Scan a project directory or a single `.py` file.
pub fn scan_path(path: &Path) -> anyhow::Result<Report> {
    let path = absolute_utf8(path)?;
    if path.as_std_path().is_file() {
        if path.extension() != Some("py") {
            anyhow::bail!("`{path}` is not a Python (`.py`) file");
        }
        let parent = path
            .parent()
            .map(SystemPath::to_path_buf)
            .unwrap_or_else(|| SystemPathBuf::from("."));
        let db = AsyncDoctorDb::open(&parent, None)?;
        return scan_files(&db, vec![path.as_std_path().to_path_buf()]);
    }

    let db = AsyncDoctorDb::open(&path, None)?;
    let files = discover_python_files(db.project_root().as_std_path())?;
    scan_files(&db, files)
}

fn absolute_utf8(path: &Path) -> anyhow::Result<SystemPathBuf> {
    let path = std::path::absolute(path)
        .with_context(|| format!("cannot resolve path `{}`", path.display()))?;
    SystemPathBuf::from_path_buf(path)
        .map_err(|p| anyhow::anyhow!("`{}` is not valid UTF-8", p.display()))
}

fn scan_files(db: &AsyncDoctorDb, files: Vec<PathBuf>) -> anyhow::Result<Report> {
    let mut report = Report::empty_complete();
    let program = db.program();

    for path in files {
        let system_path = match SystemPathBuf::from_path_buf(path.clone()) {
            Ok(p) => p,
            Err(_) => {
                report.complete = false;
                report
                    .skipped
                    .push(format!("{}: path is not valid UTF-8", path.display()));
                continue;
            }
        };

        let file = match system_path_to_file(db, &system_path) {
            Ok(f) => f,
            Err(err) => {
                report.complete = false;
                report
                    .skipped
                    .push(format!("{}: {err}", system_path.as_str()));
                continue;
            }
        };

        let program_file = program.program_file(db, file);
        let parsed = parsed_module(db, program_file.python_file(db)).load(db);
        let rel = relative_display(db.project_root(), &system_path);

        if parsed.has_invalid_syntax() {
            report.complete = false;
            let detail = parsed
                .errors()
                .first()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "syntax error".to_string());
            report.skipped.push(format!("{rel}: {detail}"));
            continue;
        }

        let source = source_text(db, file);
        let index = line_index(db, file);
        let bindings = collect_bindings(parsed.suite());
        let mut findings = Vec::new();
        let mut visitor = SleepVisitor {
            source: source.as_str(),
            index: &index,
            file: &rel,
            bindings: &bindings,
            findings: &mut findings,
            in_async_def: false,
            async_entry: None,
            class_stack: Vec::new(),
            dead_depth: 0,
        };
        for stmt in parsed.suite() {
            visitor.visit_stmt(stmt);
        }
        report.findings.extend(findings);
    }

    report.findings.sort_by(|a, b| {
        (
            a.entry.file.as_str(),
            a.entry.line,
            a.chain.first().map(|c| c.line).unwrap_or(0),
            a.id.as_str(),
        )
            .cmp(&(
                b.entry.file.as_str(),
                b.entry.line,
                b.chain.first().map(|c| c.line).unwrap_or(0),
                b.id.as_str(),
            ))
    });

    Ok(report)
}

fn discover_python_files(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build();

    for entry in walker {
        let entry = entry.context("directory walk failed")?;
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("py") {
            continue;
        }
        if path.components().any(|c| {
            matches!(
                c.as_os_str().to_str(),
                Some(".venv" | "venv" | "site-packages" | "__pycache__" | ".git" | "target")
            )
        }) {
            continue;
        }
        paths.push(path.to_path_buf());
    }
    paths.sort();
    Ok(paths)
}

fn relative_display(root: &SystemPath, path: &SystemPath) -> String {
    path.strip_prefix(root)
        .map(|p| p.as_str().replace('\\', "/"))
        .unwrap_or_else(|_| path.as_str().replace('\\', "/"))
}

#[derive(Debug, Default)]
struct Bindings {
    /// Local names bound to the `time` module (`import time [as x]`).
    time_modules: HashMap<String, ()>,
    /// Local names bound to `time.sleep` (`from time import sleep [as x]`).
    sleep_funcs: HashMap<String, ()>,
    /// Local names bound to the `typing` module (`import typing [as x]`).
    typing_modules: HashMap<String, ()>,
    /// Local names bound to `typing.TYPE_CHECKING`.
    type_checking_names: HashMap<String, ()>,
}

fn collect_bindings(suite: &[Stmt]) -> Bindings {
    let mut bindings = Bindings::default();
    for stmt in suite {
        match stmt {
            Stmt::Import(import) => {
                for alias in &import.names {
                    let local = alias
                        .asname
                        .as_ref()
                        .map(|a| a.as_str())
                        .unwrap_or_else(|| alias.name.as_str());
                    match alias.name.as_str() {
                        "time" => {
                            bindings.time_modules.insert(local.to_string(), ());
                        }
                        "typing" => {
                            bindings.typing_modules.insert(local.to_string(), ());
                        }
                        _ => {}
                    }
                }
            }
            Stmt::ImportFrom(from) => {
                if from.level != 0 {
                    continue;
                }
                let Some(module) = from.module.as_ref() else {
                    continue;
                };
                match module.as_str() {
                    "time" => {
                        for alias in &from.names {
                            if alias.name.as_str() == "sleep" {
                                let local =
                                    alias.asname.as_ref().map(|a| a.as_str()).unwrap_or("sleep");
                                bindings.sleep_funcs.insert(local.to_string(), ());
                            }
                        }
                    }
                    "typing" => {
                        for alias in &from.names {
                            if alias.name.as_str() == "TYPE_CHECKING" {
                                let local = alias
                                    .asname
                                    .as_ref()
                                    .map(|a| a.as_str())
                                    .unwrap_or("TYPE_CHECKING");
                                bindings.type_checking_names.insert(local.to_string(), ());
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    bindings
}

struct SleepVisitor<'a> {
    source: &'a str,
    index: &'a ruff_source_file::LineIndex,
    file: &'a str,
    bindings: &'a Bindings,
    findings: &'a mut Vec<Finding>,
    in_async_def: bool,
    /// `(qualname, def_line)` for the enclosing async function.
    async_entry: Option<(String, u32)>,
    class_stack: Vec<String>,
    dead_depth: u32,
}

impl<'a> Visitor<'a> for SleepVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(func) => {
                let was_async = self.in_async_def;
                let prev_entry = self.async_entry.clone();
                if func.is_async {
                    self.in_async_def = true;
                    let line = self.line_of(func.name.range.start());
                    let qualname = self.qualify(func.name.as_str());
                    self.async_entry = Some((qualname, line));
                    for s in &func.body {
                        self.visit_stmt(s);
                    }
                } else {
                    // Sync def: do not report sleeps inside, even if nested in async.
                    self.in_async_def = false;
                    self.async_entry = None;
                    for s in &func.body {
                        self.visit_stmt(s);
                    }
                }
                self.in_async_def = was_async;
                self.async_entry = prev_entry;
            }
            Stmt::If(if_stmt) => {
                self.visit_if(if_stmt);
            }
            Stmt::ClassDef(class) => {
                self.class_stack.push(class.name.as_str().to_string());
                for s in &class.body {
                    self.visit_stmt(s);
                }
                self.class_stack.pop();
            }
            other => walk_stmt(self, other),
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            // Deferred: lambda bodies are not executed on the event loop until called.
            // Do not report direct findings inside them. Comprehensions still run inline.
            Expr::Lambda(lambda) => {
                if let Some(parameters) = &lambda.parameters {
                    self.visit_parameters(parameters);
                }
            }
            Expr::Call(call) => {
                if self.in_async_def && self.dead_depth == 0 && self.is_time_sleep(&call.func) {
                    self.report_sleep(call.range());
                }
                walk_expr(self, expr);
            }
            _ => walk_expr(self, expr),
        }
    }
}

impl<'a> SleepVisitor<'a> {
    fn qualify(&self, name: &str) -> String {
        if self.class_stack.is_empty() {
            name.to_string()
        } else {
            format!("{}.{}", self.class_stack.join("."), name)
        }
    }

    fn visit_if(&mut self, if_stmt: &'a ast::StmtIf) {
        let dead = self.is_dead_branch_test(&if_stmt.test);
        if dead {
            self.dead_depth += 1;
        } else {
            self.visit_expr(&if_stmt.test);
        }
        for s in &if_stmt.body {
            self.visit_stmt(s);
        }
        if dead {
            self.dead_depth -= 1;
        }

        for clause in &if_stmt.elif_else_clauses {
            let elif_dead = clause
                .test
                .as_ref()
                .is_some_and(|t| self.is_dead_branch_test(t));
            if elif_dead {
                self.dead_depth += 1;
            } else if let Some(test) = &clause.test {
                self.visit_expr(test);
            }
            for s in &clause.body {
                self.visit_stmt(s);
            }
            if elif_dead {
                self.dead_depth -= 1;
            }
        }
    }

    fn is_time_sleep(&self, func: &Expr) -> bool {
        match func {
            Expr::Attribute(attr) => {
                attr.attr.as_str() == "sleep"
                    && matches!(
                        attr.value.as_ref(),
                        Expr::Name(name)
                            if self.bindings.time_modules.contains_key(name.id.as_str())
                    )
            }
            Expr::Name(name) => self.bindings.sleep_funcs.contains_key(name.id.as_str()),
            _ => false,
        }
    }

    fn is_dead_branch_test(&self, test: &Expr) -> bool {
        match test {
            Expr::BooleanLiteral(b) => !b.value,
            Expr::Name(name) => self
                .bindings
                .type_checking_names
                .contains_key(name.id.as_str()),
            Expr::Attribute(attr) => {
                attr.attr.as_str() == "TYPE_CHECKING"
                    && matches!(
                        attr.value.as_ref(),
                        Expr::Name(n)
                            if self.bindings.typing_modules.contains_key(n.id.as_str())
                    )
            }
            _ => false,
        }
    }

    fn report_sleep(&mut self, range: TextRange) {
        let Some((ref qualname, entry_line)) = self.async_entry else {
            return;
        };
        let call_line = self.line_of(range.start());
        let call_column = self.column_of(range.start());
        let text = snippet(self.source, range);
        let sink = "time.sleep";
        let id = finding_id(FindingIdParts {
            rule: RULE_BLOCKING_SLEEP,
            entry_file: self.file,
            entry_qualname: qualname,
            sink,
            call_file: self.file,
            call_line,
            call_column,
        });
        let group = fix_group_id(RULE_BLOCKING_SLEEP, sink);
        self.findings.push(Finding {
            id,
            rule: RULE_BLOCKING_SLEEP.to_string(),
            entry: Entry {
                file: self.file.to_string(),
                line: entry_line,
                name: qualname.clone(),
                label: EntryLabel::Other,
            },
            chain: vec![ChainStep {
                file: self.file.to_string(),
                line: call_line,
                text,
            }],
            conditions: vec!["unconditional".to_string()],
            fix: Fix {
                hint: blocking_sleep_fix_hint().to_string(),
                fix_group_id: group,
            },
            why: format!("async-doctor why {}:{}", self.file, call_line),
        });
    }

    fn line_of(&self, offset: TextSize) -> u32 {
        let line: OneIndexed = self.index.line_index(offset);
        line.get() as u32
    }

    fn column_of(&self, offset: TextSize) -> u32 {
        self.index.line_column(offset, self.source).column.get() as u32
    }
}

fn snippet(source: &str, range: TextRange) -> String {
    let start = usize::from(range.start());
    let end = usize::from(range.end()).min(source.len());
    let start = start.min(end);
    source[start..end].trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_project(files: &[(&str, &str)]) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("async-doctor-m0-{n}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (name, content) in files {
            let path = dir.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, content).unwrap();
        }
        dir
    }

    #[test]
    fn reports_direct_time_sleep_in_async_def() {
        let dir = temp_project(&[(
            "pos.py",
            "import time\n\nasync def handler():\n    time.sleep(1)\n",
        )]);
        let report = scan_path(&dir).unwrap();
        assert!(report.complete);
        assert_eq!(report.findings.len(), 1);
        let f = &report.findings[0];
        assert_eq!(f.rule, "blocking-sleep");
        assert_eq!(f.entry.name, "handler");
        assert_eq!(f.entry.label, EntryLabel::Other);
        assert_eq!(f.chain[0].text, "time.sleep(1)");
        assert_eq!(report.exit_code(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ignores_sleep_in_sync_def() {
        let dir = temp_project(&[(
            "neg_sync.py",
            "import time\n\ndef sync_helper():\n    time.sleep(1)\n",
        )]);
        let report = scan_path(&dir).unwrap();
        assert!(report.findings.is_empty());
        assert_eq!(report.exit_code(), 0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ignores_sleep_under_type_checking() {
        let dir = temp_project(&[(
            "neg_tc.py",
            "import time\nfrom typing import TYPE_CHECKING\n\nasync def handler():\n    if TYPE_CHECKING:\n        time.sleep(1)\n",
        )]);
        let report = scan_path(&dir).unwrap();
        assert!(
            report.findings.is_empty(),
            "unexpected findings: {:?}",
            report.findings
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ignores_sleep_under_typing_module_type_checking() {
        let dir = temp_project(&[(
            "neg_typing.py",
            "import time\nimport typing\n\nasync def handler():\n    if typing.TYPE_CHECKING:\n        time.sleep(1)\n",
        )]);
        let report = scan_path(&dir).unwrap();
        assert!(report.findings.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn does_not_treat_arbitrary_t_as_typing() {
        // `t` is not the typing module unless imported as such.
        let dir = temp_project(&[(
            "not_typing.py",
            "import time\n\nclass t:\n    TYPE_CHECKING = False\n\nasync def handler():\n    if t.TYPE_CHECKING:\n        time.sleep(1)\n",
        )]);
        let report = scan_path(&dir).unwrap();
        assert_eq!(
            report.findings.len(),
            1,
            "t.TYPE_CHECKING without importing typing should not be treated as dead"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn respects_typing_module_alias() {
        let dir = temp_project(&[(
            "alias_typing.py",
            "import time\nimport typing as t\n\nasync def handler():\n    if t.TYPE_CHECKING:\n        time.sleep(1)\n",
        )]);
        let report = scan_path(&dir).unwrap();
        assert!(
            report.findings.is_empty(),
            "import typing as t should make t.TYPE_CHECKING a dead branch: {:?}",
            report.findings
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn reports_from_time_import_sleep() {
        let dir = temp_project(&[(
            "from_imp.py",
            "from time import sleep\n\nasync def go():\n    sleep(0.1)\n",
        )]);
        let report = scan_path(&dir).unwrap();
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].chain[0].text, "sleep(0.1)");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ignores_sleep_inside_lambda_body() {
        let dir = temp_project(&[(
            "lambda_offload.py",
            "import asyncio\nimport time\n\nasync def handler():\n    await asyncio.to_thread(lambda: time.sleep(1))\n",
        )]);
        let report = scan_path(&dir).unwrap();
        assert!(
            report.findings.is_empty(),
            "lambda bodies must not produce direct findings: {:?}",
            report.findings
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn still_reports_sleep_inside_comprehension() {
        let dir = temp_project(&[(
            "comp.py",
            "import time\n\nasync def handler():\n    _ = [time.sleep(i) for i in (1,)]\n",
        )]);
        let report = scan_path(&dir).unwrap();
        assert_eq!(report.findings.len(), 1);
        assert!(report.findings[0].chain[0].text.contains("time.sleep"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn finding_ids_differ_for_same_method_name_on_different_classes() {
        let dir = temp_project(&[(
            "classes.py",
            "import time\n\nclass A:\n    async def handle(self):\n        time.sleep(1)\n\nclass B:\n    async def handle(self):\n        time.sleep(1)\n",
        )]);
        let report = scan_path(&dir).unwrap();
        assert_eq!(report.findings.len(), 2);
        assert_eq!(report.findings[0].entry.name, "A.handle");
        assert_eq!(report.findings[1].entry.name, "B.handle");
        assert_ne!(report.findings[0].id, report.findings[1].id);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn finding_ids_differ_for_multiple_sleeps_in_one_function() {
        let dir = temp_project(&[(
            "multi.py",
            "import time\n\nasync def handler():\n    time.sleep(1)\n    time.sleep(2)\n",
        )]);
        let report = scan_path(&dir).unwrap();
        assert_eq!(report.findings.len(), 2);
        assert_ne!(report.findings[0].id, report.findings[1].id);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn syntax_error_marks_incomplete_and_skipped() {
        let dir = temp_project(&[(
            "broken.py",
            "import time\n\nasync def handler(:\n    time.sleep(1)\n",
        )]);
        let report = scan_path(&dir).unwrap();
        assert!(!report.complete);
        assert!(
            report.skipped.iter().any(|s| s.contains("broken.py")),
            "skipped={:?}",
            report.skipped
        );
        // No reliable findings from a broken file.
        assert!(report.findings.is_empty());
        assert_eq!(report.exit_code(), 0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn accepts_single_py_file_path() {
        let dir = temp_project(&[(
            "only.py",
            "import time\n\nasync def handler():\n    time.sleep(1)\n",
        )]);
        let file = dir.join("only.py");
        let report = scan_path(&file).unwrap();
        assert!(report.complete);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].entry.file, "only.py");
        let _ = fs::remove_dir_all(dir);
    }
}
