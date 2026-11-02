//! Regression tests for recall gaps:
//! 1. Constructor calls (`__init__`, `__post_init__`, `__new__`, inherited `__init__`,
//!    `super().__init__`, `@classmethod` `cls(...)`, offloaded constructors) + `Path.mkdir`.
//! 2. `importlib.resources` / `importlib_resources` `Traversable` reads & metadata
//!    + `@functools.cache` / `@lru_cache` decorated sync helpers.
//! 3. `with open(...) as f: f.write(...)` primary sink-site attribution.

use std::fs;

use super::helpers::temp_project;
use crate::scan_path;

#[test]
fn constructor_init_follows_into_helpers_and_path_mkdir() {
    let dir = temp_project(&[(
        "worker.py",
        concat!(
            "from pathlib import Path\n",
            "\n",
            "def _read_head(repo: Path) -> str:\n",
            "    return (repo / \".git\" / \"HEAD\").read_text()\n",
            "\n",
            "class Worker:\n",
            "    def __init__(self, home: Path) -> None:\n",
            "        self.sha = _read_head(Path.cwd())\n",
            "        (home / \"logs\").mkdir(parents=True, exist_ok=True)\n",
            "\n",
            "async def main() -> None:\n",
            "    w = Worker(Path(\"/tmp\"))\n",
        ),
    )]);
    let report = scan_path(&dir).unwrap();
    let main_findings: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.entry.name == "main" && f.rule == "blocking-file")
        .collect();
    assert_eq!(
        main_findings.len(),
        2,
        "expected read_text and mkdir findings from Worker.__init__: {:?}",
        report.findings
    );
    assert!(
        main_findings.iter().any(|f| {
            f.chain.iter().any(|s| s.text.contains("Worker("))
                && f.chain.iter().any(|s| s.text.contains("_read_head"))
                && f.chain.iter().any(|s| s.text.contains("read_text"))
        }),
        "expected chain main -> Worker(...) -> _read_head -> read_text: {:?}",
        main_findings
    );
    assert!(
        main_findings.iter().any(|f| {
            f.chain.iter().any(|s| s.text.contains("Worker("))
                && f.chain.iter().any(|s| {
                    s.text.contains("mkdir") && s.resolution.contains("pathlib.Path.mkdir")
                })
        }),
        "expected chain main -> Worker(...) -> Path.mkdir: {:?}",
        main_findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn constructor_lifecycle_post_init_new_inheritance_super_classmethod_and_offload() {
    let dir = temp_project(&[
        (
            "base.py",
            concat!(
                "from pathlib import Path\n",
                "\n",
                "class BaseWorker:\n",
                "    def __init__(self, root: Path) -> None:\n",
                "        (root / \"state\").mkdir(exist_ok=True)\n",
            ),
        ),
        (
            "app.py",
            concat!(
                "import asyncio\n",
                "import time\n",
                "from dataclasses import dataclass\n",
                "from pathlib import Path\n",
                "from base import BaseWorker\n",
                "\n",
                "@dataclass\n",
                "class LoadedConfig:\n",
                "    path: Path\n",
                "    def __post_init__(self) -> None:\n",
                "        self.raw = self.path.read_text()\n",
                "\n",
                "class CustomNew:\n",
                "    def __new__(cls):\n",
                "        time.sleep(0.01)\n",
                "        return super().__new__(cls)\n",
                "\n",
                "class InheritedInit(BaseWorker):\n",
                "    pass\n",
                "\n",
                "class SuperInit(BaseWorker):\n",
                "    def __init__(self, root: Path) -> None:\n",
                "        super().__init__(root)\n",
                "\n",
                "class FactoryService:\n",
                "    def __init__(self, target: Path) -> None:\n",
                "        target.unlink(missing_ok=True)\n",
                "\n",
                "    @classmethod\n",
                "    def create(cls, target: Path) -> \"FactoryService\":\n",
                "        return cls(target)\n",
                "\n",
                "async def entry_post_init(p: Path) -> None:\n",
                "    LoadedConfig(p)\n",
                "\n",
                "async def entry_new() -> None:\n",
                "    CustomNew()\n",
                "\n",
                "async def entry_inherited(p: Path) -> None:\n",
                "    InheritedInit(p)\n",
                "\n",
                "async def entry_super(p: Path) -> None:\n",
                "    SuperInit(p)\n",
                "\n",
                "async def entry_factory(p: Path) -> None:\n",
                "    FactoryService.create(p)\n",
                "\n",
                "async def entry_offloaded(p: Path) -> None:\n",
                "    await asyncio.to_thread(LoadedConfig, p)\n",
                "    loop = asyncio.get_running_loop()\n",
                "    await loop.run_in_executor(None, SuperInit, p)\n",
            ),
        ),
    ]);
    let report = scan_path(&dir).unwrap();
    let names: std::collections::BTreeSet<_> = report
        .findings
        .iter()
        .map(|f| f.entry.name.as_str())
        .collect();
    for expected in [
        "entry_post_init",
        "entry_new",
        "entry_inherited",
        "entry_super",
        "entry_factory",
    ] {
        assert!(
            names.contains(expected),
            "missing finding for {expected}; got names={names:?}, findings={:?}",
            report.findings
        );
    }
    assert!(
        !names.contains("entry_offloaded"),
        "offloaded constructor calls must not produce findings (including offload-coroutine): {:?}",
        report.findings
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn importlib_resources_traversable_and_functools_cache_helpers() {
    let dir = temp_project(&[
        (
            "helpers.py",
            concat!(
                "import functools\n",
                "from functools import cache, lru_cache\n",
                "from importlib.resources import files\n",
                "\n",
                "@cache\n",
                "def cached_skill(name: str) -> str:\n",
                "    return files(\"mypkg\").joinpath(\"skills\", name, \"SKILL.md\").read_text(encoding=\"utf-8\")\n",
                "\n",
                "@lru_cache(maxsize=16)\n",
                "def lru_bytes(name: str) -> bytes:\n",
                "    return files(\"mypkg\").joinpath(name).read_bytes()\n",
                "\n",
                "@functools.cache\n",
                "def mod_cached_open(name: str) -> str:\n",
                "    with (files(\"mypkg\") / name).open(\"r\", encoding=\"utf-8\") as f:\n",
                "        return f.read()\n",
            ),
        ),
        (
            "app.py",
            concat!(
                "from importlib.resources import files, read_text, is_resource\n",
                "import importlib_resources\n",
                "import helpers\n",
                "from helpers import cached_skill, lru_bytes\n",
                "\n",
                "async def load_direct(name: str) -> str:\n",
                "    return files(\"mypkg\").joinpath(\"skills\", name, \"SKILL.md\").read_text(encoding=\"utf-8\")\n",
                "\n",
                "async def load_functional() -> str:\n",
                "    return read_text(\"mypkg\", \"README.txt\")\n",
                "\n",
                "async def load_backport(name: str) -> str:\n",
                "    return importlib_resources.files(\"mypkg\").joinpath(name).read_text(encoding=\"utf-8\")\n",
                "\n",
                "async def load_via_cached(name: str) -> bytes:\n",
                "    _ = cached_skill(name)\n",
                "    _ = helpers.mod_cached_open(name)\n",
                "    return lru_bytes(name)\n",
                "\n",
                "async def check_metadata_only(name: str) -> bool:\n",
                "    a = files(\"mypkg\").joinpath(name).is_file()\n",
                "    b = files(\"mypkg\").joinpath(name).is_dir()\n",
                "    c = is_resource(\"mypkg\", name)\n",
                "    d = importlib_resources.files(\"mypkg\").joinpath(name).is_file()\n",
                "    return a or b or c or d\n",
            ),
        ),
    ]);
    let report = scan_path(&dir).unwrap();
    let finding_entries: std::collections::BTreeSet<_> = report
        .findings
        .iter()
        .filter(|f| f.rule == "blocking-file")
        .map(|f| f.entry.name.as_str())
        .collect();
    for expected in [
        "load_direct",
        "load_functional",
        "load_backport",
        "load_via_cached",
    ] {
        assert!(
            finding_entries.contains(expected),
            "expected blocking-file finding for {expected}; got {finding_entries:?}, findings={:?}",
            report.findings
        );
    }
    let cached_count = report
        .findings
        .iter()
        .filter(|f| f.entry.name == "load_via_cached" && f.rule == "blocking-file")
        .count();
    assert_eq!(
        cached_count, 3,
        "all 3 @cache/@lru_cache helpers must be followed: {:?}",
        report.findings
    );
    assert!(
        !finding_entries.contains("check_metadata_only"),
        "Traversable.is_file/is_dir and is_resource must be metadata candidates, never findings: {:?}",
        report.findings
    );
    let metadata_candidates = report
        .candidates
        .iter()
        .filter(|c| c.entry.name == "check_metadata_only" && c.reason == "metadata")
        .count();
    assert!(
        metadata_candidates >= 3,
        "expected metadata candidates for Traversable.is_file/is_dir/is_resource, got {metadata_candidates}: {:?}",
        report.candidates
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn with_open_write_attributes_sink_site_to_open_line() {
    let dir = temp_project(&[(
        "io_mod.py",
        concat!(
            "from pathlib import Path\n",
            "\n",
            "async def save(p: Path, data: str) -> None:\n",
            "    with open(p, \"w\", encoding=\"utf-8\") as f:\n",
            "        f.write(data)\n",
        ),
    )]);
    let report = scan_path(&dir).unwrap();
    assert_eq!(
        report.findings.len(),
        1,
        "with open + f.write must merge into 1 finding: {:?}",
        report.findings
    );
    let f = &report.findings[0];
    assert_eq!(
        f.chain.len(),
        2,
        "merged chain must retain open and write steps: {:?}",
        f.chain
    );
    assert_eq!(f.chain[0].line, 4);
    assert_eq!(f.chain[1].line, 5);
    assert!(
        f.why.contains("io_mod.py:4"),
        "f.why must point to the open line (line 4), got: {}",
        f.why
    );
    assert_eq!(report.sink_sites.len(), 1);
    let site = &report.sink_sites[0];
    assert_eq!(
        site.line, 4,
        "sinkSites must attribute to the open line (line 4), not f.write (line 5): {:?}",
        site
    );
    assert!(
        site.text.contains("open("),
        "sinkSites text must show the open statement: {:?}",
        site
    );
    let _ = fs::remove_dir_all(dir);
}
