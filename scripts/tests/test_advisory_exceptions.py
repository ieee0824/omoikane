import datetime
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("exceptions", Path(__file__).parents[1] / "check-advisory-exceptions.py")
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)


class ExceptionPolicyTest(unittest.TestCase):
    def config(self, reason):
        return {"advisories": {"ignore": [{"id": "RUSTSEC-2024-0436", "reason": reason}]}}

    def test_expiry_boundary(self):
        config = self.config("Build-time macro compatibility review. review_by=2027-01-07")
        self.assertEqual(policy.validate(config, datetime.date(2027, 1, 6)), [])
        self.assertTrue(policy.validate(config, datetime.date(2027, 1, 7)))

    def test_vulnerability_cannot_be_excepted(self):
        config = self.config("HTTP dependency compatibility review. review_by=2027-01-07")
        config["advisories"]["ignore"][0]["id"] = "RUSTSEC-2026-0007"
        self.assertTrue(policy.validate(config, datetime.date(2026, 10, 7)))

    def test_unreasoned_and_invalid_exceptions(self):
        for config in [{"advisories": {"ignore": ["RUSTSEC-2024-0436"]}},
                       self.config("review_by=2027-01-07"),
                       self.config(None),
                       {"advisories": {"ignore": [{"id": 123, "reason": "Build-time macro compatibility review. review_by=2027-01-07"}]}},
                       self.config("Build-time macro compatibility review. review_by=2027-02-30")]:
            with self.subTest(config=config):
                self.assertTrue(policy.validate(config, datetime.date(2026, 10, 7)))
