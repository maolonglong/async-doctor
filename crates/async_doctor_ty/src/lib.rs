//! Ty/ruff adapter: the only crate allowed to depend on Astral's ty and ruff crates.

mod cache;
mod db;
mod extract;
mod scan;
mod scan_cache;
mod search_paths;
mod sitepkgs;
mod timing;
mod twin_resolve;

pub use db::AsyncDoctorDb;
pub use extract::{discover_python_files, extract_path, extract_path_with_cache};
pub use scan::{
    scan_path, scan_path_with_cache, scan_path_with_options, scan_path_with_options_and_cache,
};

#[cfg(test)]
mod tests;
