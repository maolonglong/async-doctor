import json
import os
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

from script_test_support import ROOT, diagnostic, executable, report, run_script, scanner


class HarnessTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.output = self.root / "report.json"

    def run_harness(self, stdout, *, strict=False, exit_code=0):
        binary = scanner(self.root / "scanner", stdout, exit_code)
        return run_script(
            sys.executable, ROOT / "bench/harness.py", "run", binary,
            self.root, self.output, self.root / ".venv",
            env={"BENCH_STRICT": "1" if strict else "0"},
        )

    def test_zero_findings_is_a_success_for_exit_zero_or_one(self):
        for strict in (False, True):
            for exit_code in (0, 1):
                with self.subTest(strict=strict, exit_code=exit_code):
                    result = self.run_harness(json.dumps(report()), strict=strict, exit_code=exit_code)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(result.stdout.strip().split("|")[2:], ["0", "0"])

    def test_empty_invalid_and_non_report_output_never_becomes_zero_findings(self):
        for payload in ("", " \n", "{", "not json", "null", "[]", "{}", '{"findings": {}}'):
            for strict in (False, True):
                for exit_code in (0, 1):
                    with self.subTest(payload=payload, strict=strict, exit_code=exit_code):
                        result = self.run_harness(payload, strict=strict, exit_code=exit_code)
                        self.assertNotEqual(result.returncode, 0, result.stdout)
                        self.assertEqual(result.stdout, "")
                        self.assertEqual(self.output.read_text(), payload)

    def test_true_is_not_a_scanner(self):
        result = run_script(
            sys.executable, ROOT / "bench/harness.py", "run", "/usr/bin/true",
            self.root, self.output, self.root / ".venv",
        )
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertEqual(result.stdout, "")

    def test_scan_error_cannot_be_hidden_by_valid_json(self):
        result = self.run_harness(json.dumps(report()), strict=True, exit_code=2)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exited 2", result.stderr)

    def test_strict_rejects_incomplete_omitted_and_missing_envelope(self):
        variants = [report(complete=False), report(skipped=["parse failure"])]
        for key in ("omittedFindings", "omittedCandidates", "omittedAdvice"):
            for value in (1, "0", False, -1):
                variants.append(report(**{key: value}))
        for key in (
            "complete", "schemaVersion", "environment", "omittedFindings",
            "candidates", "advice", "locationLessCandidates", "skipped",
            "sinkSites", "unusedSuppressions", "unresolvedSummary",
        ):
            data = report()
            del data[key]
            variants.append(data)
        variants.extend((
            report(schemaVersion=99), report(schemaVersion=2.0),
            report(environment=[]), report(complete=1),
        ))
        for data in variants:
            with self.subTest(data=data):
                result = self.run_harness(json.dumps(data), strict=True)
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertEqual(result.stdout, "")

    def test_normal_mode_can_time_a_partial_report(self):
        result = self.run_harness(json.dumps(report(complete=False, omittedFindings=2)))
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_normal_mode_accepts_the_legacy_minimal_count_envelope(self):
        result = self.run_harness('{"findings": [], "candidates": []}')
        self.assertEqual(result.returncode, 0, result.stderr)


class BenchRunnerTests(unittest.TestCase):
    """Run the real shell script without network, uv, or a Rust build."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        (self.root / "bench").mkdir()
        for name in ("run.sh", "harness.py"):
            shutil.copy2(ROOT / "bench" / name, self.root / "bench" / name)
        self.commit = "a" * 40
        self.manifest = self.root / "bench/repos.toml"
        self.manifest.write_text(
            '[[repo]]\nname = "demo"\nurl = "unused"\n'
            f'commit = "{self.commit}"\nscan = "pkg"\n'
            'install = ["pip", "install", "."]\n'
        )
        self.tools = self.root / "tools"
        self.tools.mkdir()
        executable(
            self.tools / "git",
            "import sys\nfrom pathlib import Path\n"
            "if sys.argv[1] == 'clone':\n"
            "    dest = Path(sys.argv[-1])\n"
            "    (dest / '.git').mkdir(parents=True)\n"
            "    (dest / 'pkg').mkdir()\n"
            "    (dest / 'pkg/app.py').write_text('async def main(): pass\\n')\n"
            f"elif sys.argv[-2:] == ['rev-parse', 'HEAD']: print({self.commit!r})\n",
        )
        executable(
            self.tools / "uv",
            "import os, sys\nfrom pathlib import Path\n"
            "if sys.argv[1] == 'venv':\n"
            "    dest = Path(sys.argv[2])\n"
            "    (dest / 'bin').mkdir(parents=True)\n"
            "    if os.environ.get('NO_ACTIVATE') != '1':\n"
            "        (dest / 'bin/activate').write_text(':\\n')\n"
            "elif os.environ.get('UV_FAIL') == '1':\n"
            "    sys.stderr.write('simulated install failure\\n')\n"
            "    raise SystemExit(7)\n",
        )
        self.state = self.root / "scan-count"
        binary = executable(
            self.root / "scanner",
            "import json, os\nfrom pathlib import Path\n"
            "state = Path(os.environ['SCAN_STATE'])\n"
            "count = int(state.read_text()) if state.exists() else 0\n"
            "state.write_text(str(count + 1))\n"
            f"data = json.loads({json.dumps(report(findings=[diagnostic()]))!r})\n"
            "if count and os.environ.get('PARITY_FAIL') == '1': data['score'] = 99\n"
            "print(json.dumps(data))\nraise SystemExit(1)\n",
        )
        self.env = {
            "PATH": str(self.tools) + os.pathsep + os.environ["PATH"],
            "ASYNC_DOCTOR_BIN": str(binary),
            "PYTHON_BIN": sys.executable,
            "BENCH_ONLY": "",
            "BENCH_FORCE_INSTALL": "",
            "UV_FAIL": "0",
            "NO_ACTIVATE": "0",
            "PARITY_FAIL": "0",
            "SCAN_STATE": str(self.state),
        }

    def run_bench(self, *, strict=False, **updates):
        self.state.unlink(missing_ok=True)
        env = dict(self.env, BENCH_STRICT="1" if strict else "0", **updates)
        return run_script("bash", self.root / "bench/run.sh", env=env)

    def test_strict_accepts_complete_identical_scans_with_findings_exit_one(self):
        result = self.run_bench(strict=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("identical=yes", result.stderr)
        rows = json.loads((self.root / "bench/.repos/results.json").read_text())
        self.assertEqual(rows[0]["findings"], 1)

    def test_install_failure_is_only_fatal_in_strict_mode(self):
        normal = self.run_bench(UV_FAIL="1")
        self.assertEqual(normal.returncode, 0, normal.stderr)
        self.assertIn("continuing", normal.stderr)
        strict = self.run_bench(strict=True, UV_FAIL="1")
        self.assertNotEqual(strict.returncode, 0, strict.stdout)
        self.assertFalse(self.state.exists(), "strict mode must not scan after failed dependency setup")

    def test_strict_does_not_trust_a_stale_install_stamp(self):
        self.assertEqual(self.run_bench().returncode, 0)
        strict = self.run_bench(strict=True, UV_FAIL="1")
        self.assertNotEqual(strict.returncode, 0, strict.stdout)
        self.assertFalse(self.state.exists())

    def test_activation_failure_is_dependency_setup_failure(self):
        strict = self.run_bench(strict=True, NO_ACTIVATE="1")
        self.assertNotEqual(strict.returncode, 0, strict.stdout)
        self.assertFalse(self.state.exists())

    def test_parity_difference_is_only_fatal_in_strict_mode(self):
        for strict in (False, True):
            with self.subTest(strict=strict):
                result = self.run_bench(strict=strict, PARITY_FAIL="1")
                self.assertIn("identical=no", result.stderr)
                self.assertEqual(result.returncode == 0, not strict, result.stdout)

    def test_strict_rejects_an_empty_selection(self):
        normal = self.run_bench(BENCH_ONLY="unknown")
        self.assertEqual(normal.returncode, 0, normal.stderr)
        strict = self.run_bench(strict=True, BENCH_ONLY="unknown")
        self.assertNotEqual(strict.returncode, 0, strict.stdout)

    def test_manifest_parse_failure_is_not_a_successful_empty_benchmark(self):
        self.manifest.write_text('[[repo]]\nname = "broken"\n')
        result = self.run_bench(strict=True)
        self.assertNotEqual(result.returncode, 0, result.stdout)


if __name__ == "__main__":
    unittest.main()
