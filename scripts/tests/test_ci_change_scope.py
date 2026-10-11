"""Conservative classification for CI's documentation-only path."""

import importlib.util
import json
import os
import tempfile
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

    def test_docker_only_changes_skip_full_ci_and_build_image(self):
        for paths in (["Dockerfile"], ["Dockerfile", "README.md"],
                      [".dockerignore", "docs/ci/notes.md"],
                      ["docker-entrypoint.sh"]):
            with self.subTest(paths=paths):
                self.assertEqual(scope.check_scope(paths),
                                 {"run_full": False, "run_docker": True})

    def test_mixed_unknown_and_ci_changes_keep_full_checks(self):
        for paths in (None, ["Dockerfile", "src/lib.rs"],
                      ["Dockerfile", "tests/README.md"],
                      [".github/workflows/docker-build.yml"],
                      ["scripts/ci-change-scope.py"],
                      [".github/workflows/change-scope.yml"]):
            with self.subTest(paths=paths):
                self.assertEqual(scope.check_scope(paths),
                                 {"run_full": True, "run_docker": True})
        self.assertEqual(scope.check_scope([]),
                         {"run_full": True, "run_docker": False})
        self.assertEqual(scope.check_scope(["src/lib.rs"]),
                         {"run_full": True, "run_docker": False})
        self.assertEqual(scope.check_scope(["README.md"]),
                         {"run_full": False, "run_docker": False})

    def test_rename_from_code_to_docker_requires_full_ci(self):
        event = {"pull_request": {"number": 42}}
        with patch.object(scope, "github_json", return_value=[
            {"filename": "Dockerfile", "previous_filename": "src/lib.rs"}
        ]):
            self.assertEqual(scope.check_scope(
                scope.changed_paths("pull_request", event, "token")),
                {"run_full": True, "run_docker": True})

    def test_main_emits_both_workflow_outputs(self):
        with tempfile.TemporaryDirectory() as directory:
            event = Path(directory) / "event.json"
            output = Path(directory) / "outputs"
            event.write_text(json.dumps({"pull_request": {"number": 42}}))
            with patch.dict(os.environ, {
                "GITHUB_EVENT_NAME": "pull_request", "GITHUB_EVENT_PATH": str(event),
                "GITHUB_OUTPUT": str(output), "CI_AUTH": "test-token",
            }), patch.object(scope, "github_json", return_value=[
                {"filename": "Dockerfile"}, {"filename": "README.md"}
            ]):
                scope.main()
            self.assertEqual(output.read_text(), "run_full=false\nrun_docker=true\n")

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
