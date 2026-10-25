"""Validate raw Acid3 reports against the pinned Selectors 4 expectation.

The fixture and its raw score/log remain unchanged. This accepts only the
specific obsolete root-element assertion, not arbitrary 99-point reports.
"""

import hashlib
import json
from pathlib import Path


def load_expectation():
    """Read the repository expectation and verify its original fixture hash."""
    root = Path(__file__).resolve().parents[1] / "tests/fixtures/acid3"
    try:
        expected = json.loads((root / "selectors4-expectation.json").read_text())
        digest = hashlib.sha256((root / "acid3.html").read_bytes()).hexdigest()
        if expected.get("profile") == "selectors-4" and expected.get("fixture_sha256") == digest:
            return expected
    except (OSError, ValueError, AttributeError):
        pass
    return None


def run_matches_expectation(run, expected):
    """Pure comparison of explicit raw run data and expected score/failures."""
    if not isinstance(run, dict) or not isinstance(expected, dict):
        return False
    if not isinstance(run.get("log"), str):
        return False
    failures = [line for line in run["log"].splitlines()
                if line.startswith("Test ") and " failed:" in line]
    return (
        all(type(run.get(key)) is int and run[key] == expected.get(key)
            for key in ("score", "total", "index"))
        and failures == expected.get("failed_lines")
        and run.get("termination_reason") == "completed"
        and all(run.get(key) == [] for key in ("script_errors", "drive_errors", "task_errors"))
    )


def report_matches_expectation(report, expected):
    """Require both independent drive modes to satisfy the same expectation."""
    return isinstance(report, dict) and all(
        run_matches_expectation(report.get(mode), expected)
        for mode in ("faithful", "direct")
    )
