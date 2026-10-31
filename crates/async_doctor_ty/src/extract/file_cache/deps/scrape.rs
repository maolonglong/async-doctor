//! AST scrape of import heads (including nested bodies).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ruff_python_ast::Stmt;
use ruff_python_parser::parse_module;

use super::roots::{absent_probe_rels, resolve_head_with_precedence};

/// Import module heads scraped via AST (sorted unique).
///
/// - `import a.b` → `a.b`
/// - `from X import Y` → `X`, `X.Y`, bare `Y`
/// - relatives resolved against `file_module`
///
/// Walks nested function/class bodies so function-local imports count.
pub fn scrape_import_heads_ast(source: &str, file_module: &str) -> Vec<String> {
    let Ok(parsed) = parse_module(source) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for stmt in &parsed.syntax().body {
        collect_import_heads(stmt, file_module, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

fn collect_import_heads(stmt: &Stmt, file_module: &str, out: &mut Vec<String>) {
    match stmt {
        Stmt::Import(i) => {
            for alias in &i.names {
                let full = alias.name.as_str();
                if !full.is_empty() {
                    out.push(full.to_string());
                }
            }
        }
        Stmt::ImportFrom(i) => {
            let base = if i.level > 0 {
                resolve_relative_import(file_module, i.level, i.module.as_ref().map(|m| m.as_str()))
            } else {
                i.module.as_ref().map(|m| m.as_str().to_string())
            };
            if let Some(ref b) = base
                && !b.is_empty()
            {
                out.push(b.clone());
            }
            for alias in &i.names {
                let name = alias.name.as_str();
                if name.is_empty() || name == "*" {
                    continue;
                }
                out.push(name.to_string());
                if let Some(ref b) = base {
                    if b.is_empty() {
                        out.push(name.to_string());
                    } else {
                        out.push(format!("{b}.{name}"));
                    }
                }
            }
        }
        Stmt::FunctionDef(f) => {
            for s in &f.body {
                collect_import_heads(s, file_module, out);
            }
        }
        Stmt::ClassDef(c) => {
            for s in &c.body {
                collect_import_heads(s, file_module, out);
            }
        }
        Stmt::If(i) => {
            for s in &i.body {
                collect_import_heads(s, file_module, out);
            }
            for elif in &i.elif_else_clauses {
                for s in &elif.body {
                    collect_import_heads(s, file_module, out);
                }
            }
        }
        Stmt::While(w) => {
            for s in &w.body {
                collect_import_heads(s, file_module, out);
            }
            for s in &w.orelse {
                collect_import_heads(s, file_module, out);
            }
        }
        Stmt::For(f) => {
            for s in &f.body {
                collect_import_heads(s, file_module, out);
            }
            for s in &f.orelse {
                collect_import_heads(s, file_module, out);
            }
        }
        Stmt::With(w) => {
            for s in &w.body {
                collect_import_heads(s, file_module, out);
            }
        }
        Stmt::Match(m) => {
            for case in &m.cases {
                for s in &case.body {
                    collect_import_heads(s, file_module, out);
                }
            }
        }
        Stmt::Try(t) => {
            for s in &t.body {
                collect_import_heads(s, file_module, out);
            }
            for h in &t.handlers {
                let ruff_python_ast::ExceptHandler::ExceptHandler(eh) = h;
                for s in &eh.body {
                    collect_import_heads(s, file_module, out);
                }
            }
            for s in &t.orelse {
                collect_import_heads(s, file_module, out);
            }
            for s in &t.finalbody {
                collect_import_heads(s, file_module, out);
            }
        }
        _ => {}
    }
}

fn resolve_relative_import(file_module: &str, level: u32, module: Option<&str>) -> Option<String> {
    if level == 0 {
        return module.map(str::to_string);
    }
    let mut parts: Vec<&str> = file_module.split('.').filter(|s| !s.is_empty()).collect();
    if !file_module.is_empty() {
        let _ = parts.pop();
    }
    let up = (level as usize).saturating_sub(1);
    if up > parts.len() {
        return None;
    }
    parts.truncate(parts.len() - up);
    if let Some(m) = module.filter(|s| !s.is_empty()) {
        parts.extend(m.split('.').filter(|s| !s.is_empty()));
    }
    if parts.is_empty() {
        Some(String::new())
    } else {
        Some(parts.join("."))
    }
}

/// Resolve scraped heads to first-party rels + absent probes for misses.
///
/// When a head resolves under a later search root, higher-precedence candidate
/// paths that were missing are still recorded as absent probes so a new file
/// that shadows the winner invalidates importers (e.g. root `pkg/m.py` over
/// `src/pkg/m.py`).
pub fn direct_deps_for_source(
    source: &str,
    file_module: &str,
    module_to_rel: &HashMap<String, String>,
    scan_root: &Path,
    search_roots: &[PathBuf],
) -> (Vec<String>, Vec<String>) {
    let mut deps = Vec::new();
    let mut absents = Vec::new();
    let mut seen_dep = HashSet::new();
    let mut seen_absent = HashSet::new();
    for head in scrape_import_heads_ast(source, file_module) {
        let (resolved, precedence_misses) =
            resolve_head_with_precedence(&head, module_to_rel, scan_root, search_roots);
        for probe in precedence_misses {
            if seen_absent.insert(probe.clone()) {
                absents.push(probe);
            }
        }
        if let Some(rel) = resolved {
            if seen_dep.insert(rel.clone()) {
                deps.push(rel);
            }
            continue;
        }
        for probe in absent_probe_rels(&head, scan_root, search_roots) {
            if seen_absent.insert(probe.clone()) {
                absents.push(probe);
            }
        }
    }
    deps.sort();
    absents.sort();
    (deps, absents)
}
