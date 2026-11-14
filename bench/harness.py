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
    rss_scale = 1024 if sys.platform == "darwin" else 1
    before_kib = usage_before.ru_maxrss // rss_scale
    after_kib = usage_after.ru_maxrss // rss_scale
    peak_kib = max(0, after_kib - before_kib)
    if peak_kib == 0:
        peak_kib = after_kib

    Path(out_json).write_text(proc.stdout, encoding="utf-8")
    if proc.returncode not in (0, 1):
        sys.stderr.write(proc.stderr)
        sys.stderr.write(f"async-doctor exited {proc.returncode}\n")
        sys.exit(proc.returncode)
    try:
        # Envelope checks only, not the complete JSON report schema.
        report = json.loads(proc.stdout)
        strict = os.environ.get("BENCH_STRICT") == "1"
        if not isinstance(report, dict):
            raise ValueError("JSON report is not an object")
        for field in (
            "findings", "candidates", "advice", "locationLessCandidates", "skipped",
            "sinkSites", "unusedSuppressions", "unresolvedSummary",
        ):
            if strict or field in ("findings", "candidates") or field in report:
                if not isinstance(report.get(field), list):
                    raise ValueError(f"JSON report missing {field} array")
        for field in ("findings", "candidates", "advice", "locationLessCandidates"):
            for item in report.get(field, []):
                if not isinstance(item, dict) or not isinstance(item.get("id"), str) or not item["id"]:
                    raise ValueError(f"invalid {field} item/id")
        if strict:
            if type(report.get("schemaVersion")) is not int or report["schemaVersion"] != 2:
                raise ValueError("strict mode requires schemaVersion 2")
            if report.get("complete") is not True or report["skipped"]:
                raise ValueError("strict mode requires complete=true and empty skipped")
            # Presence/type only; does not prove interpreter or dependency health.
            if not isinstance(report.get("environment"), dict):
                raise ValueError("strict mode requires an environment object")
            for field in ("omittedFindings", "omittedCandidates", "omittedAdvice"):
                if type(report.get(field)) is not int or report[field] != 0:
                    raise ValueError(f"strict mode requires {field}=0 (integer)")
    except ValueError as exc:
        sys.stderr.write(proc.stderr)
        raise SystemExit(f"invalid async-doctor report in {out_json}: {exc}") from exc
    findings = len(report["findings"])
    candidates = len(report["candidates"])
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
        (
            name,
            commit,
            scan,
            files,
            findings,
            candidates,
            cold,
            warm,
            rss,
            identical,
        ) = line.split("|")
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
                "identical": identical,
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

    print(
        "| Corpus | Commit | Scan | Files | Findings | Candidates | Cold | Warm | "
        "Peak RSS | Identical |"
    )
    print("| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |")
    for r in rows:
        print(
            f"| {r['name']} | `{r['commit'][:12]}` | `{r['scan']}/` | {r['files']} "
            f"| {r['findings']} | {r['candidates']} | {fmt_s(r['cold_s'])} "
            f"| {fmt_s(r['warm_s'])} | {fmt_rss(r['peak_rss_kib'])} "
            f"| {r['identical']} |"
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
