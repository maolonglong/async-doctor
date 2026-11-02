//! Cheap path-op downgrade, `open(p).read()` dedupe, and `functools.cache`
//! condition tagging.

use super::helpers::temp_project;
use crate::scan_path;

fn entry_counts(src: &str) -> (Vec<String>, Vec<(String, String)>) {
    let dir = temp_project(&[("app.py", src)]);
    let report = scan_path(&dir).unwrap();
    let findings = report
        .findings
        .iter()
        .map(|f| f.entry.name.clone())
        .collect();
    let candidates = report
        .candidates
        .iter()
        .map(|c| (c.entry.name.clone(), c.reason.clone()))
        .collect();
    (findings, candidates)
}

#[test]
fn cheap_path_ops_are_candidates() {
    let (findings, candidates) = entry_counts(concat!(
        "import os\n",
        "from pathlib import Path\n",
        "\n",
        "async def mkdir_kw(p: Path):\n",
        "    p.mkdir(parents=True, exist_ok=True)\n",
        "\n",
        "async def mkdir_pos(p: Path):\n",
        "    p.mkdir(0o755, True, True)\n",
        "\n",
        "async def makedirs_kw(d: str):\n",
        "    os.makedirs(d, exist_ok=True)\n",
        "\n",
        "async def touch(p: Path):\n",
        "    p.touch()\n",
        "\n",
        "async def unlink_kw(p: Path):\n",
        "    p.unlink(missing_ok=True)\n",
        "\n",
        "async def unlink_pos(p: Path):\n",
        "    p.unlink(True)\n",
    ));
    assert!(
        findings.is_empty(),
        "cheap path ops must not be findings: {findings:?}"
    );
    for entry in [
        "mkdir_kw",
        "mkdir_pos",
        "makedirs_kw",
        "touch",
        "unlink_kw",
        "unlink_pos",
    ] {
        assert!(
            candidates
                .iter()
                .any(|(e, r)| e == entry && r == "cheap-path-op"),
            "{entry} must be a cheap-path-op candidate: {candidates:?}"
        );
    }
}

#[test]
fn non_cheap_path_ops_stay_findings() {
    let (findings, _) = entry_counts(concat!(
        "import os\n",
        "from pathlib import Path\n",
        "\n",
        "async def mkdir_plain(p: Path):\n",
        "    p.mkdir()\n",
        "\n",
        "async def makedirs_plain(d: str):\n",
        "    os.makedirs(d)\n",
        "\n",
        "async def unlink_plain(p: Path):\n",
        "    p.unlink()\n",
        "\n",
        "async def rename(p: Path):\n",
        "    p.rename('x')\n",
        "\n",
        "async def mkdir_flag(p: Path, flag: bool):\n",
        "    p.mkdir(exist_ok=flag)\n",
        "\n",
        "async def os_remove(d: str):\n",
        "    os.remove(d)\n",
    ));
    for entry in [
        "mkdir_plain",
        "makedirs_plain",
        "unlink_plain",
        "rename",
        "mkdir_flag",
        "os_remove",
    ] {
        assert!(
            findings.iter().any(|e| e == entry),
            "{entry} must stay a finding: {findings:?}"
        );
    }
}

#[test]
fn open_read_expression_is_one_finding() {
    let dir = temp_project(&[(
        "app.py",
        "async def load(path: str) -> str:\n    return open(path).read()\n",
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(report.findings.len(), 1, "{:#?}", report.findings);
    assert_eq!(report.sink_sites.len(), 1, "{:#?}", report.sink_sites);
}

#[test]
fn functools_cache_helper_is_cache_miss_dependent() {
    let dir = temp_project(&[(
        "app.py",
        concat!(
            "import functools\n",
            "from functools import lru_cache\n",
            "\n",
            "@functools.cache\n",
            "def load_a() -> str:\n",
            "    return open('a').read()\n",
            "\n",
            "@lru_cache(maxsize=None)\n",
            "def load_b() -> str:\n",
            "    return open('b').read()\n",
            "\n",
            "async def entry_a():\n",
            "    load_a()\n",
            "\n",
            "async def entry_b():\n",
            "    load_b()\n",
        ),
    )]);
    let report = scan_path(&dir).unwrap();
    for entry in ["entry_a", "entry_b"] {
        let f = report
            .findings
            .iter()
            .find(|f| f.entry.name == entry)
            .unwrap_or_else(|| panic!("{entry} must still be a finding: {:#?}", report.findings));
        assert!(
            f.conditions.iter().any(|c| c == "cache_miss_dependent"),
            "{entry} must be tagged cache_miss_dependent: {:?}",
            f.conditions
        );
    }
}
