//! In-memory buffer / archive receivers and `.done()`-guarded Future.result.

use ruff_python_ast::{self as ast, Expr};

use super::CallCollector;
use super::thread_guards::expr_attr_path;
use crate::extract::helpers::{
    is_archive_ctor_qual, is_disk_touching_archive_method, is_in_memory_buffer_type,
    is_pure_in_memory_archive_method,
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
        if let Expr::Name(n) = expr
            && self.buffer_locals.contains(n.id.as_str())
        {
            return true;
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
        if let Some(ty) = self.type_instance_qualname(expr)
            && is_in_memory_buffer_type(&ty)
        {
            return true;
        }
        false
    }

    pub(crate) fn call_is_archive_ctor(&mut self, func: &Expr) -> bool {
        if let Some(q) = self.external_qualname(func)
            && is_archive_ctor_qual(&q)
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
    pub(crate) fn call_is_in_memory_archive(&mut self, call: &ast::ExprCall) -> bool {
        if self.call_is_archive_ctor(&call.func) && self.first_arg_is_in_memory_buffer(call) {
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
        if !is_pure_in_memory_archive_method(method)
            && !self
                .external_qualname(&call.func)
                .as_deref()
                .is_some_and(|q| {
                    crate::extract::helpers::is_archive_method_qual(q)
                        && q.rsplit_once('.')
                            .is_some_and(|(_, m)| is_pure_in_memory_archive_method(m))
                })
        {
            return false;
        }
        match attr.value.as_ref() {
            Expr::Name(n) => self.in_memory_archive_receivers.contains(n.id.as_str()),
            Expr::Call(inner) => {
                self.call_is_archive_ctor(&inner.func) && self.first_arg_is_in_memory_buffer(inner)
            }
            _ => false,
        }
    }

    pub(crate) fn call_is_done_guarded_future_result(&self, call: &ast::ExprCall) -> bool {
        let Expr::Attribute(attr) = call.func.as_ref() else {
            return false;
        };
        if attr.attr.as_str() != "result" {
            return false;
        }
        let Some(path) = expr_attr_path(&attr.value) else {
            return false;
        };
        self.future_done_receivers.contains(&path)
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
