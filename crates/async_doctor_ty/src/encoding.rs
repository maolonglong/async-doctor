//! Decode Python source bytes per PEP 263 (coding cookie) and Unicode BOM.
//!
//! ruff_db's `OsSystem::read_to_string` requires UTF-8. Non-UTF-8 sources with a
//! valid coding declaration must still be analysed; undecodable sources are
//! reported to the caller as errors so extract can put them in `skipped`.

use std::borrow::Cow;

/// Decode owned `bytes` as a Python source file, reusing the `Vec<u8>` buffer
/// directly without copying on the common UTF-8 (no BOM, no non-UTF-8 cookie) path.
pub fn decode_python_source_owned(bytes: Vec<u8>) -> Result<String, String> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF])
        || bytes.starts_with(&[0xFF, 0xFE])
        || bytes.starts_with(&[0xFE, 0xFF])
    {
        return decode_python_source(&bytes);
    }
    match coding_cookie(&bytes) {
        None => {
            String::from_utf8(bytes).map_err(|e| format!("not valid UTF-8 ({})", e.utf8_error()))
        }
        Some(enc) if is_utf8_family(&enc) => {
            String::from_utf8(bytes).map_err(|e| format!("not valid UTF-8 ({})", e.utf8_error()))
        }
        Some(enc) => match decode_named(&bytes, &enc) {
            Ok(s) => Ok(s),
            Err(err) => String::from_utf8(bytes).map_err(|_| err),
        },
    }
}

/// Decode `bytes` as a Python source file.
///
/// Order: UTF-8/UTF-16 BOM → coding cookie on line 1–2 → UTF-8 default.
/// Unknown/unsupported cookies fall back to UTF-8 when the bytes are valid UTF-8.
pub fn decode_python_source(bytes: &[u8]) -> Result<String, String> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return decode_named(rest, "utf-8");
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(rest, false);
    }

    let encoding = coding_cookie(bytes).unwrap_or_else(|| "utf-8".to_string());
    match decode_named(bytes, &encoding) {
        Ok(s) => Ok(s),
        Err(err) => {
            // e.g. `# coding: gbk` on an ASCII/UTF-8 file, or an unknown alias.
            if !is_utf8_family(&encoding)
                && let Ok(s) = std::str::from_utf8(bytes)
            {
                return Ok(s.to_string());
            }
            Err(err)
        }
    }
}

fn is_utf8_family(encoding: &str) -> bool {
    matches!(
        normalize_encoding_name(encoding).as_ref(),
        "utf-8" | "utf8" | "u8" | "utf-8-sig"
    )
}

fn coding_cookie(bytes: &[u8]) -> Option<String> {
    // PEP 263: cookie only on the first or second line, and only inside a `#` comment.
    let prefix = &bytes[..bytes.len().min(512)];
    let mut rest = prefix;
    for _ in 0..2 {
        if rest.is_empty() {
            break;
        }
        let (line, tail) = match rest.iter().position(|&b| b == b'\n') {
            Some(pos) => (&rest[..pos], &rest[pos + 1..]),
            None => (rest, &[][..]),
        };
        if let Some(enc) = cookie_on_bytes(line) {
            return Some(enc);
        }
        rest = tail;
    }
    None
}

fn cookie_on_bytes(line: &[u8]) -> Option<String> {
    let mut start = 0;
    while start < line.len() && matches!(line[start], b' ' | b'\t' | 0x0c) {
        start += 1;
    }
    if line.get(start) != Some(&b'#') {
        return None;
    }
    let text = String::from_utf8_lossy(&line[start..]);
    cookie_on_line(&text)
}

fn cookie_on_line(line: &str) -> Option<String> {
    // PEP 263: ^[ \t\f]*#.*?coding[:=][ \t]*([-\w.]+)
    let rest = line.trim_start_matches([' ', '\t', '\x0c']);
    if !rest.starts_with('#') {
        return None;
    }
    let lower = rest.as_bytes();
    let needle = b"coding";
    let mut i = 0;
    while i + needle.len() <= lower.len() {
        if lower[i..i + needle.len()].eq_ignore_ascii_case(needle) {
            let after = &rest[i + needle.len()..];
            let after = after.trim_start();
            let after = after
                .strip_prefix(':')
                .or_else(|| after.strip_prefix('='))?;
            let after = after.trim_start_matches([' ', '\t']);
            let end = after
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_' && c != '.')
                .unwrap_or(after.len());
            let name = after.get(..end)?.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
            return None;
        }
        i += 1;
    }
    None
}

fn decode_named(bytes: &[u8], encoding: &str) -> Result<String, String> {
    let enc = normalize_encoding_name(encoding);
    match enc.as_ref() {
        "utf-8" | "utf8" | "u8" | "utf-8-sig" => match std::str::from_utf8(bytes) {
            Ok(s) => Ok(s.to_string()),
            Err(e) => Err(format!("not valid UTF-8 ({e})")),
        },
        "latin-1" | "latin1" | "iso-8859-1" | "iso8859-1" | "8859" => Ok(decode_latin1(bytes)),
        "ascii" | "us-ascii" => {
            if bytes.iter().all(|b| *b < 128) {
                Ok(decode_latin1(bytes))
            } else {
                Err("not valid ASCII".to_string())
            }
        }
        "cp1252" | "windows-1252" => Ok(decode_cp1252(bytes)),
        other => Err(format!("unsupported source encoding `{other}`")),
    }
}

fn normalize_encoding_name(name: &str) -> Cow<'_, str> {
    let s = name.trim().to_ascii_lowercase().replace('_', "-");
    match s.as_str() {
        "iso8859-1" => Cow::Borrowed("iso-8859-1"),
        "utf8" => Cow::Borrowed("utf-8"),
        "utf8-sig" => Cow::Borrowed("utf-8-sig"),
        _ => Cow::Owned(s),
    }
}

fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

fn decode_cp1252(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b {
            0x80 => '\u{20AC}',
            0x82 => '\u{201A}',
            0x83 => '\u{0192}',
            0x84 => '\u{201E}',
            0x85 => '\u{2026}',
            0x86 => '\u{2020}',
            0x87 => '\u{2021}',
            0x88 => '\u{02C6}',
            0x89 => '\u{2030}',
            0x8A => '\u{0160}',
            0x8B => '\u{2039}',
            0x8C => '\u{0152}',
            0x8E => '\u{017D}',
            0x91 => '\u{2018}',
            0x92 => '\u{2019}',
            0x93 => '\u{201C}',
            0x94 => '\u{201D}',
            0x95 => '\u{2022}',
            0x96 => '\u{2013}',
            0x97 => '\u{2014}',
            0x98 => '\u{02DC}',
            0x99 => '\u{2122}',
            0x9A => '\u{0161}',
            0x9B => '\u{203A}',
            0x9C => '\u{0153}',
            0x9E => '\u{017E}',
            0x9F => '\u{0178}',
            0x81 | 0x8D | 0x8F | 0x90 | 0x9D => '\u{FFFD}',
            other => other as char,
        })
        .collect()
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> Result<String, String> {
    if !bytes.len().is_multiple_of(2) {
        return Err("truncated UTF-16 source".to_string());
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| {
            if little_endian {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        })
        .collect();
    String::from_utf16(&units).map_err(|_| "invalid UTF-16 source".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_cookie_decodes() {
        let mut bytes = b"# -*- coding: latin-1 -*-\n".to_vec();
        bytes.extend_from_slice(b"s = \"");
        bytes.push(0xE9);
        bytes.extend_from_slice(b"\"\n");
        let text = decode_python_source(&bytes).unwrap();
        assert!(text.contains('é'), "{text}");
    }

    #[test]
    fn utf8_bom_stripped() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"x = 1\n");
        assert_eq!(decode_python_source(&bytes).unwrap(), "x = 1\n");
    }

    #[test]
    fn default_utf8_rejects_latin1_bytes() {
        let bytes = b"s = \"\xe9\"\n";
        assert!(decode_python_source(bytes).is_err());
    }

    #[test]
    fn encoding_param_name_is_not_a_cookie() {
        // Regression: `def to_string(s, encoding: str = "utf-8"):` on line 1.
        let src = "def to_string(s, encoding: str = \"utf-8\"):\n    return s\n";
        assert!(cookie_on_line(src.lines().next().unwrap()).is_none());
        assert!(decode_python_source(src.as_bytes()).is_ok());
    }

    #[test]
    fn utf8_sig_cookie_accepted() {
        let src = "# coding: utf-8-sig\nx = 1\n";
        assert_eq!(decode_python_source(src.as_bytes()).unwrap(), src);
    }

    #[test]
    fn unknown_cookie_falls_back_to_utf8() {
        let src = "# coding: gbk\nx = 1\n";
        assert_eq!(decode_python_source(src.as_bytes()).unwrap(), src);
    }
}
