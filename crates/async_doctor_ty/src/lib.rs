//! Ty/ruff adapter: the only crate allowed to depend on Astral's ty and ruff crates.

mod cache;
mod db;
mod doctor_system;
mod encoding;
mod env_info;
mod extract;
mod scan;
mod scan_cache;
mod search_paths;
mod sitepkgs;
mod timing;
mod twin_resolve;

pub use db::AsyncDoctorDb;
pub use env_info::discover_scan_environment;
pub use extract::{discover_python_files, extract_path, extract_path_with_cache};
pub use scan::{
    scan_path, scan_path_with_cache, scan_path_with_options, scan_path_with_options_and_cache,
};
pub use scan_cache::take_ignore_candidate_files;

#[cfg(test)]
mod hermetic_env;
#[cfg(test)]
mod tests;
