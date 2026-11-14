//! External tool-managed Python environment resolvers: PDM PEP 582
//! `__pypackages__`, Poetry `pypoetry/virtualenvs`, and Pipenv `virtualenvs`.

use std::path::{Path, PathBuf};

use async_doctor_core::EnvironmentKind;
use ruff_db::system::{System, SystemPath, SystemPathBuf};
use ty_site_packages::SysPrefixPathOrigin;

use super::{DiscoveredEnvironment, guess_interpreter, non_empty_env, probe_venv_prefix};

pub(super) fn probe_pdm_pypackages(
    system: &dyn System,
    anc: &SystemPath,
) -> Option<DiscoveredEnvironment> {
    let pypkg = anc.join("__pypackages__");
    let mut versions: Vec<((u32, u32), SystemPathBuf, SystemPathBuf)> = Vec::new();
    for entry in system.read_directory(&pypkg).ok()?.flatten() {
        if !entry.file_type().is_directory() {
            continue;
        }
        let ver_dir = entry.into_path();
        let lib_dir = ver_dir.join("lib");
        if let Some(name) = ver_dir.file_name()
            && system.is_directory(&lib_dir)
        {
            let ver = parse_py_major_minor(name).unwrap_or((0, 0));
            versions.push((ver, ver_dir, lib_dir));
        }
    }
    versions.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.as_str().cmp(a.1.as_str())));
    let (_, ver_dir, lib_dir) = versions.into_iter().next()?;
    let interpreter = guess_interpreter(ver_dir.as_str());
    Some(DiscoveredEnvironment {
        python_environment: None,
        kind: EnvironmentKind::Venv,
        origin: "PDM __pypackages__".into(),
        prefix: Some(ver_dir),
        interpreter,
        site_packages: vec![lib_dir],
    })
}

pub(super) fn probe_external_poetry_venv(
    system: &dyn System,
    anc: &SystemPath,
) -> Option<DiscoveredEnvironment> {
    let pyproject = anc.join("pyproject.toml");
    let text = system
        .is_file(&pyproject)
        .then(|| std::fs::read_to_string(pyproject.as_std_path()).ok())
        .flatten();
    let has_poetry = system.is_file(&anc.join("poetry.lock"))
        || text
            .as_deref()
            .is_some_and(|t| t.contains("[tool.poetry]") || t.contains("poetry.core"));
    if !has_poetry {
        return None;
    }
    let raw_name = text
        .as_deref()
        .and_then(extract_poetry_project_name)
        .or_else(|| anc.file_name().map(str::to_string))?;
    let sanitized = sanitize_venv_project_name(&raw_name, true);
    let hashes = candidate_path_hashes(anc.as_std_path(), None);

    for store in poetry_venv_stores(system) {
        let Ok(entries) = std::fs::read_dir(&store) else {
            continue;
        };
        let mut matches: Vec<((u32, u32), PathBuf)> = Vec::new();
        for p in entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
            let Some(fname) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            for h in &hashes {
                if let Some(ver_str) = fname.strip_prefix(&format!("{sanitized}-{h}-py")) {
                    matches.push((parse_py_major_minor(ver_str).unwrap_or((0, 0)), p.clone()));
                }
            }
        }
        matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        for (_, matched_dir) in matches {
            if let Ok(sys_dir) = SystemPathBuf::from_path_buf(matched_dir)
                && let Some(mut found) = probe_venv_prefix(
                    system,
                    &sys_dir,
                    SysPrefixPathOrigin::LocalVenv,
                    "poetry virtualenv",
                )
            {
                found.origin = "poetry virtualenv".into();
                return Some(found);
            }
        }
    }
    None
}

pub(super) fn probe_external_pipenv_venv(
    system: &dyn System,
    anc: &SystemPath,
) -> Option<DiscoveredEnvironment> {
    if !system.is_file(&anc.join("Pipfile")) {
        return None;
    }
    let sanitized = sanitize_venv_project_name(anc.file_name()?, false);
    let hashes = candidate_path_hashes(anc.as_std_path(), Some("Pipfile"));
    for store in pipenv_venv_stores(system) {
        for h in &hashes {
            let candidate = store.join(format!("{sanitized}-{h}"));
            if candidate.is_dir()
                && let Ok(sys_dir) = SystemPathBuf::from_path_buf(candidate)
                && let Some(mut found) = probe_venv_prefix(
                    system,
                    &sys_dir,
                    SysPrefixPathOrigin::LocalVenv,
                    "pipenv virtualenv",
                )
            {
                found.origin = "pipenv virtualenv".into();
                return Some(found);
            }
        }
    }
    None
}

fn poetry_venv_stores(system: &dyn System) -> Vec<PathBuf> {
    let mut stores = Vec::new();
    let mut push_unique = |p: PathBuf| {
        if !stores.contains(&p) {
            stores.push(p);
        }
    };
    if let Some(p) = non_empty_env(system, "POETRY_VIRTUALENVS_PATH") {
        push_unique(PathBuf::from(p));
    }
    if let Some(xdg) = non_empty_env(system, "XDG_CACHE_HOME") {
        push_unique(PathBuf::from(xdg).join("pypoetry/virtualenvs"));
    }
    if let Some(home) = non_empty_env(system, "HOME") {
        let home = PathBuf::from(home);
        push_unique(home.join("Library/Caches/pypoetry/virtualenvs"));
        push_unique(home.join(".cache/pypoetry/virtualenvs"));
    }
    stores
}

fn pipenv_venv_stores(system: &dyn System) -> Vec<PathBuf> {
    let mut stores = Vec::new();
    if let Some(w) = non_empty_env(system, "WORKON_HOME") {
        stores.push(PathBuf::from(w));
    }
    if let Some(xdg_data) = non_empty_env(system, "XDG_DATA_HOME") {
        stores.push(PathBuf::from(xdg_data).join("virtualenvs"));
    } else if let Some(home) = non_empty_env(system, "HOME") {
        stores.push(PathBuf::from(home).join(".local/share/virtualenvs"));
    }
    stores
}

fn parse_py_major_minor(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.trim_end_matches('t').split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn extract_poetry_project_name(pyproject_toml: &str) -> Option<String> {
    let parsed: toml::Value = toml::from_str(pyproject_toml).ok()?;
    parsed
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        .or_else(|| parsed.get("project")?.get("name")?.as_str())
        .map(str::to_string)
}

fn sanitize_venv_project_name(name: &str, lowercase: bool) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_underscore = false;
    for ch in name.chars() {
        let sep = matches!(
            ch,
            ' ' | '$' | '`' | '!' | '*' | '@' | '"' | '\\' | '/' | '\r' | '\n' | '\t'
        );
        if sep && !prev_underscore {
            out.push('_');
        } else if !sep && lowercase {
            out.extend(ch.to_lowercase());
        } else if !sep {
            out.push(ch);
        }
        prev_underscore = sep;
    }
    out.chars().take(42).collect()
}

fn candidate_path_hashes(dir: &Path, child_file: Option<&str>) -> Vec<String> {
    let mut hashes = Vec::new();
    let canon = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    for base in [canon, dir.to_path_buf()] {
        let target = child_file.map_or(base.clone(), |f| base.join(f));
        let h = sha256_base64url8(target.to_string_lossy().as_bytes());
        if !hashes.contains(&h) {
            hashes.push(h);
        }
    }
    hashes
}

/// First 8 URL-safe Base64 characters of `SHA-256(data)` (used by Poetry & Pipenv).
#[rustfmt::skip]
pub(crate) fn sha256_base64url8(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut st: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let padded_len = (data.len() + 9).div_ceil(64) * 64;
    let mut msg = vec![0u8; padded_len];
    msg[..data.len()].copy_from_slice(data);
    msg[data.len()] = 0x80;
    msg[padded_len - 8..].copy_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(word.try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = st;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let t1 = h.wrapping_add(s1).wrapping_add((e & f) ^ ((!e) & g)).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            (h, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(s0.wrapping_add(maj)));
        }
        for (slot, add) in st.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(add);
        }
    }
    let (b0, b1) = (st[0].to_be_bytes(), st[1].to_be_bytes());
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(8);
    for t in [[b0[0], b0[1], b0[2]], [b0[3], b1[0], b1[1]]] {
        let n = ((t[0] as u32) << 16) | ((t[1] as u32) << 8) | (t[2] as u32);
        for shift in [18, 12, 6, 0] {
            out.push(B64[((n >> shift) & 0x3f) as usize] as char);
        }
    }
    out
}
