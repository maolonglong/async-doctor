//! Ty/ruff adapter: the only crate allowed to depend on Astral's ty and ruff crates.

mod db;
mod extract;
mod scan;

pub use db::AsyncDoctorDb;
pub use extract::extract_path;
pub use scan::scan_path;

#[cfg(test)]
mod tests;
