//! Ty/ruff adapter: the only crate allowed to depend on Astral's ty and ruff crates.

mod db;
mod scan;

pub use db::AsyncDoctorDb;
pub use scan::scan_path;
