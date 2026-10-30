//! Ty/ruff adapter: the only crate allowed to depend on Astral's ty and ruff crates.

mod cache;
mod db;
mod extract;
mod scan;
mod sitepkgs;

pub use db::AsyncDoctorDb;
pub use extract::{
    FOLLOW_BUDGET_DEFAULT, extract_path, extract_path_with_cache, extract_path_with_cache_budget,
};
pub use scan::{
    scan_path, scan_path_with_cache, scan_path_with_cache_budget, scan_path_with_options,
    scan_path_with_options_and_cache,
};

#[cfg(test)]
mod tests;
