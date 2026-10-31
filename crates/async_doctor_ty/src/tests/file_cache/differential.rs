//! Differential harness: after each random edit, warm/incremental JSON must
//! equal a fresh-cache cold scan (byte-identical).
//!
//! CI: every `fixtures/` entry, fixed seeds, small edit count (bounded).
//! Local: `ASYNC_DOCTOR_DIFF_EDITS` / `ASYNC_DOCTOR_DIFF_SEEDS` for longer runs.
//! Prints how many edits were executed.

use std::fs;
use std::path::{Path, PathBuf};

use super::super::helpers::{
    clone_fixture, fixtures_dir, fresh_cache_dir, report_json_with_cache, temp_project,
    with_cleared_virtual_env,
};
use super::helpers::{incremental_vs_cold, wipe_scan_cache};

/// Tiny deterministic PRNG (no extra crate).
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = if x == 0 { 0x9e37_79b9_7f4a_7c15 } else { x };
        self.0
    }

    fn gen_range(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() as usize) % n
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.gen_range(items.len())]
    }
}

#[derive(Clone, Copy)]
enum EditKind {
    BodyAppend,
    BodySleepToggle,
    AnnotationFlip,
    AddModule,
    DeleteModule,
    RenameModule,
    ImportSwap,
    AddSitePkgsChain,
}

const EDIT_KINDS: &[EditKind] = &[
    EditKind::BodyAppend,
    EditKind::BodySleepToggle,
    EditKind::AnnotationFlip,
    EditKind::AddModule,
    EditKind::DeleteModule,
    EditKind::RenameModule,
    EditKind::ImportSwap,
    EditKind::AddSitePkgsChain,
];

fn list_py_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == ".venv" || name == "venv" || name == "__pycache__" || name.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if name.ends_with(".py") || name.ends_with(".pyi") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn rel_under(root: &Path, abs: &Path) -> String {
    abs.strip_prefix(root)
        .map(|r| r.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| abs.display().to_string())
}

fn is_entrypointish(rel: &str) -> bool {
    let base = Path::new(rel)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    matches!(
        base,
        "app.py" | "main.py" | "svc.py" | "routes.py" | "__main__.py"
    ) || rel.starts_with("tests/")
        || base.starts_with("test_")
}

fn apply_edit(root: &Path, rng: &mut Rng, edit_i: usize) -> String {
    let files = list_py_files(root);
    if files.is_empty() {
        return format!("edit{edit_i}:empty");
    }
    let kind = *rng.pick(EDIT_KINDS);
    match kind {
        EditKind::BodyAppend => {
            let f = rng.pick(&files);
            let mut t = fs::read_to_string(f).unwrap_or_default();
            t.push_str(&format!("\n# diff-edit-{edit_i}-{}\n", rng.next_u64()));
            fs::write(f, t).unwrap();
            format!("body_append:{}", rel_under(root, f))
        }
        EditKind::BodySleepToggle => {
            let f = rng.pick(&files);
            let mut t = fs::read_to_string(f).unwrap_or_default();
            if t.contains("time.sleep(") {
                t = t.replacen("time.sleep(", "pass  # was sleep(", 1);
            } else if t.contains("def ") {
                t.push_str("\nimport time\ndef _diff_sleep_probe():\n    time.sleep(0.01)\n");
            } else {
                t.push_str(&format!("\n# sleep-toggle-{edit_i}\n"));
            }
            fs::write(f, t).unwrap();
            format!("sleep_toggle:{}", rel_under(root, f))
        }
        EditKind::AnnotationFlip => {
            let f = rng.pick(&files);
            let mut t = fs::read_to_string(f).unwrap_or_default();
            if t.contains("-> Safe") {
                t = t.replacen("-> Safe", "-> Blocking", 1);
            } else if t.contains("-> Blocking") {
                t = t.replacen("-> Blocking", "-> Safe", 1);
            } else if t.contains("-> None") {
                t = t.replacen("-> None", "-> object", 1);
            } else {
                t.push_str(&format!("\n# ann-flip-{edit_i}\n"));
            }
            fs::write(f, t).unwrap();
            format!("ann_flip:{}", rel_under(root, f))
        }
        EditKind::AddModule => {
            let name = format!("_diff_mod_{edit_i}");
            let pkg = root.join("pkg");
            let dest_dir = if pkg.is_dir() { pkg } else { root.to_path_buf() };
            fs::create_dir_all(&dest_dir).unwrap();
            let dest = dest_dir.join(format!("{name}.py"));
            fs::write(
                &dest,
                "import time\ndef diff_work():\n    time.sleep(0.01)\n",
            )
            .unwrap();
            // Hook an entrypoint so the new module enters the scan graph.
            if let Some(entry) = files.iter().find(|p| is_entrypointish(&rel_under(root, p))) {
                let mut t = fs::read_to_string(entry).unwrap_or_default();
                let import = if dest_dir.ends_with("pkg") {
                    format!("from pkg.{name} import diff_work\n")
                } else {
                    format!("from {name} import diff_work\n")
                };
                if !t.contains(&import) {
                    t = format!("{import}{t}");
                    if !t.contains("diff_work()") {
                        t.push_str("\nasync def _diff_entry():\n    diff_work()\n");
                    }
                    fs::write(entry, t).unwrap();
                }
            }
            format!("add_module:{}", rel_under(root, &dest))
        }
        EditKind::DeleteModule => {
            let candidates: Vec<_> = files
                .iter()
                .filter(|p| {
                    let r = rel_under(root, p);
                    !is_entrypointish(&r)
                        && !r.ends_with("__init__.py")
                        && !r.contains("site-packages")
                })
                .cloned()
                .collect();
            if candidates.is_empty() {
                return apply_edit_fallback_append(root, &files, edit_i, "delete_fallback");
            }
            let f = rng.pick(&candidates);
            let rel = rel_under(root, f);
            let _ = fs::remove_file(f);
            format!("delete_module:{rel}")
        }
        EditKind::RenameModule => {
            let candidates: Vec<_> = files
                .iter()
                .filter(|p| {
                    let r = rel_under(root, p);
                    !is_entrypointish(&r)
                        && !r.ends_with("__init__.py")
                        && Path::new(&r)
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .is_some_and(|s| !s.starts_with("_diff_ren_"))
                })
                .cloned()
                .collect();
            if candidates.is_empty() {
                return apply_edit_fallback_append(root, &files, edit_i, "rename_fallback");
            }
            let f = rng.pick(&candidates);
            let parent = f.parent().unwrap().to_path_buf();
            let new_name = format!("_diff_ren_{edit_i}.py");
            let dest = parent.join(&new_name);
            let old_rel = rel_under(root, f);
            fs::rename(f, &dest).unwrap();
            // Best-effort rewrite of imports that named the old module.
            let old_stem = Path::new(&old_rel)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            let new_stem = format!("_diff_ren_{edit_i}");
            if !old_stem.is_empty() {
                for py in list_py_files(root) {
                    let Ok(mut t) = fs::read_to_string(&py) else {
                        continue;
                    };
                    let before = t.clone();
                    t = t.replace(&format!("import {old_stem}"), &format!("import {new_stem}"));
                    t = t.replace(&format!(".{old_stem} "), &format!(".{new_stem} "));
                    t = t.replace(&format!(".{old_stem}\n"), &format!(".{new_stem}\n"));
                    t = t.replace(&format!(".{old_stem} import"), &format!(".{new_stem} import"));
                    if t != before {
                        let _ = fs::write(&py, t);
                    }
                }
            }
            format!("rename_module:{old_rel}->{new_name}")
        }
        EditKind::ImportSwap => {
            let f = rng.pick(&files);
            let mut t = fs::read_to_string(f).unwrap_or_default();
            if t.contains("import time") {
                t = t.replacen("import time", "import time as _time_alias", 1);
                t = t.replace("time.sleep", "_time_alias.sleep");
            } else {
                t.push_str("\nimport time as _diff_time\n");
            }
            fs::write(f, t).unwrap();
            format!("import_swap:{}", rel_under(root, f))
        }
        EditKind::AddSitePkgsChain => {
            let sp = find_site_packages(root);
            let Some(sp) = sp else {
                return apply_edit_fallback_append(root, &files, edit_i, "sitepkgs_fallback");
            };
            let name = format!("_diff_tp_{edit_i}");
            let pkg = sp.join(&name);
            fs::create_dir_all(&pkg).unwrap();
            fs::write(pkg.join("__init__.py"), "").unwrap();
            fs::write(
                pkg.join("block.py"),
                "import time\ndef block():\n    time.sleep(0.01)\n",
            )
            .unwrap();
            // dist-info so venv_fingerprint flips (pip-install shaped).
            let dist = sp.join(format!("{name}-0.0.1.dist-info"));
            fs::create_dir_all(&dist).unwrap();
            fs::write(
                dist.join("METADATA"),
                format!("Metadata-Version: 2.1\nName: {name}\nVersion: 0.0.1\n"),
            )
            .unwrap();
            if let Some(entry) = files.iter().find(|p| is_entrypointish(&rel_under(root, p))) {
                let mut t = fs::read_to_string(entry).unwrap_or_default();
                let import = format!("from {name}.block import block\n");
                if !t.contains(&import) {
                    t = format!("{import}{t}");
                    t.push_str("\nasync def _diff_tp_entry():\n    block()\n");
                    fs::write(entry, t).unwrap();
                }
            }
            format!("add_sitepkgs:{name}")
        }
    }
}

fn apply_edit_fallback_append(root: &Path, files: &[PathBuf], edit_i: usize, tag: &str) -> String {
    let f = &files[0];
    let mut t = fs::read_to_string(f).unwrap_or_default();
    t.push_str(&format!("\n# {tag}-{edit_i}\n"));
    fs::write(f, t).unwrap();
    format!("{tag}:{}", rel_under(root, f))
}

fn find_site_packages(root: &Path) -> Option<PathBuf> {
    for base in [root.join(".venv"), root.join("venv")] {
        let lib = base.join("lib");
        let Ok(rd) = fs::read_dir(&lib) else {
            continue;
        };
        for ent in rd.flatten() {
            let sp = ent.path().join("site-packages");
            if sp.is_dir() {
                return Some(sp);
            }
        }
    }
    None
}

fn fixture_names() -> Vec<String> {
    let root = fixtures_dir();
    let mut names = Vec::new();
    let Ok(rd) = fs::read_dir(&root) else {
        return names;
    };
    for ent in rd.flatten() {
        let name = ent.file_name().to_string_lossy().to_string();
        if name == "advice_shared_venv" || name.starts_with('.') {
            continue;
        }
        let p = ent.path();
        if p.is_dir() {
            // Skip empty / non-python trees.
            if list_py_files(&p).is_empty() {
                continue;
            }
            names.push(name);
        } else if name.ends_with(".py") {
            names.push(name);
        }
    }
    names.sort();
    names
}

fn materialize_fixture(name: &str) -> PathBuf {
    if name.ends_with(".py") {
        let src = fixtures_dir().join(name);
        let body = fs::read_to_string(&src).unwrap_or_default();
        temp_project(&[(name, &body)])
    } else {
        clone_fixture(name)
    }
}

fn run_differential_on(name: &str, seed: u64, edits: usize) -> usize {
    // Hold VIRTUAL_ENV lock so ambient/parallel tests cannot redirect the venv
    // mid-compare (project `.venv` must win).
    with_cleared_virtual_env(|| {
        let dir = materialize_fixture(name);
        let cache = fresh_cache_dir(&format!("diff-{name}-{seed}"));
        let _ = report_json_with_cache(&dir, &cache);
        let mut rng = Rng(seed ^ 0xA5A5_A5A5_A5A5_A5A5);
        let mut done = 0usize;
        for i in 0..edits {
            let label = apply_edit(&dir, &mut rng, i);
            wipe_scan_cache(&cache);
            let incremental = report_json_with_cache(&dir, &cache);
            let cold_cache = fresh_cache_dir(&format!("diff-cold-{name}-{seed}-{i}"));
            let cold = report_json_with_cache(&dir, &cold_cache);
            assert_eq!(
                incremental, cold,
                "differential {name} seed={seed} edit={i} ({label}): incremental != cold\nincr={incremental}\ncold={cold}"
            );
            let _ = fs::remove_dir_all(&cold_cache);
            done += 1;
        }
        let _ = fs::remove_dir_all(&cache);
        let _ = fs::remove_dir_all(&dir);
        done
    })
}

fn parse_seeds() -> Vec<u64> {
    if let Ok(raw) = std::env::var("ASYNC_DOCTOR_DIFF_SEEDS") {
        let parsed: Vec<u64> = raw
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if !parsed.is_empty() {
            return parsed;
        }
    }
    vec![1, 42]
}

fn edit_budget() -> usize {
    std::env::var("ASYNC_DOCTOR_DIFF_EDITS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2)
}

/// Fixed-seed bounded suite over every fixture (CI default).
#[test]
fn differential_ci_seed_suite() {
    let seeds = parse_seeds();
    let edits = edit_budget();
    let names = fixture_names();
    assert!(
        !names.is_empty(),
        "expected fixtures under {}",
        fixtures_dir().display()
    );
    let mut total = 0usize;
    for name in &names {
        for &seed in &seeds {
            total += run_differential_on(name, seed, edits);
        }
    }
    eprintln!(
        "differential_ci_seed_suite: fixtures={} seeds={} edits_per={} total_edits={total}",
        names.len(),
        seeds.len(),
        edits
    );
    assert!(total > 0, "no edits executed");
}

/// Longer local run (opt-in). Example:
/// `ASYNC_DOCTOR_DIFF_EDITS=12 ASYNC_DOCTOR_DIFF_SEEDS=7,11 cargo test -p async_doctor_ty -- differential_local_extended --nocapture --ignored`
#[test]
#[ignore = "long local differential; set ASYNC_DOCTOR_DIFF_EDITS / ASYNC_DOCTOR_DIFF_SEEDS"]
fn differential_local_extended() {
    if std::env::var("ASYNC_DOCTOR_DIFF_EDITS").is_err() {
        unsafe {
            std::env::set_var("ASYNC_DOCTOR_DIFF_EDITS", "8");
        }
    }
    if std::env::var("ASYNC_DOCTOR_DIFF_SEEDS").is_err() {
        unsafe {
            std::env::set_var("ASYNC_DOCTOR_DIFF_SEEDS", "7,11,13,17");
        }
    }
    differential_ci_seed_suite();
}

/// Focused multipkg identity (the 4→0 regression).
#[test]
fn differential_m2_multipkg_other_py() {
    let dir = clone_fixture("m2_cache_multipkg");
    let cache = fresh_cache_dir("diff-m2-multipkg");
    // Fixture ships `.venv`; discovery uses the in-project venv (no VIRTUAL_ENV).
    let cold0 = report_json_with_cache(&dir, &cache);
    let n0: usize = serde_json::from_str::<serde_json::Value>(&cold0).unwrap()["findings"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(n0, 4, "cold multipkg should report 4 findings, got {n0}");
    fs::write(dir.join("other.py"), "def unrelated():\n    return 1\n").unwrap();
    incremental_vs_cold(&dir, &cache, "m2-multipkg-other");
    let after = report_json_with_cache(&dir, &fresh_cache_dir("diff-m2-multipkg-after"));
    let n1: usize = serde_json::from_str::<serde_json::Value>(&after).unwrap()["findings"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(n1, 4, "after unrelated other.py still 4 findings, got {n1}");
}
