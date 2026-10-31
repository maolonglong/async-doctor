#!/usr/bin/env python3
"""Helpers for bench/run.sh — timing, file counts, markdown emission."""

from __future__ import annotations

import json
import os
import resource
import subprocess
import sys
from pathlib import Path


def count_py_files(root: str) -> None:
    skip_dirs = {
        ".venv",
        "venv",
        ".git",
        "__pycache__",
        "node_modules",
        ".tox",
        ".mypy_cache",
        ".ruff_cache",
        ".pytest_cache",
        "dist",
        "build",
    }
    n = 0
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [
            d for d in dirnames if d not in skip_dirs and not d.endswith(".egg-info")
        ]
        for f in filenames:
            if f.endswith(".py"):
                n += 1
    print(n)


def run_timed(bin_path: str, scan_path: str, out_json: str, venv_dir: str) -> None:
    env = os.environ.copy()
    env["VIRTUAL_ENV"] = venv_dir
    t0 = __import__("time").perf_counter()
    usage_before = resource.getrusage(resource.RUSAGE_CHILDREN)
    proc = subprocess.run(
        [bin_path, "--format", "json", scan_path],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
    )
    elapsed = __import__("time").perf_counter() - t0
    usage_after = resource.getrusage(resource.RUSAGE_CHILDREN)
    peak_kib = max(0, usage_after.ru_maxrss - usage_before.ru_maxrss)
    if peak_kib == 0:
        peak_kib = usage_after.ru_maxrss

    Path(out_json).write_text(proc.stdout)
    try:
        report = json.loads(proc.stdout) if proc.stdout.strip() else {}
    except json.JSONDecodeError:
        report = {}
    findings = len(report.get("findings", []))
    candidates = len(report.get("candidates", []))
    if proc.returncode not in (0, 1):
        sys.stderr.write(proc.stderr)
        sys.stderr.write(f"async-doctor exited {proc.returncode}\n")
        sys.exit(proc.returncode)
    print(f"{elapsed:.3f}|{peak_kib}|{findings}|{candidates}")


def parse_repos(toml_path: str) -> None:
    text = Path(toml_path).read_text()
    repos: list[dict] = []
    cur: dict | None = None
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].rstrip()
        if not line.strip():
            continue
        if line.strip() == "[[repo]]":
            if cur:
                repos.append(cur)
            cur = {}
            continue
        if cur is None:
            continue
        key, _, val = line.partition("=")
        key = key.strip()
        val = val.strip()
        if val.startswith("[") and val.endswith("]"):
            inner = val[1:-1].strip()
            items = []
            for part in inner.split(","):
                part = part.strip().strip('"').strip("'")
                if part:
                    items.append(part)
            cur[key] = items
        else:
            cur[key] = val.strip('"').strip("'")
    if cur:
        repos.append(cur)

    for r in repos:
        for req in ("name", "url", "commit", "scan"):
            if req not in r:
                raise SystemExit(f"repo missing {req}: {r}")
        install = "\x1f".join(r.get("install", []))
        sys.stdout.write(f"{r['name']}|{r['url']}|{r['commit']}|{r['scan']}|{install}\0")


def emit_table(results_json: str, results_tsv: str) -> None:
    out = Path(results_json)
    tsv = Path(results_tsv)
    rows = []
    for line in tsv.read_text().splitlines():
        line = line.strip()
        if not line:
            continue
        name, commit, scan, files, findings, candidates, cold, warm, rss = line.split("|")
        rows.append(
            {
                "name": name,
                "commit": commit,
                "scan": scan,
                "files": int(files),
                "findings": int(findings),
                "candidates": int(candidates),
                "cold_s": float(cold),
                "warm_s": float(warm),
                "peak_rss_kib": int(rss),
            }
        )
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(rows, indent=2) + "\n")

    def fmt_s(v: float) -> str:
        if v < 10:
            return f"{v:.2f}s"
        return f"{v:.1f}s"

    def fmt_rss(kib: int) -> str:
        mib = kib / 1024.0
        if mib >= 1024:
            return f"{mib / 1024:.2f} GiB"
        return f"{mib:.0f} MiB"

    print("| Corpus | Commit | Scan | Files | Findings | Candidates | Cold | Warm | Peak RSS |")
    print("| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |")
    for r in rows:
        print(
            f"| {r['name']} | `{r['commit'][:12]}` | `{r['scan']}/` | {r['files']} "
            f"| {r['findings']} | {r['candidates']} | {fmt_s(r['cold_s'])} "
            f"| {fmt_s(r['warm_s'])} | {fmt_rss(r['peak_rss_kib'])} |"
        )


def main() -> None:
    if len(sys.argv) < 2:
        raise SystemExit(f"usage: {sys.argv[0]} <count|run|parse|emit> ...")
    cmd = sys.argv[1]
    if cmd == "count":
        count_py_files(sys.argv[2])
    elif cmd == "run":
        run_timed(sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5])
    elif cmd == "parse":
        parse_repos(sys.argv[2])
    elif cmd == "emit":
        emit_table(sys.argv[2], sys.argv[3])
    else:
        raise SystemExit(f"unknown command: {cmd}")


if __name__ == "__main__":
    main()
