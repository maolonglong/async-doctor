//! Ty/ruff adapter: the only crate allowed to depend on Astral's ty and ruff crates.

mod cache;
mod db;
mod extract;
mod scan;
mod sitepkgs;

pub use db::AsyncDoctorDb;
pub use extract::extract_path;
pub use scan::{scan_path, scan_path_with_options};

#[cfg(test)]
mod tests;
