"""Tests for bounded Cargo target execution and cache cleanup."""

import importlib.util
import argparse
import json
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

    def test_target_binding_rejects_another_worktree(self):
        target = self.root / "cache" / "bound"
        guard.prepare_target(target, self.workspace)

        binding = json.loads((target / guard.BINDING_NAME).read_text())
        self.assertEqual(binding["workspace"], str(self.workspace.resolve()))
        self.assertEqual(binding["uid"], os.geteuid())

        other_workspace = self.root / "other-source"
        other_workspace.mkdir()
        with self.assertRaisesRegex(ValueError, "another worktree"):
            guard.prepare_target(target, other_workspace)

    def test_binding_uses_the_worktree_root_from_a_subdirectory(self):
        (self.workspace / ".git").write_text("gitdir: elsewhere\n")
        child = self.workspace / "nested" / "directory"
        child.mkdir(parents=True)
        target = self.root / "cache" / "from-subdirectory"

        guard.prepare_target(target, child)

        binding = guard.read_binding(target)
        self.assertEqual(binding.workspace, str(self.workspace.resolve()))

    def test_target_binding_rejects_another_user(self):
        target = self.root / "cache" / "foreign-user"
        guard.prepare_target(target, self.workspace)
        marker = target / guard.BINDING_NAME
        binding = json.loads(marker.read_text())
        binding["uid"] = os.geteuid() + 1
        marker.write_text(json.dumps(binding))

        with self.assertRaisesRegex(ValueError, "another uid"):
            guard.prepare_target(target, self.workspace)

    def test_unbound_target_with_build_artifacts_requires_reset(self):
        target = self.root / "cache" / "legacy"
        target.mkdir(parents=True)
        (target / "CACHEDIR.TAG").write_text(guard.CACHE_TAG)
        (target / "debug").mkdir()

        with self.assertRaisesRegex(ValueError, "not bound to a worktree"):
            guard.prepare_target(target, self.workspace)

        quarantine = guard.reset_target(target, self.workspace, execute=True)
        self.assertTrue((quarantine / "debug").is_dir())
        self.assertIsNotNone(guard.read_binding(target))

    def test_target_with_unwritable_artifact_is_rejected_before_command(self):
        target = self.root / "cache" / "unwritable"
        guard.prepare_target(target, self.workspace)
        artifact = target / "debug" / "deps" / "crate.rcgu.o"
        artifact.parent.mkdir(parents=True)
        artifact.write_bytes(b"object")
        artifact.chmod(0o400)
        self.addCleanup(artifact.chmod, 0o600)

        with self.assertRaisesRegex(ValueError, "not writable"):
            guard.run_guarded(
                [sys.executable, "-c", "raise SystemExit('must not run')"],
                target,
                self.workspace,
                required_free=1,
                minimum_free=0,
                maximum_target=guard.GIB,
                poll_seconds=0.01,
            )

    def test_reset_quarantines_whole_target_and_reinitializes_binding(self):
        target = self.root / "cache" / "corrupt"
        guard.prepare_target(target, self.workspace)
        artifact = target / "debug" / "incremental" / "partial.o"
        artifact.parent.mkdir(parents=True)
        artifact.write_bytes(b"partial")

        planned = guard.reset_target(
            target, self.workspace, execute=False, now=1_790_000_000
        )
        self.assertTrue(target.exists())
        self.assertFalse(planned.exists())

        quarantine = guard.reset_target(
            target, self.workspace, execute=True, now=1_790_000_000
        )
        self.assertEqual(quarantine, planned)
        self.assertEqual(
            (quarantine / artifact.relative_to(target)).read_bytes(), b"partial"
        )
        self.assertFalse((target / artifact.relative_to(target)).exists())
        binding = json.loads((target / guard.BINDING_NAME).read_text())
        self.assertEqual(binding["workspace"], str(self.workspace.resolve()))
        self.assertEqual(binding["uid"], os.geteuid())

    def test_reset_refuses_an_active_or_foreign_target(self):
        target = self.root / "cache" / "active"
        guard.prepare_target(target, self.workspace)
        target_lock = guard.lock_target(target, blocking=False)
        try:
            with self.assertRaisesRegex(ValueError, "already in use"):
                guard.reset_target(target, self.workspace, execute=True)
        finally:
            target_lock.close()
        other_workspace = self.root / "other-source"
        other_workspace.mkdir()
        with self.assertRaisesRegex(ValueError, "another worktree"):
            guard.reset_target(target, other_workspace, execute=True)

    def test_reset_refuses_preserved_source_or_evidence_targets(self):
        target = self.root / "cache" / "protected"
        guard.prepare_target(target, self.workspace)
        preserve = target / guard.PRESERVE_NAME
        preserve.touch()
        with self.assertRaisesRegex(ValueError, "marked for preservation"):
            guard.reset_target(target, self.workspace, execute=True)

        preserve.unlink()
        (target / "Cargo.toml").write_text("[package]\n")
        with self.assertRaisesRegex(ValueError, "source or evidence"):
            guard.reset_target(target, self.workspace, execute=True)


if __name__ == "__main__":
    unittest.main()
