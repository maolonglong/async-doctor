//! Third-party follow-target recording for wave-based site-packages extraction.

use std::path::{Path, PathBuf};

use ruff_db::files::{FilePath, system_path_to_file};
use ruff_db::parsed::parsed_module;
use ruff_db::system::SystemPathBuf;
use ruff_python_ast::{self as ast, Expr, Stmt};
use ty_python_core::definition::DefinitionKind;
use ty_python_semantic::types::TypeDefinition;
use ty_python_semantic::{
    HasType, ImportAliasResolution, definitions_for_attribute, definitions_for_name,
};

use super::CallCollector;
use crate::extract::ctx::scope_qualname;
use crate::extract::helpers::{module_of_definition, qualname_from_definition};
use crate::sitepkgs::{FollowTarget, is_followable_site_packages_py};

fn is_under_followable_extra(path: &str, roots: &[PathBuf]) -> bool {
    let p = Path::new(path);
    roots.iter().any(|r| p.starts_with(r))
}

fn find_fn_name_start_in_stmts(
    stmts: &[Stmt],
    target_local_qual: &str,
    class_stack: &mut Vec<String>,
    fn_stack: &mut Vec<String>,
) -> Option<u32> {
    let mut found = None;
    for stmt in stmts {
        match stmt {
            Stmt::FunctionDef(f) => {
                let q = scope_qualname(class_stack, fn_stack, f.name.as_str());
                if q == target_local_qual {
                    found = Some(u32::from(f.name.range.start()));
                }
                fn_stack.push(f.name.as_str().to_string());
                if let Some(inner) =
                    find_fn_name_start_in_stmts(&f.body, target_local_qual, class_stack, fn_stack)
                {
                    found = Some(inner);
                }
                fn_stack.pop();
            }
            Stmt::ClassDef(c) => {
                class_stack.push(c.name.as_str().to_string());
                if let Some(inner) =
                    find_fn_name_start_in_stmts(&c.body, target_local_qual, class_stack, fn_stack)
                {
                    found = Some(inner);
                }
                class_stack.pop();
            }
            Stmt::If(s) => {
                if let Some(inner) =
                    find_fn_name_start_in_stmts(&s.body, target_local_qual, class_stack, fn_stack)
                {
                    found = Some(inner);
                }
                for clause in &s.elif_else_clauses {
                    if let Some(inner) = find_fn_name_start_in_stmts(
                        &clause.body,
                        target_local_qual,
                        class_stack,
                        fn_stack,
                    ) {
                        found = Some(inner);
                    }
                }
            }
            Stmt::Try(s) => {
                for suite in [&s.body, &s.orelse, &s.finalbody] {
                    if let Some(inner) =
                        find_fn_name_start_in_stmts(suite, target_local_qual, class_stack, fn_stack)
                    {
                        found = Some(inner);
                    }
                }
                for h in &s.handlers {
                    let ast::ExceptHandler::ExceptHandler(h) = h;
                    if let Some(inner) = find_fn_name_start_in_stmts(
                        &h.body,
                        target_local_qual,
                        class_stack,
                        fn_stack,
                    ) {
                        found = Some(inner);
                    }
                }
            }
            Stmt::With(s) => {
                if let Some(inner) =
                    find_fn_name_start_in_stmts(&s.body, target_local_qual, class_stack, fn_stack)
                {
                    found = Some(inner);
                }
            }
            _ => {}
        }
    }
    found
}

fn find_relative_reexport_py(
    suite: &[Stmt],
    py_path: &SystemPathBuf,
    symbol: &str,
) -> Option<(SystemPathBuf, String)> {
    let base_dir = py_path.parent()?;
    for stmt in suite {
        let Stmt::ImportFrom(imp) = stmt else {
            continue;
        };
        if imp.level == 0 {
            continue;
        }
        for alias in &imp.names {
            let exported = alias
                .asname
                .as_ref()
                .map(|a| a.as_str())
                .unwrap_or(alias.name.as_str());
            if exported != symbol {
                continue;
            }
            let mut dir = base_dir.to_path_buf();
            for _ in 1..imp.level {
                dir = dir.parent()?.to_path_buf();
            }
            let rel_mod = imp.module.as_ref().map(|m| m.as_str()).unwrap_or("");
            let target_base = if rel_mod.is_empty() {
                dir.join(alias.name.as_str())
            } else {
                let mut p = dir;
                for seg in rel_mod.split('.') {
                    p = p.join(seg);
                }
                p
            };
            let candidate_py = target_base.with_extension("py");
            if Path::new(candidate_py.as_str()).is_file() {
                return Some((candidate_py, alias.name.as_str().to_string()));
            }
            let candidate_init = target_base.join("__init__.py");
            if Path::new(candidate_init.as_str()).is_file() {
                return Some((candidate_init, alias.name.as_str().to_string()));
            }
        }
    }
    None
}

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(crate) fn note_follow_target(&mut self, func: &Expr, qualname: &str) {
        if self.follow_targets.contains_key(qualname) {
            return;
        }
        let resolved = match func {
            Expr::Name(name) => definitions_for_name(
                self.model,
                name.id.as_str(),
                name.into(),
                ImportAliasResolution::ResolveAliases,
            ),
            Expr::Attribute(attr) => definitions_for_attribute(self.model, attr),
            _ => Vec::new(),
        };
        let mut defs: Vec<ty_python_core::definition::Definition<'db>> =
            resolved.iter().filter_map(|r| r.definition()).collect();
        if defs.is_empty()
            && let Some(ty) = func.inferred_type(self.model)
        {
            let env = self.model.program_environment();
            if let Some(TypeDefinition::Function(def)) = ty.definition(self.model.db(), &env) {
                defs.push(def);
            }
        }
        let db = self.model.db();
        for def in defs {
            let file = def.file(db);
            let FilePath::System(sys) = file.path(db) else {
                continue;
            };
            let DefinitionKind::Function(fdef) = def.kind(db) else {
                continue;
            };
            let parsed = parsed_module(db, def.python_file(db)).load(db);
            let node = fdef.node(&parsed);

            let (resolved_path, name_start) = if sys.extension() == Some("pyi") {
                let Some(mapped) = self.map_pyi_def_to_sibling_py(def, sys, node.name.as_str())
                else {
                    continue;
                };
                mapped
            } else {
                if !is_followable_site_packages_py(sys.as_str())
                    && !is_under_followable_extra(sys.as_str(), self.followable_extra_roots)
                {
                    continue;
                }
                (sys.to_path_buf(), u32::from(node.name.range.start()))
            };

            let std_path = PathBuf::from(resolved_path.as_str());
            let (pkg, ver, fp) = self
                .package_meta
                .meta_for_path(&std_path)
                .map(|(n, v, f)| (Some(n), Some(v), Some(f)))
                .unwrap_or((None, None, None));
            self.follow_targets.insert(
                qualname.to_string(),
                FollowTarget {
                    qualname: qualname.to_string(),
                    path: resolved_path,
                    name_start,
                    package_name: pkg,
                    package_version: ver,
                    package_fingerprint: fp,
                },
            );
            return;
        }
    }

    fn map_pyi_def_to_sibling_py(
        &self,
        def: ty_python_core::definition::Definition<'db>,
        pyi_sys: &ruff_db::system::SystemPath,
        fallback_fn_name: &str,
    ) -> Option<(SystemPathBuf, u32)> {
        let py_sys = pyi_sys.with_extension("py");
        if !is_followable_site_packages_py(py_sys.as_str())
            && !is_under_followable_extra(py_sys.as_str(), self.followable_extra_roots)
        {
            return None;
        }
        if !Path::new(py_sys.as_str()).is_file() {
            return None;
        }
        let local_qual = qualname_from_definition(self.model, def)
            .and_then(|full| {
                let module = module_of_definition(self.model, def)?;
                full.strip_prefix(&format!("{module}."))
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_else(|| fallback_fn_name.to_string());

        let db = self.model.db();
        let py_file = system_path_to_file(db, &py_sys).ok()?;
        let program_file = db.program_file(py_file);
        let py_parsed = parsed_module(db, program_file.python_file(db)).load(db);
        if py_parsed.has_invalid_syntax() {
            return None;
        }
        let mut classes = Vec::new();
        let mut fns = Vec::new();
        if let Some(start) =
            find_fn_name_start_in_stmts(py_parsed.suite(), &local_qual, &mut classes, &mut fns)
        {
            return Some((py_sys, start));
        }
        // Handle PEP 561 `__init__.pyi` stubs backed by `from ._impl import fn` in `__init__.py`.
        if !local_qual.contains('.')
            && let Some((reexp_py, orig_name)) =
                find_relative_reexport_py(py_parsed.suite(), &py_sys, &local_qual)
            && (is_followable_site_packages_py(reexp_py.as_str())
                || is_under_followable_extra(reexp_py.as_str(), self.followable_extra_roots))
        {
            let sub_file = system_path_to_file(db, &reexp_py).ok()?;
            let sub_pf = db.program_file(sub_file);
            let sub_parsed = parsed_module(db, sub_pf.python_file(db)).load(db);
            if !sub_parsed.has_invalid_syntax()
                && let Some(start) = find_fn_name_start_in_stmts(
                    sub_parsed.suite(),
                    &orig_name,
                    &mut classes,
                    &mut fns,
                )
            {
                return Some((reexp_py, start));
            }
        }
        None
    }
}
