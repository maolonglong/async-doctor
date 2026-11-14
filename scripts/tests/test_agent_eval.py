import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from script_test_support import ROOT, executable, report, run_script, scanner


SCRIPT = ROOT / "scripts/agent_eval.py"


class AgentEvaluationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.project = self.root / "project"
        (self.project / "tests").mkdir(parents=True)
        self.python = Path(sys.executable)
        self.app = self.project / "app.py"
        self.app.write_text("def answer(): return 41\n")
        (self.project / "README.md").write_text("The public answer must be 42.\n")
        (self.project / "pyproject.toml").write_text('[project]\nname = "example"\n')
        self.test = self.project / "tests/test_contract.py"
        self.original_test = (
            "import unittest\nfrom app import answer\n"
            "class Contract(unittest.TestCase):\n"
            "    def test_answer(self): self.assertEqual(answer(), 42)\n"
        )
        self.test.write_text(self.original_test)
        (self.project / "PROJECTS.md").write_text("Unrelated private project inventory\n")
        self.binary = executable(
            self.root / "scanner",
            "import json, sys\nfrom pathlib import Path\n"
            f"data = json.loads({json.dumps(report())!r})\n"
            "python = Path(sys.argv[sys.argv.index('--python') + 1])\n"
            "prefix = python.parent.parent if python.parent.name in {'bin', 'Scripts'} else python.parent\n"
            "data['environment']['prefix'] = str(prefix)\n"
            "data['fileCount'] = len(list(Path.cwd().rglob('*.py')))\n"
            "print(json.dumps(data))\n",
        )

    def capture(self, name, *, output=None, tests=None, timeout=None):
        destination = self.root / name if output is None else output
        args = [
            sys.executable, SCRIPT, "capture", "--project", self.project,
            "--output", destination, "--binary", self.binary,
            "--python", self.python, "--protect", "tests",
            "--protect", "README.md", "--file", "pyproject.toml",
            "--tests", tests or "-m unittest discover -s tests -v",
        ]
        if timeout is not None:
            args += ["--timeout", str(timeout)]
        return run_script(*args)

    def pair(self):
        before = self.capture("before")
        self.assertEqual(before.returncode, 0, before.stderr)
        self.app.write_text("def answer(): return 42\n")
        after = self.capture("after")
        self.assertEqual(after.returncode, 0, after.stderr)

    def review(self, *, status="clean", **updates):
        data = {
            "status": status,
            "evidence": "Parent-reviewed full session transcript; no outside answers read.",
            "beforeSha256": hashlib.sha256((self.root / "before/metadata.json").read_bytes()).hexdigest(),
            "afterSha256": hashlib.sha256((self.root / "after/metadata.json").read_bytes()).hexdigest(),
        }
        data.update(updates)
        path = self.root / "review.json"
        path.write_text(json.dumps(data))
        return path

    def check(self, review=None, after="after"):
        args = [
            sys.executable, SCRIPT, "check", "--before", self.root / "before",
            "--after", self.root / after,
        ]
        if review is not None:
            args += ["--review", review]
        result = run_script(*args)
        return result, json.loads(result.stdout)

    def test_external_captures_preserve_scan_scope_and_original_contract(self):
        venv = self.root / "venv"
        subprocess.run([sys.executable, "-m", "venv", "--without-pip", str(venv)], check=True, capture_output=True)
        self.python = venv / "bin/python"
        self.pair()
        for stage in ("before", "after"):
            destination = self.root / stage
            self.assertEqual(json.loads((destination / "scan.json").read_text())["fileCount"], 2)
            self.assertFalse((destination / "source/PROJECTS.md").exists())
            self.assertEqual((destination / "source/tests/test_contract.py").read_text(), self.original_test)
            self.assertEqual(json.loads((destination / "environment.json").read_text())["prefix"], str(venv))
        before = json.loads((self.root / "before/metadata.json").read_text())
        self.assertEqual(before["commands"]["tests"]["exit"], 1)
        result, data = self.check(self.review())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(data["checksPassed"])
        self.assertTrue(data["eligibleForIsolatedComparison"])

    def test_overlapping_and_symlink_aliased_outputs_fail_before_collection(self):
        alias = self.root / "project-alias"
        alias.symlink_to(self.project, target_is_directory=True)
        for path in (self.project / "evidence", self.project, self.root, alias / "evidence"):
            with self.subTest(path=path):
                result = self.capture("unused", output=path)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("disjoint", result.stderr)
        self.assertFalse((self.project / "evidence").exists())
        self.assertEqual(self.app.read_text(), "def answer(): return 41\n")
        outside = self.root / "unrelated.py"
        outside.write_text("unrelated_inventory = True\n")
        (self.project / "leak.py").symlink_to(outside)
        result = self.capture("escaped-source")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "escaped-source").exists())

    def test_changed_removed_or_added_protected_tests_cannot_earn_eligibility(self):
        # Keep a real contract test: Python 3.12 exits 5 for an empty suite.
        (self.project / "tests/test_retained.py").write_text(self.original_test)
        self.assertEqual(self.capture("before").returncode, 0)
        self.app.write_text("def answer(): return 42\n")
        for change in ("changed", "removed", "added"):
            with self.subTest(change=change):
                self.test.write_text(self.original_test)
                extra = self.project / "tests/test_extra.py"
                extra.unlink(missing_ok=True)
                if change == "changed":
                    self.test.write_text(self.original_test.replace("self.assertEqual(answer(), 42)", "self.assertTrue(True)"))
                elif change == "removed":
                    self.test.unlink()
                else:
                    extra.write_text("import unittest\nclass Extra(unittest.TestCase):\n    def test_extra(self): pass\n")
                after = self.root / "after"
                if after.exists():
                    after = self.root / ("after-" + change)
                capture = self.capture("unused", output=after)
                self.assertEqual(capture.returncode, 0, capture.stdout + capture.stderr)
                review = json.loads(self.review().read_text())
                review["afterSha256"] = hashlib.sha256((after / "metadata.json").read_bytes()).hexdigest()
                (self.root / "review.json").write_text(json.dumps(review))
                result, data = self.check(self.root / "review.json", after=after.name)
                self.assertNotEqual(result.returncode, 0)
                self.assertTrue(data["checksPassed"])
                self.assertFalse(data["eligibleForIsolatedComparison"])
                self.assertIn("protected", " ".join(data["issues"]))

    def test_missing_contaminated_unbound_or_empty_review_is_not_eligible(self):
        self.pair()
        variants = [None, {"status": "contaminated"}, {"afterSha256": "0" * 64}, {"evidence": ""}]
        for variant in variants:
            with self.subTest(review=variant):
                result, data = self.check(None if variant is None else self.review(**variant))
                self.assertNotEqual(result.returncode, 0)
                self.assertTrue(data["checksPassed"])
                self.assertFalse(data["eligibleForIsolatedComparison"])
                self.assertIn("review", " ".join(data["issues"]))

    def test_one_capture_cannot_stand_for_both_observations(self):
        self.pair()
        copied = self.root / "after-copy"
        shutil.copytree(self.root / "after", copied)
        for name in ("after", "after-copy", "reformatted-copy"):
            with self.subTest(after=name):
                path = self.root / "after" if name == "after" else copied
                if name == "reformatted-copy":
                    metadata = json.loads((path / "metadata.json").read_text())
                    (path / "metadata.json").write_text(json.dumps(metadata, sort_keys=True))
                review = self.review(
                    beforeSha256=hashlib.sha256((self.root / "after/metadata.json").read_bytes()).hexdigest(),
                    afterSha256=hashlib.sha256((path / "metadata.json").read_bytes()).hexdigest(),
                )
                result = run_script(
                    sys.executable, SCRIPT, "check", "--before", self.root / "after",
                    "--after", path, "--review", review,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("distinct", " ".join(json.loads(result.stdout)["issues"]))

        # Identical source and command output from two actual runs remain valid.
        for name in ("quiet-before", "quiet-after"):
            result = self.capture(name, tests="-c pass")
            self.assertEqual(result.returncode, 0, result.stderr)
        review = self.review(**{
            label + "Sha256": hashlib.sha256((self.root / name / "metadata.json").read_bytes()).hexdigest()
            for label, name in (("before", "quiet-before"), ("after", "quiet-after"))
        })
        result = run_script(
            sys.executable, SCRIPT, "check", "--before", self.root / "quiet-before",
            "--after", self.root / "quiet-after", "--review", review,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(json.loads(result.stdout)["eligibleForIsolatedComparison"])

    def test_captured_source_or_command_output_tampering_is_rejected(self):
        self.pair()
        review = self.review()
        for name in ("source/app.py", "tests.stdout", "metadata.json"):
            with self.subTest(name=name):
                path = self.root / "after" / name
                original = path.read_bytes()
                if name == "metadata.json":
                    metadata = json.loads(original)
                    del metadata["errors"]
                    path.write_text(json.dumps(metadata))
                else:
                    path.write_bytes(original + b"\nchanged\n")
                result, data = self.check(review)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(data["eligibleForIsolatedComparison"])
                self.assertIn("hash", " ".join(data["issues"]))
                path.write_bytes(original)

    def test_scanner_must_report_the_selected_python_environment(self):
        self.assertEqual(self.capture("before").returncode, 0)
        self.app.write_text("def answer(): return 42\n")
        for name, prefix in (("missing", None), ("mismatch", str(self.root / "other-python"))):
            with self.subTest(prefix=prefix):
                data = report()
                if prefix is not None:
                    data["environment"]["prefix"] = prefix
                scanner(self.binary, json.dumps(data))
                result = self.capture(name)
                self.assertNotEqual(result.returncode, 0)
                path = self.root / name / "metadata.json"
                metadata = json.loads(path.read_text())
                self.assertIn("environment", " ".join(metadata["errors"]))
                # Older collectors did not record this mismatch as an error.
                metadata["errors"] = []
                path.write_text(json.dumps(metadata))
                result, checked = self.check(after=name)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(checked["checksPassed"])
                self.assertIn("environment", " ".join(checked["issues"]))

    def test_invalid_incomplete_or_failed_scans_keep_raw_output_and_fail(self):
        valid = report()
        valid["environment"]["prefix"] = sys.prefix
        variants = [("", 0), ("{", 0), (json.dumps({**valid, "complete": False}), 0), (json.dumps(valid), 2)]
        for index, (payload, code) in enumerate(variants):
            with self.subTest(payload=payload, code=code):
                scanner(self.binary, payload, code)
                result = self.capture("invalid-" + str(index))
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual((self.root / ("invalid-" + str(index)) / "scan.json").read_text(), payload)
        scanner(self.binary, json.dumps(valid), after_output="import time; time.sleep(2)")
        result = self.capture("timeout", timeout=0.3)
        self.assertNotEqual(result.returncode, 0)
        metadata = json.loads((self.root / "timeout/metadata.json").read_text())
        self.assertTrue(metadata["commands"]["scan"]["timedOut"])
        self.assertIn('"schemaVersion": 2', (self.root / "timeout/scan.json").read_text())

    def test_different_test_commands_or_binaries_do_not_form_a_matched_session(self):
        self.assertEqual(self.capture("before").returncode, 0)
        self.app.write_text("def answer(): return 42\n")
        self.assertEqual(self.capture("after", tests="-m unittest discover -s tests").returncode, 0)
        result, data = self.check(self.review())
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("input", " ".join(data["issues"]))
        self.binary.write_text(self.binary.read_text() + "\n# another binary\n")
        self.assertEqual(self.capture("other-binary").returncode, 0)
        review = self.review(afterSha256=hashlib.sha256((self.root / "other-binary/metadata.json").read_bytes()).hexdigest())
        result, data = self.check(review, after="other-binary")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("input", " ".join(data["issues"]))

    def test_source_mutation_during_a_check_is_not_a_valid_capture(self):
        self.test.write_text(
            "import unittest\nfrom pathlib import Path\n"
            "class Mutation(unittest.TestCase):\n"
            "    def test_mutates(self): Path('app.py').write_text('changed = True\\n')\n"
        )
        result = self.capture("mutation")
        self.assertNotEqual(result.returncode, 0)
        metadata = json.loads((self.root / "mutation/metadata.json").read_text())
        self.assertEqual(metadata["commands"]["tests"]["exit"], 0)
        self.assertIn("source", " ".join(metadata["errors"]))

    def test_capture_never_overwrites_previous_evidence(self):
        self.assertEqual(self.capture("before").returncode, 0)
        original = (self.root / "before/source/app.py").read_bytes()
        self.app.write_text("def answer(): return 42\n")
        result = self.capture("before")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((self.root / "before/source/app.py").read_bytes(), original)


if __name__ == "__main__":
    unittest.main()
