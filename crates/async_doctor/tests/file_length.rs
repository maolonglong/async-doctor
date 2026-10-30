//! AGENTS.md hard ceiling: no crates/**/*.rs source file may exceed 800 lines.

use std::fs;
use std::path::PathBuf;

const MAX_LINES: usize = 800;

#[test]
fn no_crate_rust_source_exceeds_800_lines() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_crates = manifest_dir.join("..");
    let mut offenders = Vec::new();

    fn walk(dir: &std::path::Path, offenders: &mut Vec<(String, usize)>) {
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|s| s.to_str()) == Some("target") {
                    continue;
                }
                walk(&path, offenders);
            } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                let Ok(text) = fs::read_to_string(&path) else {
                    continue;
                };
                let lines = text.lines().count();
                if lines > MAX_LINES {
                    offenders.push((path.display().to_string(), lines));
                }
            }
        }
    }

    assert!(
        workspace_crates.join("async_doctor_core").is_dir(),
        "expected workspace crates dir at {}",
        workspace_crates.display()
    );
    walk(&workspace_crates, &mut offenders);
    offenders.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(
        offenders.is_empty(),
        "Rust source files exceed {MAX_LINES} lines (AGENTS.md):\n{}",
        offenders
            .iter()
            .map(|(p, n)| format!("  {p}: {n} lines"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
