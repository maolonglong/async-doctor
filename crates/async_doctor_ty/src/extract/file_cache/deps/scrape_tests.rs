//! Unit tests for import scrape / deps_fp.

use std::collections::HashMap;
use std::path::Path;

use super::fingerprint::{live_deps_fps_parallel, transitive_deps_fp};
use super::roots::{
    absent_probe_rels, build_module_to_rel, first_party_search_roots, resolve_head_to_rel,
};
use super::scrape::scrape_import_heads_ast;
use super::{DepCatalogEntry, build_deps_and_fps_parallel};

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
