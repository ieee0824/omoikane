"""Keep an obsolete fixture assertion distinct from arbitrary regressions."""

import copy
import unittest
from unittest.mock import patch

from scripts.acid3_expectations import (
    load_expectation,
    report_matches_expectation,
    run_matches_expectation,
)


class Acid3ExpectationTests(unittest.TestCase):
    def setUp(self):
        self.expected = load_expectation()
        self.assertIsNotNone(self.expected)
        self.sample = {key: self.expected[key] for key in ("score", "total", "index")} | {
            "log": "Timing note\n" + "\n".join(self.expected["failed_lines"]),
            "termination_reason": "completed",
            "script_errors": [], "drive_errors": [], "task_errors": [],
        }

    def test_original_fixture_hash_and_exact_legacy_failure_are_required(self):
        self.assertTrue(run_matches_expectation(self.sample, self.expected))
        self.assertEqual(self.sample["score"], 99)
        self.assertIn("Test 35 failed:", self.sample["log"])

    def test_changed_original_fixture_hash_is_rejected(self):
        with patch("scripts.acid3_expectations.hashlib.sha256") as digest:
            digest.return_value.hexdigest.return_value = "0" * 64
            self.assertIsNone(load_expectation())

    def test_missing_malformed_or_nonintegral_evidence_is_rejected(self):
        for key in self.sample:
            for value in (None, {}, False):
                with self.subTest(key=key, value=value):
                    altered = self.sample | {key: value}
                    self.assertFalse(run_matches_expectation(altered, self.expected))
        self.assertFalse(run_matches_expectation(self.sample | {"score": 99.0}, self.expected))
        self.assertFalse(run_matches_expectation(self.sample, None))

    def test_other_failures_incomplete_runs_and_normalized_scores_are_rejected(self):
        for change in (
            {"score": 100}, {"score": 98}, {"index": 99}, {"total": 99},
            {"log": ""}, {"log": self.sample["log"].replace("35", "34")},
            {"log": self.sample["log"] + "\nTest 99 failed: another error"},
            {"log": self.sample["log"] + "\nTest malformed failed: another error"},
            {"log": self.sample["log"] + "\n" + self.expected["failed_lines"][0]},
            {"termination_reason": "timeout"}, {"script_errors": ["error"]},
            {"drive_errors": ["error"]}, {"task_errors": ["error"]},
        ):
            with self.subTest(change=change):
                self.assertFalse(run_matches_expectation(self.sample | change, self.expected))

    def test_both_drive_modes_are_independently_required(self):
        report = {mode: copy.deepcopy(self.sample) for mode in ("faithful", "direct")}
        self.assertTrue(report_matches_expectation(report, self.expected))
        for mode in report:
            altered = copy.deepcopy(report)
            del altered[mode]
            self.assertFalse(report_matches_expectation(altered, self.expected))
            altered = copy.deepcopy(report)
            altered[mode]["score"] = 100
            self.assertFalse(report_matches_expectation(altered, self.expected))


if __name__ == "__main__":
    unittest.main()
