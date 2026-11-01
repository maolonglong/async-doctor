#!/usr/bin/env python3
"""Mechanical checks for the repo's agent docs. Zero config: edit the constants below.

Usage: python3 scripts/check_docs.py [repo_root]
Exit 1 on any failure; warnings never fail. Each message ends with how to fix it.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import time
from pathlib import Path

LINE_BUDGETS = {"AGENTS.md": 100, "ARCHITECTURE.md": 400}
DEFAULT_DOC_BUDGET = 500
SOURCE_WARN_LINES = 800
STALE_PLAN_DAYS = 14
PLAN_SECTIONS = ["Purpose", "Progress", "Surprises", "Decision log", "Validation", "Outcomes"]
DECISION_STATUS = re.compile(r"^Status: (proposed|accepted|rejected|superseded)\b", re.M)
SKIP_DIRS = {".git", "node_modules", "target", "vendor", "third_party", ".venv", "venv", "dist", "build", "fixtures", "snapshots"}
SOURCE_EXTS = {".py", ".rs", ".go", ".ts", ".tsx", ".js", ".jsx", ".java", ".kt", ".swift", ".c", ".cc", ".cpp", ".h"}
PATH_EXTS = {".md", ".py", ".rs", ".go", ".ts", ".tsx", ".js", ".toml", ".json", ".yml", ".yaml", ".sh", ".txt", ".lock", ".cfg", ".ini"}

LINK = re.compile(r"(?<!!)\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
CODE_SPAN = re.compile(r"`([^`\n]+)`")
HEADING = re.compile(r"^#{1,6}\s+(.*?)\s*#*\s*$")
EXPLICIT_ID = re.compile(r"""(?:\bid=["']([^"']+)["']|\{#([\w-]+)\})""")

failures: list[str] = []
warnings: list[str] = []


def fail(path: Path, line: int, rule: str, msg: str) -> None:
    failures.append(f"{path}:{line}: {rule}: {msg}")


def warn(path: Path, line: int, rule: str, msg: str) -> None:
    warnings.append(f"{path}:{line}: {rule}: {msg}")


def repo_files(root: Path) -> list[Path]:
    try:
        out = subprocess.run(["git", "ls-files", "-co", "--exclude-standard"], cwd=root,
                             capture_output=True, text=True, check=True).stdout
        files = [Path(p) for p in out.splitlines() if p]
    except (OSError, subprocess.CalledProcessError):
        files = []
        for dirpath, dirnames, filenames in os.walk(root):
            dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
            files += [Path(dirpath, f).relative_to(root) for f in filenames]
    return [f for f in files if not SKIP_DIRS.intersection(f.parts[:-1]) and (root / f).is_file()]


def prose_lines(text: str):
    """Yield (line_no, line) outside fenced code blocks."""
    fenced = False
    for i, line in enumerate(text.splitlines(), 1):
        if line.lstrip().startswith(("```", "~~~")):
            fenced = not fenced
            continue
        if not fenced:
            yield i, line


def slug(heading: str) -> str:
    s = re.sub(r"[`*\[\]()]", "", heading).strip().lower()
    s = re.sub(r"[^\w\- ]", "", s)
    return s.replace(" ", "-")


_anchor_cache: dict[Path, set[str]] = {}


def anchors(path: Path) -> set[str]:
    if path not in _anchor_cache:
        _anchor_cache[path] = _anchors(path)
    return _anchor_cache[path]


def _anchors(path: Path) -> set[str]:
    seen: dict[str, int] = {}
    out = set()
    for _, line in prose_lines(path.read_text(encoding="utf-8", errors="replace")):
        out.update(a or b for a, b in EXPLICIT_ID.findall(line))
        m = HEADING.match(line)
        if m:
            base = slug(EXPLICIT_ID.sub("", m.group(1)))
            n = seen.get(base, 0)
            seen[base] = n + 1
            out.add(base if n == 0 else f"{base}-{n}")
    return out


def is_doc(f: Path) -> bool:
    return f.suffix == ".md" and (len(f.parts) == 1 or f.parts[0] == "docs" or f.name == "AGENTS.md")


def check_links(root: Path, f: Path, text: str) -> None:
    for no, line in prose_lines(text):
        for target in LINK.findall(CODE_SPAN.sub("", line)):
            if re.match(r"^[a-z][a-z0-9+.-]*:", target) or "<" in target:
                continue
            path_part, _, anchor = target.partition("#")
            dest = (root / path_part.lstrip("/")) if path_part.startswith("/") else (root / f).parent / path_part
            dest = dest if path_part else root / f
            if not dest.exists():
                fail(f, no, "broken-link", f"`{target}` does not exist. Fix the path or remove the link.")
            elif anchor and dest.suffix == ".md" and anchor.lower() not in anchors(dest):
                fail(f, no, "broken-anchor", f"`#{anchor}` is not a heading in {os.path.relpath(dest, root)}. Point at an existing heading.")


def check_code_paths(root: Path, f: Path, text: str, known: set[str]) -> None:
    for no, line in prose_lines(text):
        for tok in CODE_SPAN.findall(line):
            tok = tok.strip().rstrip("/")
            if not tok or any(c in tok for c in " <>*$()=:{}|,'\"") or tok.startswith(("-", "@", "~")):
                continue
            if "/" not in tok and Path(tok).suffix not in PATH_EXTS:
                continue
            if tok in known or any(k.endswith("/" + tok) for k in known):
                continue
            fail(f, no, "stale-path", f"`{tok}` names a path that no longer exists. Update the doc to the current name, or drop the backticks if it is not a path.")


def check_structure(root: Path, f: Path, text: str) -> None:
    parts = f.parts
    if f.name in ("README.md", "_template.md") or len(parts) < 3 or parts[0] != "docs":
        return
    headings = [m.group(1) for _, l in prose_lines(text) if (m := HEADING.match(l)) and l.startswith("## ")]
    if parts[1] == "plans" and len(parts) == 4 and parts[2] in ("active", "completed"):
        for want in PLAN_SECTIONS:
            if not any(h.lower().startswith(want.lower()) for h in headings):
                fail(f, 1, "plan-section", f"missing `## {want}`. Copy the section from docs/plans/active/_template.md.")
        if parts[2] == "active":
            check_stale_plan(root, f)
    elif parts[1] == "decisions" and len(parts) == 3:
        if not DECISION_STATUS.search("\n".join(text.splitlines()[:10])):
            fail(f, 1, "decision-status", "no `Status: proposed|accepted|rejected|superseded` line under the title. Add one.")
        if not any(h.lower().startswith("alternatives considered") for h in headings):
            fail(f, 1, "decision-alternatives", "missing `## Alternatives considered`. Record the options that were actually weighed.")


def check_stale_plan(root: Path, f: Path) -> None:
    try:
        ts = subprocess.run(["git", "log", "-1", "--format=%ct", "--", str(f)], cwd=root,
                            capture_output=True, text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return
    if ts and (time.time() - int(ts)) / 86400 > STALE_PLAN_DAYS:
        warn(f, 1, "stale-plan", f"untouched for over {STALE_PLAN_DAYS} days. Update Progress, finish it, or move it to completed/.")


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
    files = repo_files(root)
    known = {f.as_posix() for f in files} | {p.as_posix() for f in files for p in f.parents if str(p) != "."}
    docs = [f for f in files if is_doc(f)]
    index = root / "docs" / "README.md"
    index_text = index.read_text(encoding="utf-8", errors="replace") if index.exists() else ""

    for f in docs:
        text = (root / f).read_text(encoding="utf-8", errors="replace")
        lines = text.count("\n") + 1
        budget = LINE_BUDGETS.get(f.as_posix(), DEFAULT_DOC_BUDGET)
        if lines > budget:
            fail(f, budget, "size-budget", f"{lines} lines > {budget}. Move detail to its home doc (see docs/README.md) or split by topic.")
        if f.name != "_template.md":
            for no, line in prose_lines(text):
                if "<!-- guide:" in CODE_SPAN.sub("", line):
                    fail(f, no, "unfilled-template", "template guidance left in place. Fill the section, then delete the comment.")
                    break
        check_links(root, f, text)
        if f.as_posix() in LINE_BUDGETS:
            check_code_paths(root, f, text, known)
        check_structure(root, f, text)
        if index.exists() and len(f.parts) == 2 and f.parts[0] == "docs" and f.name != "README.md" \
                and f"({f.name}" not in index_text:
            fail(f, 1, "orphan-doc", "not linked from docs/README.md. Add a one-line entry there.")

    for f in files:
        if f.suffix in SOURCE_EXTS and not is_doc(f):
            try:
                n = (root / f).read_text(encoding="utf-8", errors="replace").count("\n")
            except OSError:
                continue
            if n > SOURCE_WARN_LINES:
                warn(f, n, "file-size", f"{n} lines > {SOURCE_WARN_LINES}. Split by responsibility.")

    for w in warnings:
        print("warning:", w)
    for e in failures:
        print("error:", e)
    print(f"check_docs: {len(docs)} docs, {len(failures)} errors, {len(warnings)} warnings")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
