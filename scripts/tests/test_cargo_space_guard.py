"""Tests for bounded Cargo target execution and cache cleanup."""

import importlib.util
import argparse
import os
from pathlib import Path
import shutil
import sys
import tempfile
import time
import unittest


SPEC = importlib.util.spec_from_file_location(
    "cargo_space_guard", Path(__file__).parents[1] / "cargo-space-guard.py"
)
guard = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = guard
SPEC.loader.exec_module(guard)


class CargoSpaceGuardTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.workspace = self.root / "source"
        self.workspace.mkdir()

    def cache(self, name, size=16, age_hours=24 * 8):
        path = self.root / "cache" / name
        path.mkdir(parents=True)
        marker = path / ".rustc_info.json"
        marker.write_text("{}")
        (path / guard.LOCK_NAME).touch()
        (path / "artifact").write_bytes(b"x" * size)
        timestamp = time.time() - age_hours * 3600
        os.utime(marker, (timestamp, timestamp))
        return path

    def test_run_target_must_be_dedicated_and_outside_source(self):
        with self.assertRaises(ValueError):
            guard.validate_run_target(self.workspace / "target", self.workspace)
        with self.assertRaises(ValueError):
            guard.validate_run_target(Path("/target"), self.workspace)
        with self.assertRaises(ValueError):
            guard.validate_run_target(self.root, self.workspace)
        target = self.root / "cache" / "issue-737"
        self.assertEqual(guard.validate_run_target(target, self.workspace), target)

    def test_size_argument_rejects_non_finite_values(self):
        for value in ("nan", "inf", "-1"):
            with self.subTest(value=value), self.assertRaises(argparse.ArgumentTypeError):
                guard.gib(value)

    def test_cleanup_selects_only_marked_expired_targets(self):
        cache_root = self.root / "cache"
        expired = self.cache("expired")
        recent = self.cache("recent", age_hours=1)
        source = self.cache("source")
        (source / "Cargo.toml").write_text("[package]\n")
        evidence = self.cache("evidence")
        (evidence / ".artifacts").mkdir()
        unmarked = cache_root / "unmarked"
        unmarked.mkdir()
        legacy = self.cache("legacy")
        (legacy / guard.LOCK_NAME).unlink()
        selected = guard.clean_targets(
            cache_root, {recent}, 24 * 7 * 3600, guard.GIB, False, now=time.time()
        )
        self.assertEqual([cache.path for cache in selected], [expired])
        self.assertTrue(source.exists())
        self.assertTrue(evidence.exists())
        self.assertTrue(unmarked.exists())
        self.assertTrue(legacy.exists())

    def test_directory_size_counts_hard_links_once(self):
        target = self.cache("hard-links", size=4096)
        before = guard.directory_size(target)
        os.link(target / "artifact", target / "artifact-link")
        self.assertEqual(guard.directory_size(target), before)

    def test_execute_keeps_locked_target_and_removes_inactive_target(self):
        cache_root = self.root / "cache"
        locked = self.cache("locked")
        inactive = self.cache("inactive", age_hours=1)
        lock = guard.lock_target(locked, blocking=False)
        self.addCleanup(lock.close)
        removed = guard.clean_targets(
            cache_root, set(), 24 * 7 * 3600, 20, True, now=time.time()
        )
        self.assertEqual([cache.path for cache in removed], [inactive])
        self.assertTrue(locked.exists())
        self.assertFalse(inactive.exists())

    def test_insufficient_preflight_space_does_not_start_command(self):
        target = self.root / "cache" / "preflight"
        usage = shutil.disk_usage(self.root)
        low_space = usage.__class__(usage.total, usage.used, 0)
        result = guard.run_guarded(
            [self.root / "command-that-must-not-run"],
            target,
            self.workspace,
            required_free=1,
            minimum_free=0,
            maximum_target=guard.GIB,
            poll_seconds=0.01,
            usage=lambda _: low_space,
        )
        self.assertEqual(result, guard.CAPACITY_EXIT)

    def test_guard_stops_process_when_target_exceeds_limit(self):
        target = self.root / "cache" / "bounded"
        command = [
            sys.executable,
            "-c",
            "from pathlib import Path; import os,time; "
            "Path(os.environ['CARGO_TARGET_DIR'],'large').write_bytes(b'x'*16384); time.sleep(30)",
        ]
        usage = shutil.disk_usage(self.root)
        result = guard.run_guarded(
            command,
            target,
            self.workspace,
            required_free=1,
            minimum_free=0,
            maximum_target=8192,
            poll_seconds=0.01,
            usage=lambda _: usage,
        )
        self.assertEqual(result, guard.CAPACITY_EXIT)


if __name__ == "__main__":
    unittest.main()
