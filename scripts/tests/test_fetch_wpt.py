"""Offline checks for the pinned WPT sparse checkout setup."""

import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).resolve().parents[1] / "fetch-wpt.sh"


class FetchWptTests(unittest.TestCase):
    def test_sparse_patterns_are_applied_once_and_rechecked_on_change(self):
        git = shutil.which("git")
        assert git is not None
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            repo = root / "wpt"
            repo.mkdir()
            subprocess.run([git, "init", "-q", str(repo)], check=True)
            subprocess.run([git, "-C", str(repo), "config", "user.email", "test@example.com"], check=True)
            subprocess.run([git, "-C", str(repo), "config", "user.name", "Test"], check=True)
            resource = repo / "resources" / "test.txt"
            resource.parent.mkdir()
            resource.write_text("fixture\n")
            subprocess.run([git, "-C", str(repo), "add", "."], check=True)
            subprocess.run([git, "-C", str(repo), "commit", "-qm", "fixture"], check=True)
            revision = subprocess.check_output([git, "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()

            (root / "tests" / "wpt").mkdir(parents=True)
            (root / "tests" / "wpt" / "revision.txt").write_text(revision + "\n")
            script = root / "fetch-wpt.sh"
            script.write_text(SCRIPT.read_text())

            wrapper = root / "bin"
            wrapper.mkdir()
            trace = root / "git.log"
            git_wrapper = wrapper / "git"
            git_wrapper.write_text(
                '#!/bin/sh\nprintf "%s\\n" "$*" >> "$GIT_TRACE_FILE"\nexec "$REAL_GIT" "$@"\n'
            )
            git_wrapper.chmod(0o755)
            env = dict(os.environ, WPT_ROOT=str(repo), GIT_TRACE_FILE=str(trace), REAL_GIT=git)
            env["PATH"] = str(wrapper) + os.pathsep + env["PATH"]

            def run_script():
                trace.write_text("")
                subprocess.run(["bash", str(script)], cwd=root, env=env, check=True)
                return trace.read_text()

            first = run_script()
            self.assertEqual(first.count("sparse-checkout set "), 1)
            self.assertTrue(resource.is_file())
            self.assertEqual(run_script().count("sparse-checkout set "), 0)

            script.write_text(script.read_text().replace(
                'current_revision="$(git_wpt rev-parse HEAD 2>/dev/null || true)"',
                'collect_pattern "/new-path/"\ncurrent_revision="$(git_wpt rev-parse HEAD 2>/dev/null || true)"',
            ))
            self.assertEqual(run_script().count("sparse-checkout set "), 1)
            patterns = subprocess.check_output([git, "-C", str(repo), "sparse-checkout", "list"], text=True)
            self.assertIn("/new-path/", patterns.splitlines())


if __name__ == "__main__":
    unittest.main()
