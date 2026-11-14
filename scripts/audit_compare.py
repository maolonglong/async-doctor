#!/usr/bin/env python3
"""Compare async-doctor JSON reports across an audit corpus.

Classifies finding changes into:
- added (+): new finding ID whose location key was not in baseline
- removed (-): baseline finding ID whose location key is not in the new output
- renumbered (~): finding ID changed, but (rule, entry file/line, chain file/line steps) match
- modified (!): same diagnostic ID, different payload

Candidates, location-less candidates, and advice are compared separately.
Use --strict to fail on differences, missing reports, or incomplete/truncated scans.
Normal mode prints differences for manual review without failing on them.

Usage:
  # Compare two directories of `<label>.i.json` (or `<label>.json`) reports:
  python3 scripts/audit_compare.py diff <baseline_dir_or_pattern> <new_dir> [--label LABEL]

  # Run a candidate binary over a jobs manifest and compare against a baseline:
  python3 scripts/audit_compare.py run \\
    --jobs <jobs.txt> --root <audit-dir> \\
    --baseline '<audit-dir>/baseline/{L}.i.json' \\
    --binary ./target/release/async-doctor \\
    --out /tmp/async-doctor-audit-out
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


TIERS = ("findings", "candidates", "locationLessCandidates", "advice")


def load_report(path: Path, strict: bool = False) -> dict[str, Any]:
    """Validate the envelope and comparison keys, not the entire report schema."""
    text = path.read_text(encoding="utf-8")
    if not text.strip():
        raise ValueError(f"empty JSON report: {path}")
    try:
        data = json.loads(text)
    except json.JSONDecodeError as exc:
        raise ValueError(f"invalid JSON report: {path}: {exc}") from exc
    if not isinstance(data, dict):
        raise ValueError(f"JSON report is not an object: {path}")
    for field in (*TIERS, "skipped", "sinkSites", "unusedSuppressions", "unresolvedSummary"):
        if strict or field == "findings" or field in data:
            if not isinstance(data.get(field), list):
                raise ValueError(f"JSON report missing {field} array: {path}")
    for tier in TIERS:
        seen = set()
        for item in data.get(tier, []):
            if not isinstance(item, dict) or not isinstance(item.get("id"), str) or not item["id"]:
                raise ValueError(f"invalid {tier} item/id: {path}")
            if item["id"] in seen:
                raise ValueError(f"duplicate {tier} id {item['id']}: {path}")
            seen.add(item["id"])
            if not isinstance(item.get("rule"), str) or not isinstance(item.get("chain", []), list):
                raise ValueError(f"invalid {tier} rule/chain: {path}")
            for loc in [item.get("entry"), *item.get("chain", [])]:
                if (
                    not isinstance(loc, dict) or not isinstance(loc.get("file"), str)
                    or type(loc.get("line")) is not int or loc["line"] < 0
                ):
                    raise ValueError(f"invalid {tier} location: {path}")
            for step in item.get("chain", []):
                if any(not isinstance(step.get(key, ""), str) for key in ("text", "resolution")):
                    raise ValueError(f"invalid {tier} chain text: {path}")
    if strict:
        if type(data.get("schemaVersion")) is not int or data["schemaVersion"] != 2:
            raise ValueError(f"strict mode requires schemaVersion 2: {path}")
        if data.get("complete") is not True or data["skipped"]:
            raise ValueError(f"strict mode requires complete=true and empty skipped: {path}")
        # Presence/type only; does not prove interpreter or dependency health.
        if not isinstance(data.get("environment"), dict):
            raise ValueError(f"strict mode requires an environment object: {path}")
        for field in ("omittedFindings", "omittedCandidates", "omittedAdvice"):
            if type(data.get(field)) is not int or data[field] != 0:
                raise ValueError(f"strict mode requires {field}=0 (integer): {path}")
        exit_path = path.parent / f"{label_from_filename(path.name)}.exit"
        if exit_path.exists() and exit_path.read_text(encoding="utf-8").strip() not in ("0", "1"):
            raise ValueError(f"unsuccessful scan recorded in {exit_path}")
    return data


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


def label_from_filename(name: str) -> str:
    for suffix in (".i.1.json", ".i.json", ".cold.json", ".warm.json", ".json"):
        if name.endswith(suffix):
            return name[: -len(suffix)]
    return name


def resolve_baseline_path(base_spec: str, label: str) -> Path | None:
    if "{L}" in base_spec:
        candidate = Path(base_spec.format(L=label))
        return candidate if candidate.is_file() else None
    base_dir = Path(base_spec)
    if base_dir.is_file():
        return base_dir
    for suffix in (f"{label}.i.json", f"{label}.json"):
        candidate = base_dir / suffix
        if candidate.is_file():
            return candidate
    if label == "litellm":
        alt = base_dir / "litellm.i.1.json"
        if alt.is_file():
            return alt
    return None


def iter_new_reports(new_dir: Path, strict: bool = False) -> list[tuple[str, Path]]:
    if new_dir.is_file():
        return [(label_from_filename(new_dir.name), new_dir)]
    items: dict[str, Path] = {}
    for p in sorted(new_dir.glob("*.json")):
        if p.name == "results.json" or not p.is_file():
            continue
        label = label_from_filename(p.name)
        if strict and label in items:
            raise ValueError(f"duplicate report label {label}: {items[label]} and {p}")
        items[label] = p
    return sorted(items.items())


def compare_tier(base_items: list, new_items: list, label: str) -> collections.Counter[str]:
    base_map = {item["id"]: item for item in base_items}
    new_map = {item["id"]: item for item in new_items}
    added_ids = set(new_map) - set(base_map)
    removed_ids = set(base_map) - set(new_map)

    base_by_key: dict[tuple, list[str]] = collections.defaultdict(list)
    for i in sorted(removed_ids):
        base_by_key[location_key(base_map[i])].append(i)
    renumbered = []
    for i in sorted(added_ids):
        matches = base_by_key[location_key(new_map[i])]
        if matches:
            renumbered.append((i, matches.pop()))
    pure_added = sorted(added_ids - {new for new, _ in renumbered})
    pure_removed = sorted(removed_ids - {old for _, old in renumbered})
    kept = set(new_map) & set(base_map)
    modified = sorted(i for i in kept if new_map[i] != base_map[i])

    for i in pure_added:
        print("+", label, describe(new_map[i]))
    for i in pure_removed:
        print("-", label, describe(base_map[i]))
    for i, old in renumbered:
        print("~", label, describe(new_map[i]), f"({old} -> {i})")
    for i in modified:
        changes = {
            key: {"before": base_map[i].get(key), "after": new_map[i].get(key)}
            for key in sorted(base_map[i].keys() | new_map[i].keys())
            if base_map[i].get(key) != new_map[i].get(key)
        }
        print("!", label, describe(new_map[i]), json.dumps(changes, sort_keys=True))
    return collections.Counter(
        added=len(pure_added), removed=len(pure_removed), renumbered=len(renumbered),
        modified=len(modified), base=len(base_map), new=len(new_map), kept=len(kept),
    )


def compare_dirs(
    base_spec: str, new_dir: Path, summary_label: str = "diff", strict: bool = False,
) -> int:
    totals: collections.Counter[str] = collections.Counter()
    try:
        reports = iter_new_reports(new_dir, strict=strict)
        baselines = {}
        if strict:
            if "{L}" in base_spec:
                if base_spec.count("{L}") != 1:
                    raise ValueError("strict baseline patterns require exactly one {L}")
                prefix, suffix = base_spec.split("{L}")
                for name in sorted(glob.glob(glob.escape(prefix) + "*" + glob.escape(suffix))):
                    if Path(name).is_file():
                        label = name[len(prefix):len(name) - len(suffix) if suffix else None]
                        baselines[label] = Path(name)
            elif Path(base_spec).is_file():
                if len(reports) > 1:
                    raise ValueError("a single baseline report cannot cover multiple candidate reports")
                baselines = {label: Path(base_spec) for label, _ in reports}
            else:
                baselines = dict(iter_new_reports(Path(base_spec), strict=True))
    except (OSError, ValueError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    for label in sorted(baselines.keys() - dict(reports).keys()):
        print(f"fail: missing candidate {label}")
        totals["missing_candidate"] += 1
    if not reports:
        print(f"error: no *.json reports found in {new_dir}", file=sys.stderr)
        return 2

    different = False
    for label, new_path in reports:
        try:
            new_report = load_report(new_path, strict=strict)
        except (OSError, ValueError) as exc:
            print(f"fail {label}: {exc}")
            totals["failed"] += 1
            continue
        base_path = baselines.get(label) if strict else resolve_baseline_path(base_spec, label)
        if base_path is None:
            print(f"nobase {label}")
            totals["nobase"] += 1
            continue
        try:
            base_report = load_report(base_path, strict=strict)
        except (OSError, ValueError) as exc:
            print(f"fail {label}: baseline {exc}")
            totals["failed"] += 1
            continue

        for tier in TIERS:
            counts = compare_tier(
                base_report.get(tier, []), new_report.get(tier, []),
                label if tier == "findings" else f"{tier} {label}",
            )
            prefix = "" if tier == "findings" else f"{tier}_"
            for key, value in counts.items():
                totals[prefix + key] += value
            different |= any(counts[key] for key in ("added", "removed", "renumbered", "modified"))

    print(summary_label, dict(totals))
    failed = totals["failed"] or (strict and (different or totals["nobase"] or totals["missing_candidate"]))
    return 1 if failed else 0


def run_batch(
    jobs_file: Path,
    binary: Path,
    out_dir: Path,
    timeout_sec: int,
    root_dir: Path,
    strict: bool = False,
) -> bool:
    out_dir.mkdir(parents=True, exist_ok=True)
    successful = True
    labels = set()
    for number, raw in enumerate(jobs_file.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split()
        if len(parts) not in (2, 3):
            print(f"fail: invalid job at {jobs_file}:{number}", file=sys.stderr)
            successful = False
            continue
        label, scan_rel = parts[0], parts[1]
        if label in labels or Path(label).name != label or label in (".", ".."):
            print(f"fail: duplicate or invalid job label {label}", file=sys.stderr)
            successful = False
            continue
        labels.add(label)
        venv_rel = parts[2] if len(parts) > 2 else None

        json_out = out_dir / f"{label}.i.json"
        err_out = out_dir / f"{label}.err"
        exit_out = out_dir / f"{label}.exit"
        # Never let a skipped/failed current job reuse a prior successful report.
        for path in (json_out, err_out, exit_out):
            path.unlink(missing_ok=True)
        scan_path = (root_dir / scan_rel).resolve()
        if not scan_path.exists():
            print(f"skip {label}: missing {scan_path}", file=sys.stderr)
            successful = False
            continue

        env = os.environ.copy()
        env.pop("VIRTUAL_ENV", None)
        if venv_rel:
            venv_path = (root_dir / venv_rel).resolve()
            if venv_path.is_dir():
                env["VIRTUAL_ENV"] = str(venv_path)
            else:
                print(f"skip venv {label}: missing directory {venv_path}", file=sys.stderr)
                if strict:
                    successful = False
                    continue

        cache_dir = Path(tempfile.mkdtemp(prefix=f"ad-audit-{label}-"))
        env["XDG_CACHE_HOME"] = str(cache_dir)
        status = "error"
        try:
            with json_out.open("w", encoding="utf-8") as fo, err_out.open("w", encoding="utf-8") as fe:
                proc = subprocess.run(
                    [str(binary.resolve()), "--format", "json", str(scan_path)],
                    stdout=fo,
                    stderr=fe,
                    env=env,
                    timeout=timeout_sec,
                    check=False,
                )
            status = str(proc.returncode)
            if proc.returncode not in (0, 1):
                print(f"fail {label}: scan exited {proc.returncode} ({binary})", file=sys.stderr)
                successful = False
        except subprocess.TimeoutExpired:
            status = "timeout"
            print(f"fail {label}: scan timeout after {timeout_sec}s", file=sys.stderr)
            successful = False
            if not strict:
                raise SystemExit(1)
        except OSError as exc:
            print(f"fail {label}: scan could not run: {exc}", file=sys.stderr)
            successful = False
            if not strict:
                raise SystemExit(1)
        finally:
            shutil.rmtree(cache_dir, ignore_errors=True)
            exit_out.write_text(status + "\n", encoding="utf-8")
    if strict:
        unexpected = {label for label, _ in iter_new_reports(out_dir)} - labels
        if unexpected or not labels:
            print(f"fail: empty jobs or stale/unexpected reports in {out_dir}: {sorted(unexpected)}", file=sys.stderr)
            successful = False
    return successful


def main() -> int:
    argv = sys.argv[1:]
    if len(argv) >= 3 and argv[0] not in ("diff", "run") and not argv[0].startswith("-"):
        # Backwards-compatible shorthand: audit_compare.py '<base>/{L}.i.json' <label> <new_dir>
        argv = ["diff", argv[0], argv[2], "--label", argv[1], *argv[3:]]

    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_diff = sub.add_parser("diff", help="Compare baseline JSON dir/pattern against new JSON dir")
    p_diff.add_argument("baseline", help="Baseline dir or pattern with {L}, e.g. 'baseline/{L}.i.json'")
    p_diff.add_argument("new_dir", type=Path, help="Directory with new *.i.json / *.json files")
    p_diff.add_argument("--label", default="new", help="Summary label")
    p_diff.add_argument("--strict", action="store_true", help="Fail on differences, missing or incomplete reports")

    p_run = sub.add_parser("run", help="Run binary over jobs manifest and compare against baseline")
    p_run.add_argument("--jobs", type=Path, required=True, help="Jobs file (label path [venv])")
    p_run.add_argument("--root", type=Path, default=Path("."), help="Root dir for relative paths in jobs")
    p_run.add_argument("--baseline", required=True, help="Baseline dir/pattern or baseline binary")
    p_run.add_argument("--binary", type=Path, required=True, help="Candidate async-doctor binary")
    p_run.add_argument("--out", type=Path, required=True, help="Output directory for candidate JSONs")
    p_run.add_argument("--timeout", type=int, default=2400, help="Per-repo timeout in seconds")
    p_run.add_argument("--strict", action="store_true", help="Also require successful jobs and dependency paths")

    args = parser.parse_args(argv)
    if args.cmd == "diff":
        return compare_dirs(args.baseline, args.new_dir, summary_label=args.label, strict=args.strict)

    baseline_ok = True
    base_path = Path(args.baseline)
    if base_path.is_file() and os.access(base_path, os.X_OK):
        base_out = args.out / "baseline"
        baseline_ok = run_batch(args.jobs, base_path, base_out, args.timeout, args.root, strict=args.strict)
        base_spec = str(base_out)
    else:
        base_spec = args.baseline

    cand_out = args.out / "candidate" if base_spec == str(args.out / "baseline") else args.out
    candidate_ok = run_batch(args.jobs, args.binary, cand_out, args.timeout, args.root, strict=args.strict)
    comparison = compare_dirs(base_spec, cand_out, summary_label="candidate", strict=args.strict)
    return comparison or (1 if args.strict and not (baseline_ok and candidate_ok) else 0)


if __name__ == "__main__":
    raise SystemExit(main())
