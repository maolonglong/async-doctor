//! Stdlib / builtins sink catalogue and lookup.

mod sinks;
mod table;
#[cfg(test)]
mod tests;

pub use sinks::STDLIB_SINKS;
pub use table::StdlibSinkTable;
