//! Sync boto3 vs aioboto3/aiobotocore receiver tracking.

use ruff_python_ast::Expr;

use super::CallCollector;

impl<'a, 'db> CallCollector<'a, 'db> {
    pub(crate) fn note_boto_binding(&mut self, name: &str, value: &Expr) {
        if self.expr_binds_aio_boto(value) {
            self.aio_boto_receivers.insert(name.to_string());
            self.boto_receivers.remove(name);
            return;
        }
        if self.expr_is_boto_factory_or_receiver(value) {
            self.boto_receivers.insert(name.to_string());
            self.aio_boto_receivers.remove(name);
        } else {
            self.boto_receivers.remove(name);
            self.aio_boto_receivers.remove(name);
        }
    }

    /// Sync `boto3.client` / `boto3.resource` / `Session.client` — never aioboto3.
    pub(crate) fn call_is_boto_factory(&mut self, func: &Expr) -> bool {
        if self.expr_is_aioboto_context(func) {
            return false;
        }
        let Some(q) = self.external_qualname(func) else {
            return false;
        };
        if is_aioboto_qualname(&q) {
            return false;
        }
        matches!(
            q.as_str(),
            "boto3.client"
                | "boto3.resource"
                | "boto3.session.Session.client"
                | "boto3.session.Session.resource"
                | "boto3.Session.client"
                | "boto3.Session.resource"
        ) || ((q.starts_with("boto3.") || q.starts_with("botocore."))
            && (q.ends_with(".Session.client") || q.ends_with(".Session.resource")))
    }

    pub(crate) fn expr_is_boto_factory_or_receiver(&mut self, expr: &Expr) -> bool {
        if self.expr_is_aio_boto_receiver(expr) {
            return false;
        }
        match expr {
            Expr::Name(n) => self.boto_receivers.contains(n.id.as_str()),
            Expr::Call(c) => {
                if self.call_is_boto_factory(&c.func) {
                    return true;
                }
                // `resource(...).Table(...)` / attribute factory on a boto recv.
                match c.func.as_ref() {
                    Expr::Attribute(a) => self.expr_is_boto_factory_or_receiver(&a.value),
                    other => self.expr_is_boto_factory_or_receiver(other),
                }
            }
            Expr::Attribute(a) => self.expr_is_boto_factory_or_receiver(&a.value),
            _ => false,
        }
    }

    pub(crate) fn expr_is_aio_boto_receiver(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Name(n) => self.aio_boto_receivers.contains(n.id.as_str()),
            Expr::Call(c) => match c.func.as_ref() {
                Expr::Attribute(a) => self.expr_is_aio_boto_receiver(&a.value),
                other => self.expr_is_aio_boto_receiver(other),
            },
            Expr::Attribute(a) => self.expr_is_aio_boto_receiver(&a.value),
            _ => false,
        }
    }

    /// True when `value` is an aioboto3/aiobotocore Session, client, or resource.
    pub(crate) fn expr_binds_aio_boto(&mut self, expr: &Expr) -> bool {
        match expr {
            Expr::Name(n) => self.aio_boto_receivers.contains(n.id.as_str()),
            Expr::Call(c) => {
                if self.call_is_aioboto_session_or_factory(&c.func) {
                    return true;
                }
                match c.func.as_ref() {
                    Expr::Attribute(a) => self.expr_binds_aio_boto(&a.value),
                    other => self.expr_binds_aio_boto(other),
                }
            }
            Expr::Attribute(a) => self.expr_binds_aio_boto(&a.value),
            _ => false,
        }
    }

    fn call_is_aioboto_session_or_factory(&mut self, func: &Expr) -> bool {
        if !self.expr_is_aioboto_context(func) {
            // `from aioboto3 import Session` — resolve the callee qualname.
            if let Some(q) = self.external_qualname(func)
                && is_aioboto_qualname(&q)
                && (q.ends_with(".Session")
                    || q.ends_with(".client")
                    || q.ends_with(".resource")
                    || q.ends_with(".Session.client")
                    || q.ends_with(".Session.resource"))
            {
                return true;
            }
            return false;
        }
        match func {
            Expr::Attribute(a) => matches!(a.attr.as_str(), "Session" | "client" | "resource"),
            Expr::Name(n) => matches!(n.id.as_str(), "Session" | "client" | "resource"),
            _ => false,
        }
    }

    /// Walk call/attr chains for an `aioboto3` / `aiobotocore` root (name or binding).
    pub(crate) fn expr_is_aioboto_context(&mut self, expr: &Expr) -> bool {
        match expr {
            Expr::Name(n) => {
                let s = n.id.as_str();
                if s == "aioboto3" || s == "aiobotocore" {
                    return true;
                }
                if self.aio_boto_receivers.contains(s) {
                    return true;
                }
                self.external_qualname(expr)
                    .as_deref()
                    .is_some_and(is_aioboto_qualname)
            }
            Expr::Attribute(a) => {
                if let Some(q) = self.external_qualname(expr)
                    && is_aioboto_qualname(&q)
                {
                    return true;
                }
                self.expr_is_aioboto_context(&a.value)
            }
            Expr::Call(c) => self.expr_is_aioboto_context(&c.func),
            _ => false,
        }
    }
}

fn is_aioboto_qualname(q: &str) -> bool {
    q == "aioboto3"
        || q == "aiobotocore"
        || q.starts_with("aioboto3.")
        || q.starts_with("aiobotocore.")
}
