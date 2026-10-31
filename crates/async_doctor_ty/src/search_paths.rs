//! Zero-config discovery of extra module search paths.
//!
//! Nested first-party packages (`pkg/pkg/`) are often only importable via IDE
//! hints (`extraPaths`, ruff `src`) or setuptools editable finders. Ty already
//! honors path-style `.pth` editables under site-packages; finder-based
//! installs (`__editable__*_finder.py` + `MAPPING`) and config `extraPaths`
//! need this module.
//!
//! Discovered roots are appended *after* existing `src_roots` (never as
//! front-loaded `extra_paths`) and duplicates of the project root / already
//! present roots are skipped — e.g. `[tool.ruff] src = ["."]` must not shadow
//! file-relative resolution.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use ruff_db::system::{SystemPath, SystemPathBuf};

/// Collect additional search roots for `project_root`.
///
/// Callers must append the result after existing `src_roots`. Paths equal to
/// `project_root` or any entry in `existing_roots` (by string or canonicalize)
/// are omitted.
pub(crate) fn discover_extra_paths(
    project_root: &SystemPath,
    site_packages: &[SystemPathBuf],
    existing_roots: &[SystemPathBuf],
) -> Vec<SystemPathBuf> {
    let root = PathBuf::from(project_root.as_str());
    let mut out: BTreeSet<PathBuf> = BTreeSet::new();

    collect_from_pyrightconfig(&root, &mut out);
    collect_from_pyproject(&root, &mut out);
    for sp in site_packages {
        collect_from_editable_finders(Path::new(sp.as_str()), &root, &mut out);
    }

    let mut skip: BTreeSet<PathBuf> = existing_roots
        .iter()
        .map(|p| PathBuf::from(p.as_str()))
        .collect();
    skip.insert(root.clone());
    if let Ok(canon) = root.canonicalize() {
        skip.insert(canon);
    }
    for p in existing_roots {
        if let Ok(c) = PathBuf::from(p.as_str()).canonicalize() {
            skip.insert(c);
        }
    }

    out.into_iter()
        .filter(|p| p.is_dir())
        .filter(|p| !should_skip(p, &skip))
        .filter_map(|p| SystemPathBuf::from_path_buf(p).ok())
        .collect()
}

fn should_skip(path: &Path, skip: &BTreeSet<PathBuf>) -> bool {
    if skip.contains(path) {
        return true;
    }
    if let Ok(c) = path.canonicalize() {
        return skip.contains(&c);
    }
    false
}

fn collect_from_pyrightconfig(project_root: &Path, out: &mut BTreeSet<PathBuf>) {
    let json_path = project_root.join("pyrightconfig.json");
    if let Ok(text) = fs::read_to_string(&json_path)
        && let Ok(val) = serde_json::from_str::<serde_json::Value>(&text)
    {
        push_string_list(project_root, val.get("extraPaths"), out);
        push_string_list(project_root, val.get("extra_paths"), out);
    }

    let toml_path = project_root.join("pyrightconfig.toml");
    if let Ok(text) = fs::read_to_string(&toml_path)
        && let Ok(table) = text.parse::<toml::Table>()
    {
        push_toml_string_list(project_root, table.get("extraPaths"), out);
        push_toml_string_list(project_root, table.get("extra_paths"), out);
    }
}

fn collect_from_pyproject(project_root: &Path, out: &mut BTreeSet<PathBuf>) {
    let path = project_root.join("pyproject.toml");
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    let Ok(table) = text.parse::<toml::Table>() else {
        return;
    };
    let Some(tool) = table.get("tool").and_then(|v| v.as_table()) else {
        return;
    };

    if let Some(pyright) = tool.get("pyright").and_then(|v| v.as_table()) {
        push_toml_string_list(project_root, pyright.get("extraPaths"), out);
        push_toml_string_list(project_root, pyright.get("extra_paths"), out);
    }

    if let Some(ty) = tool.get("ty").and_then(|v| v.as_table())
        && let Some(env) = ty.get("environment").and_then(|v| v.as_table())
    {
        push_toml_string_list(project_root, env.get("extra-paths"), out);
        push_toml_string_list(project_root, env.get("extraPaths"), out);
    }

    if let Some(ruff) = tool.get("ruff").and_then(|v| v.as_table()) {
        push_toml_string_list(project_root, ruff.get("src"), out);
    }
}

fn collect_from_editable_finders(
    site_packages: &Path,
    project_root: &Path,
    out: &mut BTreeSet<PathBuf>,
) {
    let Ok(entries) = fs::read_dir(site_packages) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("__editable__") && name.ends_with("_finder.py")) {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        for (_pkg, mapped) in parse_editable_mapping(&text) {
            let mapped_path = resolve_path(project_root, &mapped);
            // MAPPING values point at the package directory itself. ty search
            // paths need the parent so `import pkg` finds `parent/pkg`.
            if let Some(parent) = mapped_path.parent() {
                out.insert(parent.to_path_buf());
            }
        }
    }
}

fn push_string_list(
    project_root: &Path,
    value: Option<&serde_json::Value>,
    out: &mut BTreeSet<PathBuf>,
) {
    let Some(serde_json::Value::Array(items)) = value else {
        return;
    };
    for item in items {
        if let Some(s) = item.as_str() {
            out.insert(resolve_path(project_root, s));
        }
    }
}

fn push_toml_string_list(
    project_root: &Path,
    value: Option<&toml::Value>,
    out: &mut BTreeSet<PathBuf>,
) {
    let Some(toml::Value::Array(items)) = value else {
        return;
    };
    for item in items {
        if let Some(s) = item.as_str() {
            out.insert(resolve_path(project_root, s));
        }
    }
}

fn resolve_path(project_root: &Path, raw: &str) -> PathBuf {
    let p = PathBuf::from(raw);
    if p.is_absolute() {
        p
    } else {
        project_root.join(p)
    }
}

/// Parse `MAPPING = {'pkg': '/abs/pkg'}` (or double quotes / multiline) from a
/// setuptools editable finder module. Values may be absolute or project-relative.
fn parse_editable_mapping(source: &str) -> Vec<(String, String)> {
    let Some(rest) = mapping_dict_body(source) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r' | b',') {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] == b'}' {
            break;
        }
        let Some((key, next)) = parse_py_string(bytes, i) else {
            break;
        };
        i = next;
        while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b':' {
            break;
        }
        i += 1;
        while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        let Some((val, next)) = parse_py_string(bytes, i) else {
            break;
        };
        i = next;
        out.push((key, val));
    }
    out
}

fn mapping_dict_body(source: &str) -> Option<&str> {
    let bytes = source.as_bytes();
    let needle = b"MAPPING";
    let mut search = 0;
    while search + needle.len() <= bytes.len() {
        if &bytes[search..search + needle.len()] == needle {
            let mut j = search + needle.len();
            while j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b':' {
                while j < bytes.len() && bytes[j] != b'=' {
                    j += 1;
                }
            }
            if j < bytes.len() && bytes[j] == b'=' {
                j += 1;
                while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'{' {
                    return Some(&source[j + 1..]);
                }
            }
        }
        search += 1;
    }
    None
}

fn parse_py_string(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    if start >= bytes.len() {
        return None;
    }
    let mut i = start;
    // Optional prefixes: r/u/b/f and combinations (setuptools may emit r'…').
    while i < bytes.len()
        && matches!(
            bytes[i],
            b'r' | b'R' | b'u' | b'U' | b'b' | b'B' | b'f' | b'F'
        )
    {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let quote = bytes[i];
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    i += 1;
    let mut out = String::new();
    while i < bytes.len() {
        let c = bytes[i];
        if c == quote {
            return Some((out, i + 1));
        }
        if c == b'\\' && i + 1 < bytes.len() {
            out.push(bytes[i + 1] as char);
            i += 2;
            continue;
        }
        out.push(c as char);
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parse_mapping_single_line() {
        let src = "MAPPING = {'libcore': '/tmp/ed/libcore/libcore'}\nNAMESPACES = {}\n";
        let got = parse_editable_mapping(src);
        assert_eq!(
            got,
            vec![("libcore".into(), "/tmp/ed/libcore/libcore".into())]
        );
    }

    #[test]
    fn parse_mapping_raw_string() {
        let src = "MAPPING = {'libcore': r'/tmp/ed/libcore'}\n";
        let got = parse_editable_mapping(src);
        assert_eq!(got, vec![("libcore".into(), "/tmp/ed/libcore".into())]);
    }

    #[test]
    fn parse_mapping_multiline_double_quotes() {
        let src = r#"
MAPPING = {
    "a": "/x/a",
    "b": "/y/b",
}
"#;
        let got = parse_editable_mapping(src);
        assert_eq!(
            got,
            vec![("a".into(), "/x/a".into()), ("b".into(), "/y/b".into())]
        );
    }

    #[test]
    fn discover_skips_project_root_dot() {
        let dir = std::env::temp_dir().join(format!(
            "async-doctor-search-dot-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("pyproject.toml"),
            r#"
[tool.ruff]
src = ["."]
"#,
        )
        .unwrap();
        let root = SystemPathBuf::from_path_buf(dir.clone()).unwrap();
        let paths = discover_extra_paths(&root, &[], std::slice::from_ref(&root));
        assert!(
            paths.is_empty(),
            "src=['.'] must not re-add project root: {paths:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_reads_pyright_ruff_and_finder() {
        let dir = std::env::temp_dir().join(format!(
            "async-doctor-search-paths-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("libcore/libcore")).unwrap();
        fs::create_dir_all(dir.join("other")).unwrap();
        fs::create_dir_all(dir.join("ruffsrc")).unwrap();
        fs::create_dir_all(dir.join(".venv/lib/python3.12/site-packages")).unwrap();

        fs::write(
            dir.join("pyrightconfig.json"),
            r#"{"extraPaths": ["libcore"]}"#,
        )
        .unwrap();
        fs::write(
            dir.join("pyproject.toml"),
            r#"
[tool.ruff]
src = ["ruffsrc"]

[tool.ty.environment]
extra-paths = ["other"]
"#,
        )
        .unwrap();

        let mapped = dir.join("libcore/libcore");
        fs::write(
            dir.join(".venv/lib/python3.12/site-packages/__editable___libcore_0_1_0_finder.py"),
            format!("MAPPING = {{'libcore': '{}'}}\n", mapped.display()),
        )
        .unwrap();

        let root = SystemPathBuf::from_path_buf(dir.clone()).unwrap();
        let site =
            SystemPathBuf::from_path_buf(dir.join(".venv/lib/python3.12/site-packages")).unwrap();
        let paths = discover_extra_paths(&root, &[site], std::slice::from_ref(&root));
        let as_str: Vec<_> = paths.iter().map(|p| p.as_str().to_string()).collect();
        assert!(
            as_str.iter().any(|p| p.ends_with("libcore")),
            "expected libcore from pyright/finder: {as_str:?}"
        );
        assert!(
            as_str.iter().any(|p| p.ends_with("ruffsrc")),
            "expected ruff src: {as_str:?}"
        );
        assert!(
            as_str.iter().any(|p| p.ends_with("other")),
            "expected ty extra-paths: {as_str:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
