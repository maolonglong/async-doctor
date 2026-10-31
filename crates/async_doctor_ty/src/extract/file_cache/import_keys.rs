//! First-party import fingerprint keys for per-file cache invalidation.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// Sentinel fingerprint for an unresolved first-party import head.
///
/// Stored so a later resolution (new file appears) invalidates the slot.
pub const UNRESOLVED_IMPORT_FP: &str = "?";

/// Map first-party import heads from `source` onto content fingerprints.
///
/// Keys are **canonical scanned module names** (`catalog.module`). `from X import Y`
/// records submodule `Y` / `X.Y` when those exist in `module_fps`, not only `X`.
/// Unresolved first-party-looking imports are recorded as [`UNRESOLVED_IMPORT_FP`].
///
/// `canon_modules` is the set of bare scanned module names (no package-root aliases).
pub fn import_fps_for_source(
    source: &str,
    file_module: &str,
    module_fps: &BTreeMap<String, String>,
    canon_modules: &HashSet<String>,
    scan_root: &Path,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for head in scrape_import_modules(source, file_module) {
        if let Some((alias_key, fp)) = resolve_import_head(module_fps, &head) {
            let canon = canonicalize_module_name(&alias_key, canon_modules, scan_root)
                .unwrap_or(alias_key);
            out.insert(canon, fp);
        } else if looks_first_party(&head, module_fps) {
            let canon =
                canonicalize_module_name(&head, canon_modules, scan_root).unwrap_or(head);
            out.insert(canon, UNRESOLVED_IMPORT_FP.to_string());
        }
    }
    out
}

/// True when every recorded import fingerprint still matches `module_fps`.
pub fn import_fps_still_valid(
    cached: &BTreeMap<String, String>,
    module_fps: &BTreeMap<String, String>,
) -> bool {
    cached.iter().all(|(mod_name, fp)| match resolve_import_head(module_fps, mod_name) {
        Some((_, cur)) => &cur == fp,
        None => fp == UNRESOLVED_IMPORT_FP,
    })
}

/// Expand direct import fingerprints to the first-party import closure.
///
/// `direct_deps` maps each scanned module to the scanned modules it imports
/// (already canonicalized). Missing deps are skipped (deleted modules are
/// caught by [`import_fps_still_valid`] on the recorded keys).
pub fn expand_transitive_import_fps(
    direct: &BTreeMap<String, String>,
    direct_deps: &BTreeMap<String, Vec<String>>,
    module_fps: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = direct.clone();
    let mut stack: Vec<String> = out.keys().cloned().collect();
    while let Some(m) = stack.pop() {
        let Some(deps) = direct_deps.get(&m) else {
            continue;
        };
        for d in deps {
            if out.contains_key(d) {
                continue;
            }
            if let Some(fp) = module_fps.get(d) {
                out.insert(d.clone(), fp.clone());
                stack.push(d.clone());
            } else if !out.contains_key(d) {
                // Keep unresolved transitive heads visible for invalidation.
                out.insert(d.clone(), UNRESOLVED_IMPORT_FP.to_string());
            }
        }
    }
    out
}

/// Scrape direct first-party import deps for `source`, canonicalized to scanned
/// module names when possible.
pub fn direct_import_deps(
    source: &str,
    file_module: &str,
    module_fps: &BTreeMap<String, String>,
    canon_modules: &HashSet<String>,
    scan_root: &Path,
) -> Vec<String> {
    let mut out = Vec::new();
    for head in scrape_import_modules(source, file_module) {
        if let Some((alias_key, _)) = resolve_import_head(module_fps, &head) {
            out.push(
                canonicalize_module_name(&alias_key, canon_modules, scan_root)
                    .unwrap_or(alias_key),
            );
        } else if looks_first_party(&head, module_fps) {
            out.push(canonicalize_module_name(&head, canon_modules, scan_root).unwrap_or(head));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Map an import head / alias onto the bare scanned module name.
pub fn canonicalize_module_name(
    name: &str,
    canon_modules: &HashSet<String>,
    scan_root: &Path,
) -> Option<String> {
    if canon_modules.contains(name) {
        return Some(name.to_string());
    }
    if scan_root.join("__init__.py").is_file()
        && let Some(pkg) = scan_root.file_name().and_then(|n| n.to_str())
    {
        if name == pkg && canon_modules.contains("") {
            return Some(String::new());
        }
        if let Some(rest) = name.strip_prefix(&format!("{pkg}."))
            && canon_modules.contains(rest)
        {
            return Some(rest.to_string());
        }
    }
    // Parent walk: pkg.util.helper → pkg.util → util (via alias strip)
    let mut cur = name;
    while let Some((parent, _)) = cur.rsplit_once('.') {
        if canon_modules.contains(parent) {
            return Some(parent.to_string());
        }
        if scan_root.join("__init__.py").is_file()
            && let Some(pkg) = scan_root.file_name().and_then(|n| n.to_str())
            && let Some(rest) = parent.strip_prefix(&format!("{pkg}."))
            && canon_modules.contains(rest)
        {
            return Some(rest.to_string());
        }
        cur = parent;
    }
    None
}

/// Register `module → fp` under the bare name and scan-root package aliases.
///
/// When the scan root is itself a package directory named `pkg`, modules are
/// recorded as `util` but imports say `pkg.util` — both keys share the same fp.
pub fn insert_module_fp_aliases(
    module_fps: &mut BTreeMap<String, String>,
    scan_root: &Path,
    module: &str,
    fp: String,
) {
    for alias in module_name_aliases(scan_root, module) {
        module_fps.insert(alias, fp.clone());
    }
}

/// All names under which a scanned module should be keyed.
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

fn resolve_import_head(
    module_fps: &BTreeMap<String, String>,
    name: &str,
) -> Option<(String, String)> {
    if let Some(fp) = module_fps.get(name) {
        return Some((name.to_string(), fp.clone()));
    }
    // Walk parents so `pkg.util.helper` can hit `pkg.util` when only the
    // package file exists — but prefer the longest match already tried above.
    let mut cur = name;
    while let Some((parent, _)) = cur.rsplit_once('.') {
        if let Some(fp) = module_fps.get(parent) {
            return Some((parent.to_string(), fp.clone()));
        }
        cur = parent;
    }
    None
}

fn looks_first_party(name: &str, module_fps: &BTreeMap<String, String>) -> bool {
    if name.is_empty() || name == UNRESOLVED_IMPORT_FP {
        return false;
    }
    // Relative-resolved names and anything that shares a prefix with a known
    // first-party module are treated as first-party for invalidation.
    if module_fps.contains_key(name) {
        return true;
    }
    for known in module_fps.keys() {
        if known.is_empty() {
            continue;
        }
        if name == known.as_str()
            || name.starts_with(&format!("{known}."))
            || known.starts_with(&format!("{name}."))
        {
            return true;
        }
    }
    false
}

/// Import module heads scraped from source (unsorted unique list).
///
/// - `import a.b` → `a.b`
/// - `from pkg import util` → `pkg`, `pkg.util`, `util`
/// - `from . import util` → resolved parent + `util`, and `util`
/// - `from .util import work` → resolved `….util` (and imported names as submodules)
pub fn scrape_import_modules(source: &str, file_module: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in source.lines() {
        let line = strip_line_comment(raw.trim());
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("import ") {
            for part in rest.split(',') {
                let full = import_module_token(part);
                if !full.is_empty() {
                    out.push(full);
                }
            }
        } else if let Some(rest) = line.strip_prefix("from ") {
            let Some((mod_part, imports_part)) = split_from_import(rest) else {
                continue;
            };
            let base = if mod_part.starts_with('.') {
                resolve_relative_import(file_module, mod_part)
            } else if !mod_part.is_empty() {
                Some(mod_part.to_string())
            } else {
                None
            };
            if let Some(ref b) = base {
                out.push(b.clone());
            }
            for name in parse_import_names(imports_part) {
                if name == "*" {
                    continue;
                }
                // Scan-root-relative bare name (`util` when scan root is `pkg/`).
                out.push(name.clone());
                if let Some(ref b) = base {
                    if b.is_empty() {
                        out.push(name);
                    } else {
                        out.push(format!("{b}.{name}"));
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn strip_line_comment(line: &str) -> &str {
    // Naive but matches the prior scraper: ignore full-line comments only.
    if line.starts_with('#') {
        ""
    } else {
        line
    }
}

fn import_module_token(part: &str) -> String {
    let mut bits = part.split_whitespace();
    let Some(first) = bits.next() else {
        return String::new();
    };
    // `import a.b as c` → a.b
    first.to_string()
}

fn split_from_import(rest: &str) -> Option<(&str, &str)> {
    // `pkg.util import work, other`
    let mut bits = rest.split_whitespace();
    let mod_part = bits.next()?;
    if bits.next()? != "import" {
        return None;
    }
    let start = rest.find(" import ")? + " import ".len();
    Some((mod_part, rest[start..].trim()))
}

fn parse_import_names(imports_part: &str) -> Vec<String> {
    let mut names = Vec::new();
    for part in imports_part.split(',') {
        let tok = part
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches(|c| c == '(' || c == ')');
        if !tok.is_empty() {
            names.push(tok.to_string());
        }
    }
    names
}

fn resolve_relative_import(file_module: &str, rel: &str) -> Option<String> {
    let dots = rel.chars().take_while(|c| *c == '.').count();
    let rest = &rel[dots..];
    let mut parts: Vec<&str> = file_module.split('.').filter(|s| !s.is_empty()).collect();
    if !file_module.is_empty() {
        let _ = parts.pop();
    }
    if dots > 1 {
        let up = dots - 1;
        if up > parts.len() {
            return None;
        }
        parts.truncate(parts.len() - up);
    }
    if !rest.is_empty() {
        parts.extend(rest.split('.').filter(|s| !s.is_empty()));
    }
    if parts.is_empty() {
        Some(String::new())
    } else {
        Some(parts.join("."))
    }
}

#[cfg(test)]
mod scrape_tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn from_pkg_import_util_keys_submodule_not_only_pkg() {
        let mut fps = BTreeMap::new();
        fps.insert("pkg".into(), "fp_pkg".into());
        fps.insert("pkg.util".into(), "fp_util".into());
        fps.insert("util".into(), "fp_util".into());
        let canon: HashSet<String> = ["pkg".into(), "pkg.util".into()].into_iter().collect();
        let root = Path::new("/tmp/proj");
        let got = import_fps_for_source("from pkg import util\n", "app", &fps, &canon, root);
        assert_eq!(got.get("pkg.util").map(String::as_str), Some("fp_util"));
        assert!(
            got.contains_key("pkg.util"),
            "must key the imported submodule, got {got:?}"
        );
    }

    #[test]
    fn relative_from_import_keys_on_y() {
        let mut fps = BTreeMap::new();
        fps.insert("pkg.util".into(), "fp_util".into());
        fps.insert("util".into(), "fp_util".into());
        let canon: HashSet<String> = ["pkg.util".into(), "pkg.app".into()].into_iter().collect();
        let root = Path::new("/tmp/proj");
        let got = import_fps_for_source("from . import util\n", "pkg.app", &fps, &canon, root);
        assert!(
            got.contains_key("pkg.util"),
            "from . import util must key util, got {got:?}"
        );
    }

    #[test]
    fn package_root_alias_resolves_pkg_util_to_util() {
        let dir = std::env::temp_dir().join(format!(
            "async-doctor-pkg-alias-{}",
            std::process::id()
        ));
        let root = dir.join("pkg");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("__init__.py"), "").unwrap();
        let mut fps = BTreeMap::new();
        fps.insert("util".into(), "fp".into());
        fps.insert("pkg.util".into(), "fp".into());
        let canon: HashSet<String> = ["util".into(), "other".into()].into_iter().collect();
        let got = import_fps_for_source("from pkg.util import work\n", "other", &fps, &canon, &root);
        assert_eq!(
            got.get("util").map(String::as_str),
            Some("fp"),
            "pkg.util import must canonicalize to util, got {got:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
