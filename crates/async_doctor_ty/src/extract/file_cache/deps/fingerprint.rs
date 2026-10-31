//! Content fingerprints and transitive / SCC `deps_fp` computation.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use rayon::prelude::*;

use super::super::short_hash;
use super::roots::{first_party_search_roots, importable_module, search_roots_for_file};
use super::scrape::direct_deps_for_source;
use super::{ABSENT_DEP_FP, DepCatalogEntry};

/// `(direct_deps, absent_probes, content_fps)` keyed by relative path.
type DepsAndFps = (
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
    let project_roots = first_party_search_roots(scan_root);
    let pairs: Vec<(String, Vec<String>, Vec<String>, String)> = catalog
        .par_iter()
        .filter_map(|e| {
            let bytes = std::fs::read(&e.abs).ok()?;
            let fp = short_hash(&bytes);
            let source = String::from_utf8_lossy(&bytes);
            // File-parent first, then project/src/extras — mirrors ty resolution.
            let search_roots = search_roots_for_file(scan_root, &e.abs);
            let file_module = {
                let m = importable_module(&e.abs, &project_roots);
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
