//! In-memory buffer / archive receivers.

use ruff_python_ast::{self as ast, Expr};
use ty_python_semantic::HasType;
use ty_python_semantic::types::{Type, TypeDefinition};

use super::CallCollector;
use crate::extract::helpers::{
    NarrowFact, is_archive_ctor_qual, is_disk_touching_archive_method, is_in_memory_buffer_type,
    is_pure_in_memory_archive_method, module_of_definition,
};

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(crate) fn first_arg_is_in_memory_buffer(&mut self, call: &ast::ExprCall) -> bool {
        if let Some(arg) = call.arguments.args.first()
            && self.expr_is_in_memory_buffer(arg)
        {
            return true;
        }
        // PIL fp=/file=; tarfile fileobj=; zipfile file=.
        let kw = call.arguments.keywords.iter().find(|k| {
            k.arg
                .as_ref()
                .is_some_and(|a| matches!(a.as_str(), "fp" | "file" | "filename" | "fileobj"))
        });
        kw.is_some_and(|k| self.expr_is_in_memory_buffer(&k.value))
    }

    pub(crate) fn expr_is_in_memory_buffer(&mut self, expr: &Expr) -> bool {
        if let Expr::Name(n) = expr {
            if self.buffer_locals.contains(n.id.as_str()) {
                return true;
            }
            if let Some(NarrowFact::IsInstance(ty)) = self.narrowed_locals.get(n.id.as_str()) {
                return is_in_memory_buffer_type(ty);
            }
        }
        if let Expr::Call(c) = expr {
            // BytesIO(...) / StringIO(...) constructor call.
            if let Some(q) = self.external_qualname(&c.func)
                && is_in_memory_buffer_type(&q)
            {
                return true;
            }
            if let Expr::Attribute(a) = c.func.as_ref()
                && matches!(a.attr.as_str(), "BytesIO" | "StringIO")
            {
                return true;
            }
            if let Expr::Name(n) = c.func.as_ref()
                && matches!(n.id.as_str(), "BytesIO" | "StringIO")
            {
                return true;
            }
        }
        self.expr_type_is_all_in_memory_buffers(expr)
    }

    fn expr_type_is_all_in_memory_buffers(&self, expr: &Expr) -> bool {
        let Some(ty) = expr.inferred_type(self.model) else {
            return false;
        };
        let drop_none = matches!(
            expr,
            Expr::Name(n)
                if matches!(
                    self.narrowed_locals.get(n.id.as_str()),
                    Some(NarrowFact::NotNone)
                )
        );
        let env = self.model.program_environment();
        self.ty_is_in_memory_buffer(ty, &env, drop_none)
    }

    fn ty_is_in_memory_buffer(
        &self,
        ty: Type<'db>,
        env: &ty_python_semantic::ProgramEnvironment<'db>,
        drop_none: bool,
    ) -> bool {
        if matches!(
            ty,
            Type::ClassLiteral(_) | Type::SubclassOf(_) | Type::GenericAlias(_)
        ) {
            return false;
        }
        if let Some(union) = ty.as_union() {
            let mut saw_arm = false;
            for elem in union.elements(self.model.db()).iter() {
                if matches!(elem, Type::Never) {
                    continue;
                }
                if drop_none && elem.is_none(self.model.db()) {
                    continue;
                }
                saw_arm = true;
                if !self.ty_is_in_memory_buffer(*elem, env, false) {
                    return false;
                }
            }
            return saw_arm;
        }
        let Some(TypeDefinition::StaticClass(def) | TypeDefinition::DynamicClass(def)) =
            ty.definition(self.model.db(), env)
        else {
            return false;
        };
        let Some(name) = def.name(self.model.db()) else {
            return false;
        };
        let Some(module) = module_of_definition(self.model, def) else {
            return false;
        };
        let qual = format!("{module}.{name}");
        is_in_memory_buffer_type(&qual)
    }

    pub(crate) fn call_is_archive_ctor(&mut self, func: &Expr) -> bool {
        let ext_qual = self.external_qualname(func);
        self.call_is_archive_ctor_with_qual(func, ext_qual.as_deref())
    }

    pub(crate) fn call_is_archive_ctor_with_qual(
        &self,
        func: &Expr,
        ext_qual: Option<&str>,
    ) -> bool {
        if let Some(q) = ext_qual
            && is_archive_ctor_qual(q)
        {
            return true;
        }
        match func {
            Expr::Attribute(a) => {
                matches!(a.attr.as_str(), "ZipFile" | "TarFile")
                    || (a.attr.as_str() == "open"
                        && matches!(
                            a.value.as_ref(),
                            Expr::Name(n) if matches!(n.id.as_str(), "tarfile" | "zipfile")
                        ))
            }
            Expr::Name(n) => matches!(n.id.as_str(), "ZipFile" | "TarFile"),
            _ => false,
        }
    }

    /// True when this call is a buffer-backed archive ctor or a *pure* in-memory
    /// method on such a receiver. Disk-touching methods (`write`, `extract`,
    /// `extractall`, `add`, …) still report even on BytesIO receivers.
    pub(crate) fn call_is_in_memory_archive(
        &mut self,
        call: &ast::ExprCall,
        func_ext_qual: Option<&str>,
    ) -> bool {
        if self.call_is_archive_ctor_with_qual(&call.func, func_ext_qual)
            && self.first_arg_is_in_memory_buffer(call)
        {
            return true;
        }
        let Expr::Attribute(attr) = call.func.as_ref() else {
            return false;
        };
        let method = attr.attr.as_str();
        // Never suppress disk-touching ops on buffer-backed archives.
        if is_disk_touching_archive_method(method) {
            return false;
        }
        let recv_is_in_memory_archive = match attr.value.as_ref() {
            Expr::Name(n) => self.in_memory_archive_receivers.contains(n.id.as_str()),
            Expr::Call(inner) => {
                self.call_is_archive_ctor(&inner.func) && self.first_arg_is_in_memory_buffer(inner)
            }
            _ => false,
        };
        if !recv_is_in_memory_archive {
            return false;
        }
        if method == "addfile" {
            return self.tarfile_addfile_fileobj_is_in_memory(call);
        }
        is_pure_in_memory_archive_method(method)
            || func_ext_qual.is_some_and(|q| {
                crate::extract::helpers::is_archive_method_qual(q)
                    && q.rsplit_once('.')
                        .is_some_and(|(_, m)| is_pure_in_memory_archive_method(m))
            })
    }

    /// On an already-proven in-memory `TarFile`, `addfile(tarinfo, fileobj=None)`
    /// is non-blocking iff `fileobj` is omitted, `None`, or an in-memory buffer.
    fn tarfile_addfile_fileobj_is_in_memory(&mut self, call: &ast::ExprCall) -> bool {
        if call.arguments.keywords.iter().any(|kw| kw.arg.is_none()) {
            return false;
        }
        let fileobj = call.arguments.args.get(1).or_else(|| {
            call.arguments
                .keywords
                .iter()
                .find(|kw| kw.arg.as_ref().is_some_and(|a| a.as_str() == "fileobj"))
                .map(|kw| &kw.value)
        });
        match fileobj {
            None | Some(Expr::NoneLiteral(_)) => true,
            Some(expr) => self.expr_is_in_memory_buffer(expr),
        }
    }

    pub(super) fn note_buffer_binding(&mut self, name: &str, value: &Expr) {
        if self.expr_is_in_memory_buffer(value) {
            self.buffer_locals.insert(name.to_string());
        } else {
            self.buffer_locals.remove(name);
        }
    }

    pub(super) fn note_archive_binding(&mut self, name: &str, value: &Expr) {
        match value {
            Expr::Call(c)
                if self.call_is_archive_ctor(&c.func) && self.first_arg_is_in_memory_buffer(c) =>
            {
                self.in_memory_archive_receivers.insert(name.to_string());
            }
            Expr::Name(n) if self.in_memory_archive_receivers.contains(n.id.as_str()) => {
                self.in_memory_archive_receivers.insert(name.to_string());
            }
            _ => {
                self.in_memory_archive_receivers.remove(name);
            }
        }
    }
}
