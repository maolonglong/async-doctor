#!/usr/bin/env python3
"""Capture repair checks outside project roots; gate audited session comparisons.

This repository tool does not launch agents, prevent cross-thread reads, judge
test adequacy, or prove a repair is optimal. Transcript review is a separate,
parent-owned input bound to the exact before/after capture hashes.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shlex
import subprocess
import sys
import uuid
from pathlib import Path

from audit_compare import load_report


SKIP_DIRS = {".git", ".venv", "venv", "site-packages", "__pycache__", "target",
             ".pytest_cache", ".mypy_cache", ".ruff_cache", "node_modules"}
ENVIRONMENT_CODE = (
    "import importlib.metadata as m, json, sys; "
    "print(json.dumps({'executable': sys.executable, 'prefix': sys.prefix, "
    "'version': sys.version, 'packages': sorted((d.metadata['Name'], d.version) "
    "for d in m.distributions())}))"
)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def disjoint(project: Path, evidence: Path) -> None:
    if evidence.is_relative_to(project) or project.is_relative_to(evidence):
        raise ValueError("project and evidence paths must be disjoint, including symlink aliases")


def project_path(project: Path, relative: str) -> Path:
    path = project / relative
    if Path(relative).is_absolute() or ".." in Path(relative).parts:
        raise ValueError(f"expected a project-relative path: {relative}")
    if path.is_symlink() or not path.resolve().is_relative_to(project):
        raise ValueError(f"source path escapes the project or is a symlink: {relative}")
    return path


def source_files(project: Path, extras: list[str], protected: list[str]) -> tuple[dict, dict]:
    for name in [*extras, *protected]:
        path = project_path(project, name)
        if not path.exists() or (name in extras and not path.is_file()):
            raise ValueError(f"missing source/contract path: {name}")
    prefixes = [Path(name) for name in protected]
    files, contract = {}, {}
    for directory, names, filenames in os.walk(project):
        names[:] = sorted(name for name in names if name not in SKIP_DIRS)
        for name in names:
            if (Path(directory) / name).is_symlink():
                raise ValueError(f"symlink source directory: {Path(directory) / name}")
        for name in sorted(filenames):
            path = Path(directory) / name
            relative = path.relative_to(project)
            guarded = any(relative == prefix or relative.is_relative_to(prefix) for prefix in prefixes)
            if path.suffix not in {".py", ".pyi"} and relative.as_posix() not in extras and not guarded:
                continue
            project_path(project, relative.as_posix())
            files[relative.as_posix()] = path.read_bytes()
            if guarded:
                contract[relative.as_posix()] = digest(files[relative.as_posix()])
    if not any(Path(name).suffix in {".py", ".pyi"} for name in files):
        raise ValueError("no Python sources selected")
    return files, contract


def run_command(argv: list[str], project: Path, output: Path, name: str, env: dict, timeout: float) -> dict:
    timed_out = False
    try:
        process = subprocess.run(argv, cwd=project, env=env, capture_output=True, timeout=timeout)
        stdout, stderr, code = process.stdout, process.stderr, process.returncode
    except subprocess.TimeoutExpired as exc:
        stdout, stderr, code, timed_out = exc.output or b"", exc.stderr or b"", None, True
    except OSError as exc:
        stdout, stderr, code = b"", str(exc).encode(), None
    extension = "json" if name in {"scan", "environment"} else "stdout"
    (output / f"{name}.{extension}").write_bytes(stdout)
    (output / f"{name}.stderr").write_bytes(stderr)
    (output / f"{name}.exit").write_text(str(code) + "\n", encoding="utf-8")
    return {"argv": argv, "exit": code, "timedOut": timed_out}


def validate_scan(output: Path) -> None:
    report = load_report(output / "scan.json", strict=True)
    environment = json.loads((output / "environment.json").read_text(encoding="utf-8"))
    if not isinstance(environment, dict) or not all(key in environment for key in ("version", "prefix", "packages")):
        raise ValueError("invalid Python environment metadata")
    prefix = report["environment"].get("prefix")
    if not isinstance(prefix, str) or not prefix or Path(prefix).resolve() != Path(environment["prefix"]).resolve():
        raise ValueError("scan environment prefix does not match the selected Python environment")


def capture(args: argparse.Namespace) -> int:
    project, output = args.project.resolve(), args.output.resolve()
    disjoint(project, output)
    if not project.is_dir() or args.timeout <= 0:
        raise ValueError("capture requires a project directory and a positive timeout")
    scan = project_path(project, args.scan)
    if not scan.exists():
        raise ValueError(f"missing scan target: {scan}")
    extras = sorted({Path(name).as_posix() for name in args.file})
    protected = sorted({Path(name).as_posix() for name in args.protect})
    files, contract = source_files(project, extras, protected)
    binary = args.binary.resolve(strict=True)
    # Resolving a venv's Python symlink would lose the virtualenv identity.
    python = args.python.absolute()
    test_args = shlex.split(args.tests)
    if not test_args:
        raise ValueError("a Python test command is required")
    identity = {"binarySha256": digest(binary.read_bytes()),
                "skillSha256": digest(args.skill.read_bytes()) if args.skill else None}
    output.mkdir(parents=True, exist_ok=False)
    for name, data in files.items():
        destination = output / "source" / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(data)
    env = os.environ.copy()
    env.update(PYTHONDONTWRITEBYTECODE="1", NO_COLOR="1", XDG_CACHE_HOME=str(output / "cache"))
    env.pop("VIRTUAL_ENV", None)
    env["PATH"] = str(python.parent) + os.pathsep + env.get("PATH", "")
    commands = {
        "environment": [str(python), "-c", ENVIRONMENT_CODE],
        "tests": [str(python), *test_args],
        "scan": [str(binary), "--python", str(python), "--format", "json", args.scan],
    }
    results = {name: run_command(argv, project, output, name, env, args.timeout)
               for name, argv in commands.items()}
    errors = []
    for name, result in results.items():
        if result["exit"] not in ((0,) if name == "environment" else (0, 1)):
            errors.append(f"{name} command failed or timed out: {result['exit']}")
    try:
        validate_scan(output)
    except (OSError, ValueError) as exc:
        errors.append(str(exc))
    try:
        current, _ = source_files(project, extras, protected)
        if current != files:
            errors.append("project source changed during collection")
        if digest(binary.read_bytes()) != identity["binarySha256"]:
            errors.append("binary changed during collection")
        if args.skill and digest(args.skill.read_bytes()) != identity["skillSha256"]:
            errors.append("skill changed during collection")
    except (OSError, ValueError) as exc:
        errors.append(f"source verification failed: {exc}")
    artifacts = {p.relative_to(output).as_posix(): digest(p.read_bytes())
                 for p in output.rglob("*") if p.is_file() and "cache" not in p.relative_to(output).parts}
    metadata = {
        "schemaVersion": 1, "captureId": uuid.uuid4().hex,
        "project": str(project), "scan": args.scan,
        "python": str(python), "testArgs": test_args, **identity,
        "extraFiles": extras, "protectedPaths": protected, "protected": contract,
        "files": {name: digest(data) for name, data in files.items()},
        "commands": results, "artifacts": artifacts, "errors": errors,
    }
    data = (json.dumps(metadata, indent=2) + "\n").encode()
    (output / "metadata.json").write_bytes(data)
    print(json.dumps({"output": str(output), "metadataSha256": digest(data), "errors": errors}))
    return 1 if errors else 0


def read_capture(path: Path) -> dict:
    path = path.resolve()
    metadata = json.loads((path / "metadata.json").read_text(encoding="utf-8"))
    if metadata["schemaVersion"] != 1:
        raise ValueError("unsupported capture schema")
    disjoint(Path(metadata["project"]), path)
    for name, expected in metadata["artifacts"].items():
        if digest(project_path(path, name).read_bytes()) != expected:
            raise ValueError(f"capture hash mismatch: {name}")
    source = path / "source"
    actual = {p.relative_to(source).as_posix() for p in source.rglob("*") if p.is_file()}
    if actual != metadata["files"].keys():
        raise ValueError("capture source inventory/hash mismatch")
    for name, expected in metadata["files"].items():
        if digest(project_path(source, name).read_bytes()) != expected:
            raise ValueError(f"capture source hash mismatch: {name}")
    validate_scan(path)
    return metadata


def check(args: argparse.Namespace) -> int:
    issues, captures = [], {}
    if args.before.resolve() == args.after.resolve():
        issues.append("before and after captures must be distinct observations")
    for label, path in (("before", args.before), ("after", args.after)):
        try:
            metadata = read_capture(path)
            issues.extend(f"{label}: {error}" for error in metadata["errors"])
            captures[label] = metadata
        except (OSError, ValueError, KeyError, TypeError) as exc:
            issues.append(f"{label} capture: {exc}")
    after = captures.get("after")
    checks_passed = (after is not None and not after["errors"]
                     and after["commands"]["tests"]["exit"] == 0
                     and not any(issue.startswith("after capture:") for issue in issues))
    if after is not None and after["commands"]["tests"]["exit"] != 0:
        issues.append("after test command failed")
    if len(captures) == 2:
        before = captures["before"]
        if before == after or (
            before.get("captureId") is not None and before["captureId"] == after.get("captureId")
        ):
            issues.append("before and after captures must be distinct observations, not copies")
        if before["protected"] != after["protected"]:
            issues.append("protected original contract files changed, disappeared, or were added")
        for field in ("project", "scan", "python", "testArgs", "binarySha256", "skillSha256", "extraFiles", "protectedPaths"):
            if before[field] != after[field]:
                issues.append(f"session input mismatch: {field}")
        if (args.before / "environment.json").read_bytes() != (args.after / "environment.json").read_bytes():
            issues.append("session input mismatch: Python environment")
    try:
        if args.review is None:
            raise ValueError("transcript review is missing")
        review_path = args.review.resolve()
        for metadata in captures.values():
            disjoint(Path(metadata["project"]), review_path)
        review = json.loads(review_path.read_text(encoding="utf-8"))
        if review.get("status") != "clean" or not isinstance(review.get("evidence"), str) or not review["evidence"].strip():
            raise ValueError("transcript review is contaminated, unreviewed, or lacks evidence")
        for label, path in (("before", args.before), ("after", args.after)):
            if review.get(label + "Sha256") != digest((path / "metadata.json").read_bytes()):
                raise ValueError(f"transcript review does not bind the {label} capture hash")
    except (OSError, ValueError, AttributeError) as exc:
        issues.append(f"review: {exc}")
    eligible = not issues and checks_passed
    print(json.dumps({"checksPassed": checks_passed, "eligibleForIsolatedComparison": eligible,
                      "issues": issues}, indent=2))
    return 0 if eligible else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    collect = commands.add_parser("capture", help="record one project state without changing scan scope")
    for name in ("project", "output", "binary", "python"):
        collect.add_argument("--" + name, type=Path, required=True)
    collect.add_argument("--protect", action="append", required=True, help="original test/contract file or directory")
    collect.add_argument("--file", action="append", default=[], help="additional project-relative non-Python input")
    collect.add_argument("--tests", required=True, help="quoted Python test arguments, e.g. '-m pytest tests'")
    collect.add_argument("--scan", default=".", help="project-relative scan target")
    collect.add_argument("--skill", type=Path, help="exact product skill supplied to the agent, if any")
    collect.add_argument("--timeout", type=float, default=300, help="per-command timeout in seconds")
    compare = commands.add_parser("check", help="gate a before/after session using external transcript review")
    compare.add_argument("--before", type=Path, required=True)
    compare.add_argument("--after", type=Path, required=True)
    compare.add_argument("--review", type=Path, help="parent-owned review JSON, bound to capture hashes")
    args = parser.parse_args()
    try:
        return capture(args) if args.command == "capture" else check(args)
    except (OSError, ValueError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
