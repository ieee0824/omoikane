"""Conservative classification for CI's documentation-only path."""

import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / "ci-change-scope.py"
spec = importlib.util.spec_from_file_location("ci_change_scope", SCRIPT)
scope = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scope)


class ChangeScopeTests(unittest.TestCase):
    def test_only_explicit_prose_paths_skip_full_ci(self):
        self.assertTrue(scope.is_documentation_only(["AGENTS.md", "docs/ci/notes.md"]))
        for paths in ([], None, ["src/lib.rs"], ["tests/README.md"],
                      ["docs/example.html"], ["AGENTS.md", "scripts/fetch-wpt.sh"],
                      ["docs/guide.md", "tests/wpt/revision.txt"],
                      [".github/workflows/ci.yml"]):
            with self.subTest(paths=paths):
                self.assertFalse(scope.is_documentation_only(paths))

    def test_pull_request_collects_every_page(self):
        event = {"pull_request": {"number": 42}}
        with patch.object(scope, "github_json", side_effect=[
            [{"filename": "docs/guide.md"}] * 100,
            [{"filename": "src/lib.rs"}],
        ]) as fetch:
            self.assertEqual(len(scope.changed_paths("pull_request", event, "token")), 101)
            self.assertEqual(fetch.call_count, 2)

    def test_push_and_other_events(self):
        event = {"ref": "refs/heads/main", "before": "a" * 40, "after": "b" * 40}
        with patch.object(scope, "github_json", return_value={"files": [{"filename": "README.md"}]}) as fetch:
            self.assertEqual(scope.changed_paths("push", event, "token"), ["README.md"])
            self.assertIn("compare/", fetch.call_args.args[0])
        self.assertIsNone(scope.changed_paths("workflow_dispatch", event, "token"))
        event["before"] = "0" * 40
        self.assertIsNone(scope.changed_paths("push", event, "token"))

    def test_rename_from_code_to_documentation_requires_full_ci(self):
        event = {"pull_request": {"number": 42}}
        with patch.object(scope, "github_json", return_value=[
            {"filename": "docs/guide.md", "previous_filename": "src/guide.rs"}
        ]):
            self.assertFalse(scope.is_documentation_only(
                scope.changed_paths("pull_request", event, "token")))


if __name__ == "__main__":
    unittest.main()
