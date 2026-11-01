#!/usr/bin/env python3
"""Compare async-doctor JSON reports across an audit corpus.

Classifies finding changes into:
- added (+): new finding ID whose location key was not in baseline
- removed (-): baseline finding ID whose location key is not in the new output
- renumbered (~): finding ID changed, but (rule, entry file/line, chain file/line steps) match

Usage:
  # Compare two directories of `<label>.i.json` (or `<label>.json`) reports:
  python3 scripts/audit_compare.py diff <baseline_dir_or_pattern> <new_dir> [--label LABEL]

  # Run two binaries (or git refs) over a jobs manifest and compare:
  python3 scripts/audit_compare.py run --jobs <jobs.txt> --base-bin <bin_or_dir> --new-bin <bin> --out <dir>
"""

from __future__ import annotations

import argparse
import collections
import glob
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any


def load_findings(path: Path) -> dict[str, dict[str, Any]]:
    with path.open(encoding="utf-8") as f:
        data = json.load(f)
    return {item["id"]: item for item in data.get("findings", [])}


def location_key(finding: dict[str, Any]) -> tuple[Any, ...]:
    """Rule + entry location + call-chain step locations (matches renumbered IDs)."""
    entry = finding["entry"]
    chain = tuple((step["file"], step["line"]) for step in finding.get("chain", []))
    return (finding["rule"], entry["file"], entry["line"], chain)


def describe(finding: dict[str, Any]) -> str:
    entry = finding["entry"]
    chain = finding.get("chain", [])
    if chain:
        last = chain[-1]
        tail_loc = f"{last['file'][-50:]}:{last['line']}"
        tail_txt = f"{last.get('text', '')[:50]} | {last.get('resolution', '')[:50]}"
    else:
        tail_loc = "<empty-chain>"
        tail_txt = ""
    return f"{finding['rule']} {entry['file']}:{entry['line']} -> {tail_loc} {tail_txt}".rstrip()


def resolve_baseline_path(base_spec: str, label: str) -> Path | None:
    if "{L}" in base_spec:
        candidate = Path(base_spec.format(L=label))
        return candidate if candidate.is_file() else None
    base_dir = Path(base_spec)
    for suffix in (f"{label}.i.json", f"{label}.json"):
        candidate = base_dir / suffix
        if candidate.is_file():
            return candidate
    if label == "litellm":
        alt = base_dir / "litellm.i.1.json"
        if alt.is_file():
            return alt
    return None


def iter_new_reports(new_dir: Path) -> list[tuple[str, Path]]:
    items: dict[str, Path] = {}
    for p in sorted(new_dir.glob("*.json")):
        name = p.name
        if name.endswith(".i.1.json"):
            label = name[: -len(".i.1.json")]
        elif name.endswith(".i.json"):
            label = name[: -len(".i.json")]
        else:
            label = name[: -len(".json")]
        items[label] = p
    return sorted(items.items())


def compare_dirs(base_spec: str, new_dir: Path, summary_label: str = "diff") -> int:
    totals: collections.Counter[str] = collections.Counter()
    reports = iter_new_reports(new_dir)
    if not reports:
        print(f"error: no *.json reports found in {new_dir}", file=sys.stderr)
        return 2

    for label, new_path in reports:
        base_path = resolve_baseline_path(base_spec, label)
        if base_path is None:
            print(f"nobase {label}")
            continue
        new_map = load_findings(new_path)
        base_map = load_findings(base_path)

        added_ids = set(new_map) - set(base_map)
        removed_ids = set(base_map) - set(new_map)

        base_by_key = {location_key(base_map[i]): i for i in removed_ids}
        renumbered = [
            (i, base_by_key[location_key(new_map[i])])
            for i in added_ids
            if location_key(new_map[i]) in base_by_key
        ]
        renum_new = {x for x, _ in renumbered}
        renum_old = {y for _, y in renumbered}

        pure_added = sorted(added_ids - renum_new)
        pure_removed = sorted(removed_ids - renum_old)

        totals["renumbered"] += len(renumbered)
        totals["added"] += len(pure_added)
        totals["removed"] += len(pure_removed)
        totals["base"] += len(base_map)
        totals["new"] += len(new_map)
        totals["kept"] += len(set(new_map) & set(base_map))

        for i in pure_added:
            print("+", label, describe(new_map[i]))
        for i in pure_removed:
            print("-", label, describe(base_map[i]))
        for i, _ in sorted(renumbered):
            print("~", label, describe(new_map[i]))

    print(summary_label, dict(totals))
    return 0


def run_batch(
    jobs_file: Path,
    binary: Path,
    out_dir: Path,
    timeout_sec: int,
    root_dir: Path,
) -> None:
    out_dir.mkdir(parents=True, exist_ok=True)
    for raw in jobs_file.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split()
        label, scan_rel = parts[0], parts[1]
        venv_rel = parts[2] if len(parts) > 2 else None

        scan_path = (root_dir / scan_rel).resolve()
        if not scan_path.exists():
            print(f"skip {label}: missing {scan_path}", file=sys.stderr)
            continue

        cache_dir = Path(tempfile.mkdtemp(prefix=f"ad-audit-{label}-"))
        env = os.environ.copy()
        env["XDG_CACHE_HOME"] = str(cache_dir)
        if venv_rel:
            venv_path = (root_dir / venv_rel).resolve()
            if venv_path.exists():
                env["VIRTUAL_ENV"] = str(venv_path)
            else:
                env.pop("VIRTUAL_ENV", None)
        else:
            env.pop("VIRTUAL_ENV", None)

        json_out = out_dir / f"{label}.i.json"
        err_out = out_dir / f"{label}.err"
        try:
            with json_out.open("w", encoding="utf-8") as fo, err_out.open("w", encoding="utf-8") as fe:
                proc = subprocess.run(
                    [str(binary), "--format", "json", str(scan_path)],
                    stdout=fo,
                    stderr=fe,
                    env=env,
                    timeout=timeout_sec,
                    check=False,
                )
            (out_dir / f"{label}.exit").write_text(f"{proc.returncode}\n", encoding="utf-8")
        finally:
            shutil.rmtree(cache_dir, ignore_errors=True)


def main() -> int:
    if len(sys.argv) >= 4 and sys.argv[1] not in ("diff", "run", "-h", "--help"):
        # Backwards-compatible shorthand: audit_compare.py '<base>/{L}.i.json' <label> <new_dir>
        return compare_dirs(sys.argv[1], Path(sys.argv[3]), summary_label=sys.argv[2])

    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_diff = sub.add_parser("diff", help="Compare baseline JSON dir/pattern against new JSON dir")
    p_diff.add_argument("baseline", help="Baseline dir or pattern with {L}, e.g. 'baseline/{L}.i.json'")
    p_diff.add_argument("new_dir", type=Path, help="Directory with new *.i.json / *.json files")
    p_diff.add_argument("--label", default="new", help="Summary label")

    p_run = sub.add_parser("run", help="Run binary over jobs manifest and compare against baseline")
    p_run.add_argument("--jobs", type=Path, required=True, help="Jobs file (label path [venv])")
    p_run.add_argument("--root", type=Path, default=Path("."), help="Root dir for relative paths in jobs")
    p_run.add_argument("--baseline", required=True, help="Baseline dir/pattern or baseline binary")
    p_run.add_argument("--binary", type=Path, required=True, help="Candidate async-doctor binary")
    p_run.add_argument("--out", type=Path, required=True, help="Output directory for candidate JSONs")
    p_run.add_argument("--timeout", type=int, default=2400, help="Per-repo timeout in seconds")

    args = parser.parse_args()
    if args.cmd == "diff":
        return compare_dirs(args.baseline, args.new_dir, summary_label=args.label)

    base_path = Path(args.baseline)
    if base_path.is_file() and os.access(base_path, os.X_OK):
        base_out = args.out / "baseline"
        run_batch(args.jobs, base_path, base_out, args.timeout, args.root)
        base_spec = str(base_out)
    else:
        base_spec = args.baseline

    cand_out = args.out / "candidate" if base_spec == str(args.out / "baseline") else args.out
    run_batch(args.jobs, args.binary, cand_out, args.timeout, args.root)
    return compare_dirs(base_spec, cand_out, summary_label="candidate")


if __name__ == "__main__":
    raise SystemExit(main())
