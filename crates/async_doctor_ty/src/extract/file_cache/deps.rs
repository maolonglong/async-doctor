//! Transitive AST-import dependency fingerprints for per-file cache hits.
//!
//! `deps_fp(F)` hashes sorted `{rel}\t{content_fp_or_?}` lines for every
//! first-party file in F's transitive import closure, plus synthetic absent
//! probes for unresolved import heads. Cross-module inference in the review
//! scenarios flows through imports, so content-hash changes in that closure
//! invalidate F without a per-file salsa reset.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use ruff_db::system::SystemPathBuf;
use ruff_python_ast::Stmt;
use ruff_python_parser::parse_module;

use crate::extract::path::module_name_from_relative;

use super::short_hash;

/// Sentinel content fingerprint for a probed-but-missing first-party path.
pub const ABSENT_DEP_FP: &str = "?";

/// Catalog entry needed to build the direct-import graph.
#[derive(Debug, Clone)]
pub struct DepCatalogEntry {
    pub abs: std::path::PathBuf,
    pub rel: String,
    pub module: String,
}

/// First-party module search roots matching ty's layout (`db.rs`):
/// project root, `src/` when present, package-parent when scanning a package
/// dir, plus config / editable-finder extras (without opening a salsa Db).
pub fn first_party_search_roots(project_root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![project_root.to_path_buf()];
    let src_layout = project_root.join("src");
    if src_layout.is_dir() {
        roots.push(src_layout);
    }
    if project_root.join("__init__.py").is_file()
        && let Some(parent) = project_root.parent()
    {
        let parent_buf = parent.to_path_buf();
        if !roots.iter().any(|r| r == &parent_buf) {
            roots.push(parent_buf);
        }
    }
    let Ok(sys_root) = SystemPathBuf::from_path_buf(project_root.to_path_buf()) else {
        return roots;
    };
    let existing: Vec<SystemPathBuf> = roots
        .iter()
        .filter_map(|r| SystemPathBuf::from_path_buf(r.clone()).ok())
        .collect();
    let site: Vec<SystemPathBuf> = Vec::new();
    for extra in crate::search_paths::discover_extra_paths(&sys_root, &site, &existing) {
        let p = PathBuf::from(extra.as_str());
        if p.is_dir() && !roots.iter().any(|r| r == &p) {
            roots.push(p);
        }
    }
    roots
}

/// Module name of `abs` when imported via search root `root` (exact strip).
pub fn module_under_search_root(root: &Path, abs: &Path) -> Option<String> {
    let rel = abs.strip_prefix(root).ok()?;
    let rel = rel.to_string_lossy().replace('\\', "/");
    if rel.is_empty() {
        return None;
    }
    Some(module_name_from_relative(&rel))
}

/// Prefer the longest matching search-root prefix (e.g. `src/` over project).
pub fn importable_module(abs: &Path, roots: &[PathBuf]) -> String {
    let mut best: Option<(usize, String)> = None;
    for root in roots {
        let Ok(canon_root) = root.canonicalize() else {
            if let Some(m) = module_under_search_root(root, abs) {
                let len = root.as_os_str().len();
                if best.as_ref().is_none_or(|(l, _)| len > *l) {
                    best = Some((len, m));
                }
            }
            continue;
        };
        let abs_c = abs.canonicalize().unwrap_or_else(|_| abs.to_path_buf());
        if let Some(m) = module_under_search_root(&canon_root, &abs_c) {
            let len = canon_root.as_os_str().len();
            if best.as_ref().is_none_or(|(l, _)| len > *l) {
                best = Some((len, m));
            }
        }
    }
    best.map(|(_, m)| m).unwrap_or_default()
}

/// Build `module_name (+ search-root / package aliases) → rel` for exact resolution.
pub fn build_module_to_rel(
    catalog: &[DepCatalogEntry],
    scan_root: &Path,
) -> HashMap<String, String> {
    let roots = first_party_search_roots(scan_root);
    let mut out = HashMap::with_capacity(catalog.len().saturating_mul(4));
    for e in catalog {
        for alias in module_name_aliases(scan_root, &e.module) {
            out.insert(alias, e.rel.clone());
        }
        for root in &roots {
            if let Some(m) = module_under_search_root(root, &e.abs)
                && !m.is_empty()
            {
                out.insert(m, e.rel.clone());
            }
            // Also try canonical paths when abs is already canonicalized.
            if let (Ok(cr), Ok(ca)) = (root.canonicalize(), e.abs.canonicalize())
                && let Some(m) = module_under_search_root(&cr, &ca)
                && !m.is_empty()
            {
                out.insert(m, e.rel.clone());
            }
        }
    }
    out
}

/// Package-root aliases: when scanning `pkg/` itself, `util` is also `pkg.util`.
pub fn module_name_aliases(scan_root: &Path, module: &str) -> Vec<String> {
    let mut names = Vec::with_capacity(2);
    names.push(module.to_string());
    if scan_root.join("__init__.py").is_file()
        && let Some(pkg) = scan_root.file_name().and_then(|n| n.to_str())
        && !pkg.is_empty()
        && pkg != "."
        && pkg != ".."
    {
        if module.is_empty() {
            if !names.iter().any(|n| n == pkg) {
                names.push(pkg.to_string());
            }
        } else {
            let aliased = format!("{pkg}.{module}");
            if !names.iter().any(|n| n == &aliased) {
                names.push(aliased);
            }
        }
    }
    names
}

/// Exact catalog / alias match only — **no** parent fallback.
pub fn resolve_head_to_rel(
    head: &str,
    module_to_rel: &HashMap<String, String>,
    _scan_root: &Path,
) -> Option<String> {
    module_to_rel.get(head).cloned()
}

/// Project-relative absent probes under every first-party search root.
///
/// `from pkg.newm import f` with a `src/` layout probes `src/pkg/newm.py`
/// (and `__init__`), not only `<root>/pkg/newm.py`.
pub fn absent_probe_rels(head: &str, project_root: &Path, roots: &[PathBuf]) -> Vec<String> {
    if head.is_empty() || head.starts_with('.') {
        return Vec::new();
    }
    let path = head.replace('.', "/");
    let suffixes = [format!("{path}.py"), format!("{path}/__init__.py")];
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let proj_canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    for root in roots {
        for suffix in &suffixes {
            let abs = root.join(suffix);
            let rel = if let Ok(r) = abs.strip_prefix(project_root) {
                r.to_string_lossy().replace('\\', "/")
            } else if let Ok(r) = abs.strip_prefix(&proj_canon) {
                r.to_string_lossy().replace('\\', "/")
            } else if let (Ok(cr), Ok(ca)) = (root.canonicalize(), abs.canonicalize()) {
                match ca.strip_prefix(&proj_canon) {
                    Ok(r) => r.to_string_lossy().replace('\\', "/"),
                    Err(_) => match ca.strip_prefix(&cr) {
                        // Root outside the project (editable): keep root-relative
                        // marker prefixed so it cannot collide with in-tree paths.
                        Ok(r) => {
                            let name = cr.file_name().and_then(|n| n.to_str()).unwrap_or("ext");
                            let rest = r.to_string_lossy().replace('\\', "/");
                            format!("__root__/{name}/{rest}")
                        }
                        Err(_) => continue,
                    },
                }
            } else {
                continue;
            };
            if seen.insert(rel.clone()) {
                out.push(rel);
            }
        }
    }
    out
}
/// Import module heads scraped via AST (sorted unique).
///
/// - `import a.b` → `a.b`
/// - `from X import Y` → `X`, `X.Y`, bare `Y`
/// - relatives resolved against `file_module`
///
/// Walks nested function/class bodies so function-local imports count.
pub fn scrape_import_heads_ast(source: &str, file_module: &str) -> Vec<String> {
    let Ok(parsed) = parse_module(source) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for stmt in &parsed.syntax().body {
        collect_import_heads(stmt, file_module, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

fn collect_import_heads(stmt: &Stmt, file_module: &str, out: &mut Vec<String>) {
    match stmt {
        Stmt::Import(i) => {
            for alias in &i.names {
                let full = alias.name.as_str();
                if !full.is_empty() {
                    out.push(full.to_string());
                }
            }
        }
        Stmt::ImportFrom(i) => {
            let base = if i.level > 0 {
                resolve_relative_import(file_module, i.level, i.module.as_ref().map(|m| m.as_str()))
            } else {
                i.module.as_ref().map(|m| m.as_str().to_string())
            };
            if let Some(ref b) = base
                && !b.is_empty()
            {
                out.push(b.clone());
            }
            for alias in &i.names {
                let name = alias.name.as_str();
                if name.is_empty() || name == "*" {
                    continue;
                }
                out.push(name.to_string());
                if let Some(ref b) = base {
                    if b.is_empty() {
                        out.push(name.to_string());
                    } else {
                        out.push(format!("{b}.{name}"));
                    }
                }
            }
        }
        Stmt::FunctionDef(f) => {
            for s in &f.body {
                collect_import_heads(s, file_module, out);
            }
        }
        Stmt::ClassDef(c) => {
            for s in &c.body {
                collect_import_heads(s, file_module, out);
            }
        }
        Stmt::If(i) => {
            for s in &i.body {
                collect_import_heads(s, file_module, out);
            }
            for elif in &i.elif_else_clauses {
                for s in &elif.body {
                    collect_import_heads(s, file_module, out);
                }
            }
        }
        Stmt::While(w) => {
            for s in &w.body {
                collect_import_heads(s, file_module, out);
            }
            for s in &w.orelse {
                collect_import_heads(s, file_module, out);
            }
        }
        Stmt::For(f) => {
            for s in &f.body {
                collect_import_heads(s, file_module, out);
            }
            for s in &f.orelse {
                collect_import_heads(s, file_module, out);
            }
        }
        Stmt::With(w) => {
            for s in &w.body {
                collect_import_heads(s, file_module, out);
            }
        }
        Stmt::Match(m) => {
            for case in &m.cases {
                for s in &case.body {
                    collect_import_heads(s, file_module, out);
                }
            }
        }
        Stmt::Try(t) => {
            for s in &t.body {
                collect_import_heads(s, file_module, out);
            }
            for h in &t.handlers {
                let ruff_python_ast::ExceptHandler::ExceptHandler(eh) = h;
                for s in &eh.body {
                    collect_import_heads(s, file_module, out);
                }
            }
            for s in &t.orelse {
                collect_import_heads(s, file_module, out);
            }
            for s in &t.finalbody {
                collect_import_heads(s, file_module, out);
            }
        }
        _ => {}
    }
}

fn resolve_relative_import(file_module: &str, level: u32, module: Option<&str>) -> Option<String> {
    if level == 0 {
        return module.map(str::to_string);
    }
    let mut parts: Vec<&str> = file_module.split('.').filter(|s| !s.is_empty()).collect();
    if !file_module.is_empty() {
        let _ = parts.pop();
    }
    let up = (level as usize).saturating_sub(1);
    if up > parts.len() {
        return None;
    }
    parts.truncate(parts.len() - up);
    if let Some(m) = module.filter(|s| !s.is_empty()) {
        parts.extend(m.split('.').filter(|s| !s.is_empty()));
    }
    if parts.is_empty() {
        Some(String::new())
    } else {
        Some(parts.join("."))
    }
}

/// Resolve scraped heads to first-party rels + absent probes for misses.
pub fn direct_deps_for_source(
    source: &str,
    file_module: &str,
    module_to_rel: &HashMap<String, String>,
    scan_root: &Path,
    search_roots: &[PathBuf],
) -> (Vec<String>, Vec<String>) {
    let mut deps = Vec::new();
    let mut absents = Vec::new();
    let mut seen_dep = HashSet::new();
    let mut seen_absent = HashSet::new();
    for head in scrape_import_heads_ast(source, file_module) {
        if let Some(rel) = resolve_head_to_rel(&head, module_to_rel, scan_root) {
            if seen_dep.insert(rel.clone()) {
                deps.push(rel);
            }
            continue;
        }
        for probe in absent_probe_rels(&head, scan_root, search_roots) {
            if seen_absent.insert(probe.clone()) {
                absents.push(probe);
            }
        }
    }
    deps.sort();
    absents.sort();
    (deps, absents)
}

/// `(direct_deps, absent_probes, content_fps)` keyed by relative path.
pub type DepsAndFps = (
    HashMap<String, Vec<String>>,
    HashMap<String, Vec<String>>,
    HashMap<String, String>,
);

/// Content fingerprints only (no AST scrape) — cheap wide-miss preflight.
pub fn build_content_fps_parallel(catalog: &[DepCatalogEntry]) -> HashMap<String, String> {
    catalog
        .par_iter()
        .filter_map(|e| {
            let bytes = std::fs::read(&e.abs).ok()?;
            Some((e.rel.clone(), short_hash(&bytes)))
        })
        .collect()
}

/// One parallel pass: content fingerprints + direct deps + absents.
///
/// Avoids double-reading every catalog file on cold save. Uses importable
/// module names under ty search roots so `src/` layouts resolve.
pub fn build_deps_and_fps_parallel(
    catalog: &[DepCatalogEntry],
    module_to_rel: &HashMap<String, String>,
    scan_root: &Path,
) -> DepsAndFps {
    let search_roots = first_party_search_roots(scan_root);
    let pairs: Vec<(String, Vec<String>, Vec<String>, String)> = catalog
        .par_iter()
        .filter_map(|e| {
            let bytes = std::fs::read(&e.abs).ok()?;
            let fp = short_hash(&bytes);
            let source = String::from_utf8_lossy(&bytes);
            let file_module = {
                let m = importable_module(&e.abs, &search_roots);
                if m.is_empty() { e.module.clone() } else { m }
            };
            let (deps, absents) = direct_deps_for_source(
                &source,
                &file_module,
                module_to_rel,
                scan_root,
                &search_roots,
            );
            Some((e.rel.clone(), deps, absents, fp))
        })
        .collect();
    let mut direct = HashMap::with_capacity(pairs.len());
    let mut absents = HashMap::with_capacity(pairs.len());
    let mut content_fps = HashMap::with_capacity(pairs.len());
    for (rel, deps, ab, fp) in pairs {
        content_fps.insert(rel.clone(), fp);
        direct.insert(rel.clone(), deps);
        absents.insert(rel, ab);
    }
    (direct, absents, content_fps)
}
/// FNV hash of sorted `{rel}\t{fp}` lines for the transitive import closure
/// of `start_rel`, plus that file's absent probes.
pub fn transitive_deps_fp(
    start_rel: &str,
    direct: &HashMap<String, Vec<String>>,
    content_fps: &HashMap<String, String>,
    absents_for_start: &[String],
) -> String {
    let mut seen = BTreeSet::new();
    let mut stack = vec![start_rel.to_string()];
    while let Some(rel) = stack.pop() {
        if !seen.insert(rel.clone()) {
            continue;
        }
        if let Some(deps) = direct.get(&rel) {
            for d in deps {
                if !seen.contains(d) {
                    stack.push(d.clone());
                }
            }
        }
    }
    let mut lines: Vec<String> = seen
        .into_iter()
        .map(|rel| {
            let fp = content_fps
                .get(&rel)
                .cloned()
                .unwrap_or_else(|| ABSENT_DEP_FP.to_string());
            format!("{rel}\t{fp}")
        })
        .collect();
    for probe in absents_for_start {
        // Absent probes always hash as "?". When the path later appears and
        // resolves, it moves into `direct` with a real content hash instead.
        lines.push(format!("{probe}\t{ABSENT_DEP_FP}"));
    }
    lines.sort();
    lines.dedup();
    let mut blob = String::from("deps-fp-v1\n");
    for line in lines {
        blob.push_str(&line);
        blob.push('\n');
    }
    short_hash(blob.as_bytes())
}

/// Live `deps_fp` for `rel` using a shared direct graph.
pub fn live_deps_fp(
    rel: &str,
    direct: &HashMap<String, Vec<String>>,
    absents_by_rel: &HashMap<String, Vec<String>>,
    content_fps: &HashMap<String, String>,
) -> String {
    let absents = absents_by_rel.get(rel).map(Vec::as_slice).unwrap_or(&[]);
    transitive_deps_fp(rel, direct, content_fps, absents)
}

/// Compute `deps_fp` for many files via SCC condensation + bottom-up hashes.
///
/// Sound for invalidation (any transitive content / absent-probe change
/// flips the fp) and O(N+E). Uses `deps-fp-v2` so values differ from the
/// single-file v1 helper — both are only compared against slots written by
/// the same algorithm.
pub fn live_deps_fps_parallel(
    rels: &[String],
    direct: &HashMap<String, Vec<String>>,
    absents_by_rel: &HashMap<String, Vec<String>>,
    content_fps: &HashMap<String, String>,
) -> HashMap<String, String> {
    if rels.is_empty() {
        return HashMap::new();
    }
    let mut nodes: BTreeSet<String> = BTreeSet::new();
    for r in rels {
        nodes.insert(r.clone());
    }
    for (k, deps) in direct {
        nodes.insert(k.clone());
        for d in deps {
            nodes.insert(d.clone());
        }
    }
    let node_list: Vec<String> = nodes.into_iter().collect();
    let index_of: HashMap<&str, usize> = node_list
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();
    let n = node_list.len();
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (k, deps) in direct {
        let Some(&u) = index_of.get(k.as_str()) else {
            continue;
        };
        for d in deps {
            if let Some(&v) = index_of.get(d.as_str()) {
                adj[u].push(v);
            }
        }
    }

    let mut disc = vec![-1i32; n];
    let mut low = vec![0i32; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut on_stack = vec![false; n];
    let mut time = 0i32;
    let mut scc_of = vec![0usize; n];
    let mut sccs: Vec<Vec<usize>> = Vec::new();
    #[allow(clippy::too_many_arguments)]
    fn strongconnect(
        u: usize,
        adj: &[Vec<usize>],
        disc: &mut [i32],
        low: &mut [i32],
        stack: &mut Vec<usize>,
        on_stack: &mut [bool],
        time: &mut i32,
        scc_of: &mut [usize],
        sccs: &mut Vec<Vec<usize>>,
    ) {
        disc[u] = *time;
        low[u] = *time;
        *time += 1;
        stack.push(u);
        on_stack[u] = true;
        for &v in &adj[u] {
            if disc[v] == -1 {
                strongconnect(v, adj, disc, low, stack, on_stack, time, scc_of, sccs);
                low[u] = low[u].min(low[v]);
            } else if on_stack[v] {
                low[u] = low[u].min(disc[v]);
            }
        }
        if low[u] == disc[u] {
            let mut comp = Vec::new();
            let id = sccs.len();
            loop {
                let w = stack.pop().expect("tarjan stack");
                on_stack[w] = false;
                scc_of[w] = id;
                comp.push(w);
                if w == u {
                    break;
                }
            }
            sccs.push(comp);
        }
    }
    for u in 0..n {
        if disc[u] == -1 {
            strongconnect(
                u,
                &adj,
                &mut disc,
                &mut low,
                &mut stack,
                &mut on_stack,
                &mut time,
                &mut scc_of,
                &mut sccs,
            );
        }
    }
    let scc_n = sccs.len();
    let mut scc_adj: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); scc_n];
    for u in 0..n {
        let su = scc_of[u];
        for &v in &adj[u] {
            let sv = scc_of[v];
            if su != sv {
                scc_adj[su].insert(sv);
            }
        }
    }
    // Body hash per SCC: sorted member `rel\tcontent_fp` lines.
    let mut scc_body: Vec<String> = Vec::with_capacity(scc_n);
    for comp in &sccs {
        let mut lines: Vec<String> = comp
            .iter()
            .map(|&u| {
                let rel = &node_list[u];
                let fp = content_fps
                    .get(rel)
                    .map(String::as_str)
                    .unwrap_or(ABSENT_DEP_FP);
                format!("{rel}\t{fp}")
            })
            .collect();
        lines.sort();
        lines.dedup();
        let mut blob = String::from("scc-body\n");
        for line in lines {
            blob.push_str(&line);
            blob.push('\n');
        }
        scc_body.push(short_hash(blob.as_bytes()));
    }
    // Tarjan emits sinks first — bottom-up reach hash on the condensed DAG.
    let mut scc_reach: Vec<String> = vec![String::new(); scc_n];
    for s in 0..scc_n {
        let mut parts: Vec<&str> = Vec::with_capacity(1 + scc_adj[s].len());
        parts.push(scc_body[s].as_str());
        for &child in &scc_adj[s] {
            parts.push(scc_reach[child].as_str());
        }
        parts.sort();
        let mut blob = String::from("scc-reach\n");
        for p in parts {
            blob.push_str(p);
            blob.push('\n');
        }
        scc_reach[s] = short_hash(blob.as_bytes());
    }

    let mut out = HashMap::with_capacity(rels.len());
    for rel in rels {
        let Some(&u) = index_of.get(rel.as_str()) else {
            out.insert(
                rel.clone(),
                live_deps_fp(rel, direct, absents_by_rel, content_fps),
            );
            continue;
        };
        let mut blob = String::from("deps-fp-v2\n");
        blob.push_str(&scc_reach[scc_of[u]]);
        blob.push('\n');
        if let Some(absents) = absents_by_rel.get(rel) {
            let mut probes = absents.clone();
            probes.sort();
            probes.dedup();
            for probe in probes {
                blob.push_str(&probe);
                blob.push('\t');
                blob.push_str(ABSENT_DEP_FP);
                blob.push('\n');
            }
        }
        out.insert(rel.clone(), short_hash(blob.as_bytes()));
    }
    out
}

#[cfg(test)]
mod scrape_tests {
    use super::*;

    #[test]
    fn from_pkg_import_util_heads() {
        let heads = scrape_import_heads_ast("from pkg import util\n", "app");
        assert!(heads.iter().any(|h| h == "pkg"));
        assert!(heads.iter().any(|h| h == "pkg.util"));
        assert!(heads.iter().any(|h| h == "util"));
    }

    #[test]
    fn relative_from_import_heads() {
        let heads = scrape_import_heads_ast("from . import util\n", "pkg.app");
        assert!(
            heads.iter().any(|h| h == "pkg.util" || h == "util"),
            "got {heads:?}"
        );
    }

    #[test]
    fn nested_fn_import_scraped() {
        let src = "async def h():\n    from pkg import util\n    util.work()\n";
        let heads = scrape_import_heads_ast(src, "pkg.app11");
        assert!(
            heads.iter().any(|h| h == "pkg.util" || h == "util"),
            "function-local import must be scraped, got {heads:?}"
        );
    }

    #[test]
    fn exact_resolve_no_parent_fallback() {
        let mut map = HashMap::new();
        map.insert("pkg.util".into(), "pkg/util.py".into());
        let root = Path::new("/tmp/proj");
        assert!(resolve_head_to_rel("pkg.util.helper", &map, root).is_none());
        assert_eq!(
            resolve_head_to_rel("pkg.util", &map, root).as_deref(),
            Some("pkg/util.py")
        );
    }

    #[test]
    fn src_layout_absent_probes_and_module_map() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let src_pkg = root.join("src/pkg");
        std::fs::create_dir_all(&src_pkg).unwrap();
        std::fs::write(root.join("pyproject.toml"), "[project]\nname=\"x\"\n").unwrap();
        std::fs::write(src_pkg.join("__init__.py"), "").unwrap();
        std::fs::write(
            src_pkg.join("svc.py"),
            "from pkg.newm import f\nasync def run():\n    f()\n",
        )
        .unwrap();
        let catalog = vec![
            DepCatalogEntry {
                abs: src_pkg.join("__init__.py"),
                rel: "src/pkg/__init__.py".into(),
                module: "src.pkg".into(),
            },
            DepCatalogEntry {
                abs: src_pkg.join("svc.py"),
                rel: "src/pkg/svc.py".into(),
                module: "src.pkg.svc".into(),
            },
        ];
        let map = build_module_to_rel(&catalog, root);
        assert_eq!(
            map.get("pkg.svc").map(String::as_str),
            Some("src/pkg/svc.py"),
            "src/ search root must register importable pkg.svc"
        );
        let roots = first_party_search_roots(root);
        assert!(roots.iter().any(|r| r.ends_with("src")));
        let probes = absent_probe_rels("pkg.newm", root, &roots);
        assert!(
            probes.iter().any(|p| p == "src/pkg/newm.py"),
            "absent probe must sit under src/, got {probes:?}"
        );
        let (direct, absents, _) = build_deps_and_fps_parallel(&catalog, &map, root);
        let svc_abs = absents.get("src/pkg/svc.py").cloned().unwrap_or_default();
        assert!(
            svc_abs.iter().any(|p| p == "src/pkg/newm.py"),
            "svc deps must probe src/pkg/newm.py, got {svc_abs:?}; direct={:?}",
            direct.get("src/pkg/svc.py")
        );
    }

    #[test]
    fn transitive_includes_impl_for_ret3() {
        let mut direct = HashMap::new();
        direct.insert("pkg/svc.py".into(), vec!["pkg/factory.py".into()]);
        direct.insert("pkg/factory.py".into(), vec!["pkg/impl.py".into()]);
        direct.insert("pkg/impl.py".into(), vec!["pkg/bases.py".into()]);
        direct.insert("pkg/bases.py".into(), vec![]);
        let mut fps = HashMap::new();
        fps.insert("pkg/svc.py".into(), "a".into());
        fps.insert("pkg/factory.py".into(), "b".into());
        fps.insert("pkg/impl.py".into(), "c".into());
        fps.insert("pkg/bases.py".into(), "d".into());
        let fp1 = transitive_deps_fp("pkg/svc.py", &direct, &fps, &[]);
        fps.insert("pkg/impl.py".into(), "c2".into());
        let fp2 = transitive_deps_fp("pkg/svc.py", &direct, &fps, &[]);
        assert_ne!(fp1, fp2, "impl content change must change svc deps_fp");
    }

    #[test]
    fn batch_deps_fp_v2_invalidates_on_cycle_member_and_absents() {
        let mut direct = HashMap::new();
        direct.insert("a.py".into(), vec!["b.py".into()]);
        direct.insert("b.py".into(), vec!["c.py".into(), "a.py".into()]); // cycle a↔b
        direct.insert("c.py".into(), vec![]);
        let mut fps = HashMap::new();
        fps.insert("a.py".into(), "A".into());
        fps.insert("b.py".into(), "B".into());
        fps.insert("c.py".into(), "C".into());
        let mut absents = HashMap::new();
        absents.insert("a.py".into(), vec!["missing.py".into()]);
        let rels = vec!["a.py".into(), "b.py".into(), "c.py".into()];
        let batch = live_deps_fps_parallel(&rels, &direct, &absents, &fps);
        fps.insert("c.py".into(), "C2".into());
        let batch2 = live_deps_fps_parallel(&rels, &direct, &absents, &fps);
        assert_ne!(
            batch.get("a.py"),
            batch2.get("a.py"),
            "c.py content change must invalidate a.py through the cycle"
        );
        absents.get_mut("a.py").unwrap().clear();
        let batch3 = live_deps_fps_parallel(&rels, &direct, &absents, &fps);
        assert_ne!(
            batch2.get("a.py"),
            batch3.get("a.py"),
            "absent-probe change must invalidate a.py"
        );
    }
}
