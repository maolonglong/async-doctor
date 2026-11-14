import copy
import json
import sys
import tempfile
import unittest
from pathlib import Path

from script_test_support import ROOT, diagnostic, report, run_script, scanner, write_report


SCRIPT = ROOT / "scripts/audit_compare.py"


class AuditDiffTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.base = self.root / "base"
        self.candidate = self.root / "candidate"
        self.base.mkdir()
        self.candidate.mkdir()
        self.base_report = write_report(self.base / "demo.i.json")
        self.new_report = write_report(self.candidate / "demo.i.json")

    def diff(self, *, strict=False, baseline=None, candidate=None):
        args = [sys.executable, SCRIPT, "diff", baseline or self.base, candidate or self.candidate]
        if strict:
            args.append("--strict")
        return run_script(*args)

    def test_identical_zero_finding_reports_pass_both_modes(self):
        for strict in (False, True):
            with self.subTest(strict=strict):
                result = self.diff(strict=strict)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_normal_mode_keeps_legacy_reports_comparable(self):
        write_report(self.base_report, {"findings": []})
        write_report(self.new_report, {"findings": []})
        normal = self.diff()
        self.assertEqual(normal.returncode, 0, normal.stderr)
        strict = self.diff(strict=True)
        self.assertNotEqual(strict.returncode, 0)

    def test_empty_invalid_non_report_and_malformed_diagnostics_fail(self):
        for payload in ("", "{", "null", "[]", "{}", '{"findings": {}}', '{"findings": [{}]}'):
            for target in (self.base_report, self.new_report):
                with self.subTest(payload=payload, target=target):
                    target.write_text(payload)
                    result = self.diff()
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    write_report(target)

    def test_invalid_candidate_is_not_skipped_when_baseline_is_missing(self):
        self.base_report.unlink()
        self.new_report.write_text("")
        result = self.diff()
        self.assertNotEqual(result.returncode, 0, result.stdout)

    def test_add_remove_and_renumber_findings_are_strict_failures(self):
        finding = diagnostic()
        for before, after, marker in (
            ([], [finding], "+ demo"),
            ([finding], [], "- demo"),
            ([finding], [dict(finding, id="new-id")], "~ demo"),
        ):
            write_report(self.base_report, report(findings=before))
            write_report(self.new_report, report(findings=after))
            for strict in (False, True):
                with self.subTest(marker=marker, strict=strict):
                    result = self.diff(strict=strict)
                    self.assertIn(marker, result.stdout)
                    self.assertEqual(result.returncode == 0, not strict, result.stderr)

    def test_same_id_finding_payload_change_is_visible_and_strict(self):
        write_report(self.base_report, report(findings=[diagnostic()]))
        write_report(self.new_report, report(findings=[diagnostic(line=10)]))
        for strict in (False, True):
            with self.subTest(strict=strict):
                result = self.diff(strict=strict)
                self.assertIn("! demo", result.stdout)
                self.assertIn('"before"', result.stdout)
                self.assertEqual(result.returncode == 0, not strict, result.stderr)

    def test_renumber_matching_does_not_hide_an_extra_diagnostic_at_the_same_location(self):
        write_report(self.base_report, report(findings=[diagnostic("old")]))
        write_report(self.new_report, report(findings=[diagnostic("new-1"), diagnostic("new-2")]))
        result = self.diff()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.count("+ demo"), 1)
        self.assertEqual(result.stdout.count("~ demo"), 1)

    def test_candidate_and_locationless_changes_are_visible_and_strict(self):
        for tier in ("candidates", "locationLessCandidates"):
            item = diagnostic()
            item["reason"] = "conditional"
            if tier == "locationLessCandidates":
                item["entry"]["file"] = ""
                item["entry"]["line"] = 0
                item["chain"] = []
            write_report(self.new_report, report(**{tier: [item]}))
            for strict in (False, True):
                with self.subTest(tier=tier, strict=strict):
                    result = self.diff(strict=strict)
                    self.assertIn(f"+ {tier} demo", result.stdout)
                    self.assertEqual(result.returncode == 0, not strict, result.stderr)

    def test_advice_add_remove_renumber_and_content_changes_are_strict(self):
        advice = dict(diagnostic(), category="performance")
        changed = copy.deepcopy(advice)
        changed["fix"]["hint"] = "use another async API"
        for before, after, marker in (
            ([], [advice], "+ advice demo"),
            ([advice], [], "- advice demo"),
            ([advice], [dict(advice, id="new-id")], "~ advice demo"),
            ([advice], [changed], "! advice demo"),
        ):
            write_report(self.base_report, report(advice=before))
            write_report(self.new_report, report(advice=after))
            for strict in (False, True):
                with self.subTest(marker=marker, strict=strict):
                    result = self.diff(strict=strict)
                    self.assertIn(marker, result.stdout)
                    self.assertEqual(result.returncode == 0, not strict, result.stderr)

    def test_report_item_order_is_not_a_diagnostic_difference(self):
        first = dict(diagnostic(), category="performance")
        second = dict(diagnostic("id-2", 10), category="correctness")
        write_report(self.base_report, report(advice=[first, second]))
        write_report(self.new_report, report(advice=[second, first]))
        result = self.diff(strict=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_strict_validates_both_envelopes_without_changing_loose_partial_comparison(self):
        variants = [report(complete=False), report(skipped=["parse failure"]), report(environment=[])]
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
        variants.extend((report(schemaVersion=2.0), report(schemaVersion=99), report(complete=1)))
        for data in variants:
            for target in (self.base_report, self.new_report):
                with self.subTest(data=data, target=target):
                    write_report(target, data)
                    result = self.diff(strict=True)
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    write_report(target)
        write_report(self.new_report, report(complete=False, omittedAdvice=1))
        normal = self.diff()
        self.assertEqual(normal.returncode, 0, normal.stderr)

    def test_missing_baseline_is_only_fatal_in_strict_mode(self):
        self.base_report.unlink()
        for strict in (False, True):
            with self.subTest(strict=strict):
                result = self.diff(strict=strict)
                self.assertIn("nobase demo", result.stdout)
                self.assertEqual(result.returncode == 0, not strict, result.stderr)

    def test_missing_candidate_is_discovered_from_directory_and_template_baselines(self):
        write_report(self.base / "missing.i.json")
        for baseline in (self.base, str(self.base / "{L}.i.json")):
            normal = self.diff(baseline=baseline)
            self.assertEqual(normal.returncode, 0, normal.stderr)
            strict = self.diff(strict=True, baseline=baseline)
            self.assertNotEqual(strict.returncode, 0, strict.stdout)
            self.assertIn("missing candidate missing", strict.stdout)

    def test_template_inventory_uses_the_label_placeholder_not_the_filename_stem(self):
        self.base_report.unlink()
        write_report(self.base / "run-demo.i.json")
        write_report(self.base / "run-missing.i.json")
        result = self.diff(strict=True, baseline=str(self.base / "run-{L}.i.json"))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing candidate missing", result.stdout)
        self.assertNotIn("nobase demo", result.stdout)

    def test_empty_or_nonexistent_candidate_directory_is_not_a_clean_audit(self):
        self.new_report.unlink()
        for strict in (False, True):
            for candidate in (self.candidate, self.root / "absent"):
                with self.subTest(strict=strict, candidate=candidate):
                    result = self.diff(strict=strict, candidate=candidate)
                    self.assertNotEqual(result.returncode, 0)

    def test_exit_sidecar_rejects_a_failed_scan_even_with_valid_json(self):
        for target in (self.base, self.candidate):
            for status in ("2", "-9", "timeout", "garbage"):
                with self.subTest(target=target, status=status):
                    sidecar = target / "demo.exit"
                    sidecar.write_text(status + "\n")
                    strict = self.diff(strict=True)
                    self.assertNotEqual(strict.returncode, 0, strict.stdout)
                    normal = self.diff()
                    self.assertEqual(normal.returncode, 0, normal.stderr)
                    sidecar.unlink()

    def test_duplicate_diagnostic_ids_are_not_silently_dropped(self):
        write_report(self.new_report, report(findings=[diagnostic(), diagnostic()]))
        result = self.diff()
        self.assertNotEqual(result.returncode, 0, result.stdout)

    def test_duplicate_report_labels_are_rejected_in_strict_mode(self):
        write_report(self.candidate / "demo.json")
        result = self.diff(strict=True)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("duplicate", result.stdout + result.stderr)

    def test_single_files_can_be_compared_with_different_names(self):
        other = write_report(self.root / "other.json")
        result = self.diff(strict=True, baseline=self.base_report, candidate=other)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_one_baseline_file_cannot_silently_cover_multiple_projects_in_strict_mode(self):
        write_report(self.candidate / "another.json")
        normal = self.diff(baseline=self.base_report)
        self.assertEqual(normal.returncode, 0, normal.stderr)
        strict = self.diff(strict=True, baseline=self.base_report)
        self.assertNotEqual(strict.returncode, 0)

    def test_shorthand_accepts_strict_without_losing_compatibility(self):
        result = run_script(sys.executable, SCRIPT, self.base, "head", self.candidate, "--strict")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("head", result.stdout)
        write_report(self.new_report, report(findings=[diagnostic()]))
        changed = run_script(sys.executable, SCRIPT, self.base, "head", self.candidate, "--strict")
        self.assertNotEqual(changed.returncode, 0)
        self.assertIn("+ demo", changed.stdout)


class AuditRunTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        (self.root / "scan").mkdir()
        self.jobs = self.root / "jobs.txt"
        self.jobs.write_text("demo scan\n")
        self.base = self.root / "baseline"
        write_report(self.base / "demo.i.json")
        self.out = self.root / "out"
        self.binary = scanner(self.root / "scanner", json.dumps(report()))

    def run_audit(self, *, strict=False, baseline=None, timeout=10):
        args = [
            sys.executable, SCRIPT, "run", "--jobs", self.jobs, "--root", self.root,
            "--baseline", baseline or self.base, "--binary", self.binary,
            "--out", self.out, "--timeout", timeout,
        ]
        if strict:
            args.append("--strict")
        return run_script(*args)

    def test_strict_accepts_successful_exit_zero_and_findings_exit_one(self):
        for code, data in ((0, report()), (1, report(findings=[diagnostic()]))):
            with self.subTest(code=code):
                scanner(self.binary, json.dumps(data), code)
                write_report(self.base / "demo.i.json", data)
                result = self.run_audit(strict=True)
                self.assertEqual(result.returncode, 0, result.stderr + result.stdout)

    def test_strict_rejects_candidate_and_baseline_scan_failure_with_valid_stdout(self):
        scanner(self.binary, json.dumps(report()), 2)
        normal = self.run_audit()
        self.assertEqual(normal.returncode, 0, normal.stderr)
        strict = self.run_audit(strict=True)
        self.assertNotEqual(strict.returncode, 0)
        self.assertIn("exited 2", strict.stdout + strict.stderr)
        failed_base = scanner(self.root / "baseline-scanner", json.dumps(report()), 2)
        scanner(self.binary, json.dumps(report()))
        strict = self.run_audit(strict=True, baseline=failed_base)
        self.assertNotEqual(strict.returncode, 0)
        self.assertIn("exited 2", strict.stdout + strict.stderr)

    def test_missing_declared_venv_only_blocks_strict_runs(self):
        self.jobs.write_text("demo scan absent-venv\n")
        normal = self.run_audit()
        self.assertEqual(normal.returncode, 0, normal.stderr)
        strict = self.run_audit(strict=True)
        self.assertNotEqual(strict.returncode, 0)
        self.assertIn("venv", strict.stdout + strict.stderr)

    def test_skipped_job_cannot_be_masked_by_stale_output(self):
        write_report(self.out / "demo.i.json")
        self.jobs.write_text("demo absent-scan\n")
        strict = self.run_audit(strict=True)
        self.assertNotEqual(strict.returncode, 0)
        self.assertIn("missing", strict.stdout + strict.stderr)

    def test_stale_unlisted_reports_and_empty_jobs_cannot_satisfy_strict_coverage(self):
        write_report(self.out / "old.i.json")
        write_report(self.base / "old.i.json")
        result = self.run_audit(strict=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("stale", result.stderr)
        (self.out / "old.i.json").unlink()
        (self.base / "old.i.json").unlink()
        self.jobs.write_text("# no selected jobs\n")
        result = self.run_audit(strict=True)
        self.assertNotEqual(result.returncode, 0)

    def test_missing_binary_is_a_scan_failure_without_a_traceback(self):
        self.binary.unlink()
        result = self.run_audit(strict=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("could not run", result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_timeout_is_reported_without_a_traceback(self):
        scanner(self.binary, json.dumps(report()), after_output="import time; time.sleep(3)")
        for strict in (False, True):
            with self.subTest(strict=strict):
                result = self.run_audit(strict=strict, timeout=1)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("timeout", (result.stdout + result.stderr).lower())
                self.assertNotIn("Traceback", result.stderr)


if __name__ == "__main__":
    unittest.main()
