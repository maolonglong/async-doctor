"""Self-check for template/scripts/check_docs.py. Run: python3 tests/test_check_docs.py"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SKILL = Path(__file__).resolve().parent.parent
TEMPLATE = SKILL / "template"
SCRIPT = TEMPLATE / "scripts" / "check_docs.py"

DECISION = """# {title}

{status}

## Problem

x

## Decision

x

## Alternatives considered

x
{extra}"""


def run(root: Path) -> tuple[int, list[str]]:
    p = subprocess.run([sys.executable, str(SCRIPT), str(root)], capture_output=True, text=True)
    return p.returncode, [l for l in p.stdout.splitlines() if l.startswith("error:")]


class Repo:
    """A minimal filled-in docs tree in a temp dir (no git, so the os.walk path runs)."""

    def __init__(self) -> None:
        self.dir = Path(tempfile.mkdtemp())
        (self.dir / "docs" / "decisions").mkdir(parents=True)
        (self.dir / "AGENTS.md").write_text("# Map\n\nSee [docs](docs/README.md).\n")
        (self.dir / "docs" / "README.md").write_text("# Docs\n")

    def write(self, rel: str, text: str) -> None:
        p = self.dir / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)

    def decision(self, name: str, status: str, extra: str = "") -> None:
        self.write(f"docs/decisions/{name}.md", DECISION.format(title=name, status=status, extra=extra))

    def errors(self) -> list[str]:
        return run(self.dir)[1]

    def cleanup(self) -> None:
        shutil.rmtree(self.dir)


class CheckDocsTest(unittest.TestCase):
    def setUp(self) -> None:
        self.repo = Repo()

    def tearDown(self) -> None:
        self.repo.cleanup()

    def assertRules(self, want: list[str]) -> None:
        got = sorted(e.split(": ")[2] for e in self.repo.errors())
        self.assertEqual(got, sorted(want), "\n".join(self.repo.errors()))

    def test_template_fails_only_on_unfilled_guidance(self) -> None:
        tmp = Path(tempfile.mkdtemp())
        try:
            shutil.copytree(TEMPLATE, tmp / "t", symlinks=True)
            code, errors = run(tmp / "t")
            self.assertEqual(code, 1)
            self.assertTrue(errors)
            self.assertEqual({e.split(": ")[2] for e in errors}, {"unfilled-template"}, "\n".join(errors))
        finally:
            shutil.rmtree(tmp)

    def test_clean_repo_passes(self) -> None:
        self.assertEqual(run(self.repo.dir), (0, []))

    def test_legal_statuses_pass(self) -> None:
        self.repo.decision("a", "Status: proposed")
        self.repo.decision("b", "Status: accepted")
        self.repo.decision("c", "Status: rejected: too slow on large repos")
        self.repo.decision("d", "Status: superseded by [b](b.md)")
        self.assertRules([])

    def test_illegal_status_values(self) -> None:
        self.repo.decision("a", "Status: implemented")
        self.repo.decision("b", "Status: rejected")
        self.repo.decision("c", "Status: Accepted")
        self.repo.decision("d", "No status here")
        self.assertRules(["decision-status"] * 4)

    def test_superseded_link_must_resolve(self) -> None:
        self.repo.decision("a", "Status: superseded by [gone](gone.md)")
        self.repo.decision("b", "Status: superseded by the new one")
        self.assertRules(["broken-link", "decision-superseded", "decision-superseded"])

    def test_accepted_decision_with_plan_sections(self) -> None:
        self.repo.decision("a", "Status: accepted", "\n## Plan\n\n1. x\n\n## Acceptance criteria\n\n- x\n")
        self.repo.decision("b", "Status: proposed", "\n## Plan\n\n1. x\n")
        self.assertRules(["decision-plan", "decision-plan"])

    def test_missing_alternatives(self) -> None:
        self.repo.write("docs/decisions/a.md", "# a\n\nStatus: accepted\n\n## Decision\n\nx\n")
        self.assertRules(["decision-alternatives"])

    def test_skill_links(self) -> None:
        for n in ("good", "missing", "wrong"):
            self.repo.write(f".agents/skills/{n}/SKILL.md", "---\nname: x\n---\n")
        links = self.repo.dir / ".claude" / "skills"
        links.mkdir(parents=True)
        os.symlink("../../.agents/skills/good", links / "good")
        os.symlink("../../.agents/skills/missing-typo", links / "wrong")
        os.symlink("../../.agents/skills/orphan", links / "orphan")
        errors = self.repo.errors()
        self.assertEqual(len(errors), 3, "\n".join(errors))
        for name in ("missing", "wrong", "orphan"):
            self.assertTrue(any(f".claude/skills/{name}:" in e and "skill-link" in e for e in errors), name)

    def test_skills_without_claude_dir(self) -> None:
        self.repo.write(".agents/skills/a/SKILL.md", "x\n")
        self.assertRules(["skill-link"])


if __name__ == "__main__":
    unittest.main()
