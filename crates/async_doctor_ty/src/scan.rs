//! M0 scanner: report direct `time.sleep(...)` calls inside `async def`.
//!
//! Uses the ty Db + ruff AST (via `parsed_module`). Skips sync `def` bodies and
//! dead branches under `if TYPE_CHECKING` / `if False` / `if typing.TYPE_CHECKING`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use async_doctor_core::{
    ChainStep, Entry, EntryLabel, Finding, Fix, Report, finding_id, fix_group_id,
};
use async_doctor_rules::{RULE_BLOCKING_SLEEP, blocking_sleep_fix_hint};
use ignore::WalkBuilder;
use ruff_db::files::system_path_to_file;
use ruff_db::parsed::parsed_module;
use ruff_db::source::{line_index, source_text};
use ruff_db::system::{SystemPath, SystemPathBuf};
use ruff_python_ast::visitor::{Visitor, walk_stmt};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_source_file::OneIndexed;
use ruff_text_size::{Ranged, TextRange, TextSize};

use crate::db::AsyncDoctorDb;

/// Scan `root` for M0 findings.
pub fn scan_path(root: &Path) -> anyhow::Result<Report> {
    let root = absolute_utf8(root)?;
    let db = AsyncDoctorDb::new(&root, None)?;
    scan_db(&db)
}

fn absolute_utf8(path: &Path) -> anyhow::Result<SystemPathBuf> {
    let path = std::path::absolute(path)
        .with_context(|| format!("cannot resolve path `{}`", path.display()))?;
    SystemPathBuf::from_path_buf(path)
        .map_err(|p| anyhow::anyhow!("`{}` is not valid UTF-8", p.display()))
}

fn scan_db(db: &AsyncDoctorDb) -> anyhow::Result<Report> {
    let mut report = Report::empty_complete();
    let files = discover_python_files(db.root().as_std_path())?;
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
        let source = source_text(db, file);
        let index = line_index(db, file);
        let rel = relative_display(db.root(), &system_path);

        let bindings = collect_time_sleep_bindings(parsed.suite());
        let mut findings = Vec::new();
        let mut visitor = SleepVisitor {
            source: source.as_str(),
            index: &index,
            file: &rel,
            bindings: &bindings,
            findings: &mut findings,
            in_async_def: false,
            async_entry: None,
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
        )
            .cmp(&(
                b.entry.file.as_str(),
                b.entry.line,
                b.chain.first().map(|c| c.line).unwrap_or(0),
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
        // Skip common virtualenv / build trees even if not gitignored.
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

/// Maps local names that refer to `time.sleep` (or modules that expose `.sleep` as time).
#[derive(Debug, Default)]
struct SleepBindings {
    /// Local name → module alias for `import time [as alias]` (call `alias.sleep`).
    time_modules: HashMap<String, ()>,
    /// Local name bound directly to `time.sleep` via `from time import sleep [as x]`.
    sleep_funcs: HashMap<String, ()>,
}

fn collect_time_sleep_bindings(suite: &[Stmt]) -> SleepBindings {
    let mut bindings = SleepBindings::default();
    for stmt in suite {
        match stmt {
            Stmt::Import(import) => {
                for alias in &import.names {
                    if alias.name.as_str() == "time" {
                        let local = alias.asname.as_ref().map(|a| a.as_str()).unwrap_or("time");
                        bindings.time_modules.insert(local.to_string(), ());
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
                if module.as_str() != "time" {
                    continue;
                }
                for alias in &from.names {
                    if alias.name.as_str() == "sleep" {
                        let local = alias.asname.as_ref().map(|a| a.as_str()).unwrap_or("sleep");
                        bindings.sleep_funcs.insert(local.to_string(), ());
                    }
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
    bindings: &'a SleepBindings,
    findings: &'a mut Vec<Finding>,
    in_async_def: bool,
    async_entry: Option<(String, u32)>,
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
                    self.async_entry = Some((func.name.as_str().to_string(), line));
                    for s in &func.body {
                        self.visit_stmt(s);
                    }
                } else {
                    // Sync def: do not report sleeps inside, even if nested in async.
                    self.in_async_def = false;
                    self.async_entry = None;
                    // Still walk nested async defs.
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
                for s in &class.body {
                    self.visit_stmt(s);
                }
            }
            other => {
                if self.in_async_def && self.dead_depth == 0 {
                    walk_stmt(self, other);
                } else if !self.in_async_def {
                    // Still need to discover nested async defs in module/class bodies.
                    walk_stmt(self, other);
                } else {
                    // Dead branch inside async: skip call reporting but still allow nested defs.
                    walk_stmt(self, other);
                }
            }
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if self.in_async_def
            && self.dead_depth == 0
            && let Expr::Call(call) = expr
            && self.is_time_sleep(&call.func)
        {
            self.report_sleep(call.range());
        }
        ruff_python_ast::visitor::walk_expr(self, expr);
    }
}

impl<'a> SleepVisitor<'a> {
    fn visit_if(&mut self, if_stmt: &'a ast::StmtIf) {
        let dead = is_dead_branch_test(&if_stmt.test);
        if dead {
            self.dead_depth += 1;
        } else {
            // Condition itself may contain calls; only report if live.
            self.visit_expr(&if_stmt.test);
        }
        for s in &if_stmt.body {
            self.visit_stmt(s);
        }
        if dead {
            self.dead_depth -= 1;
        }

        for clause in &if_stmt.elif_else_clauses {
            let elif_dead = clause.test.as_ref().is_some_and(is_dead_branch_test);
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
                        Expr::Name(name) if self.bindings.time_modules.contains_key(name.id.as_str())
                    )
            }
            Expr::Name(name) => self.bindings.sleep_funcs.contains_key(name.id.as_str()),
            _ => false,
        }
    }

    fn report_sleep(&mut self, range: TextRange) {
        let Some((ref name, entry_line)) = self.async_entry else {
            return;
        };
        let call_line = self.line_of(range.start());
        let text = snippet(self.source, range);
        let sink = "time.sleep";
        let id = finding_id(RULE_BLOCKING_SLEEP, self.file, name, sink);
        let group = fix_group_id(RULE_BLOCKING_SLEEP, sink);
        self.findings.push(Finding {
            id,
            rule: RULE_BLOCKING_SLEEP.to_string(),
            entry: Entry {
                file: self.file.to_string(),
                line: entry_line,
                name: name.clone(),
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
}

fn snippet(source: &str, range: TextRange) -> String {
    let start = usize::from(range.start());
    let end = usize::from(range.end()).min(source.len());
    let start = start.min(end);
    source[start..end].trim().to_string()
}

fn is_dead_branch_test(test: &Expr) -> bool {
    match test {
        Expr::BooleanLiteral(b) => !b.value,
        Expr::Name(name) => name.id.as_str() == "TYPE_CHECKING",
        Expr::Attribute(attr) => {
            attr.attr.as_str() == "TYPE_CHECKING"
                && matches!(
                    attr.value.as_ref(),
                    Expr::Name(n) if matches!(n.id.as_str(), "typing" | "t")
                )
        }
        _ => false,
    }
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
}
