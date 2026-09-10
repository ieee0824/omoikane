"""Prevent absent, stale or fallback-only evidence from approving a release."""
import contextlib
import copy
import importlib.util
import io
from pathlib import Path
import tempfile
import tarfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("gate5", Path(__file__).parents[1] / "jit-gate5.py")
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)


def probe(target):
    rows = [{"seed": seed, "shape": shape, "sample": sample, "jit_enabled": enabled,
             "success": True, "expected": 42, "actual": 42, "elapsed_ns": 1000,
             "compiled_entries": int(enabled), "generated_code_bytes": 128 * int(enabled),
             "compile_time_ns": 200 * int(enabled), "property_guard_hits": int(enabled),
             "bailouts": 0}
            for seed in (305, 541, 542, 543) for shape in ("arith", "prop-mono")
            for sample in range(5) for enabled in (False, True)]
    return {"status": "passed", "architecture": target.split("-")[0], "cases": rows}


class GateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.identity = {"revision": "a" * 40, "lock_sha256": "b" * 64, "source_dirty": False}
        self.save("inputs/identity.json", self.identity)
        for target, (library, archive) in gate.TARGETS.items():
            identity = dict(self.identity, target=target)
            self.save(f"{target}/suite/result.json", {
                "identity": identity, "status": "passed", "test_results": [["100", "0", "0"]],
                "steps": [{"name": "fetch-wpt", "exit": 0},
                          {"name": "full-suite", "exit": 0, "command": gate.SUITE_COMMAND}]})
            manifest = {"identity": identity, "default_library_features": [],
                        "diagnostic_features": ["baseline-jit"], "files": {
                            name: {"sha256": "c" * 64, "bytes": 10}
                            for name in (library, "omoikane.h", "omoikane-jit-smoke")}}
            self.save(f"{target}/package/result.json", {
                "identity": identity, "status": "passed", "archive": {
                    "name": archive, "bytes": 30, "manifest": manifest},
                "steps": [{"name": name, "exit": 0} for name in
                          ("default-library", "jit-probe-build", "ffi-smoke", "jit-smoke")]})
            self.save(f"{target}/suite/acid3.json", {mode: {"score": 100, "total": 100}
                                                   for mode in ("faithful", "direct")})
            self.save(f"{target}/suite/wpt.json", {"summary": {"total": 69, "regression": 0},
                       "revision": Path("tests/wpt/revision.txt").read_text().strip()})
            self.save(f"{target}/suite/web-api.json", {"regressions": [], "total": 78})
            self.save(f"{target}/suite/native.json", {"status": "passed", "jit_policy": "execution-confirmed",
                       "architecture": target.split("-")[0], "cases": [{"success": True}] * 16})
            self.save(f"{target}/package/ffi.json", {"status": "passed", "cases": [{}, {}],
                       "library_sha256": "c" * 64})
            self.save(f"{target}/package/probe/probe.json", probe(target))

    def save(self, relative, value):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        gate.write(path, value)

    def decision(self, result="success"):
        with patch.object(gate, "output", return_value=self.identity["revision"]), contextlib.redirect_stdout(io.StringIO()):
            return gate.aggregate(self.root, result)

    def test_complete_evidence_passes(self):
        self.assertTrue(self.decision())

    def test_archive_preserves_executable_and_library_bytes(self):
        source = self.root / "packaged-probe"
        source.write_bytes(b"packaged executable bytes")
        source.chmod(0o755)
        library = self.root / "library"
        library.write_bytes(b"packaged library bytes")
        library.chmod(0o644)
        archive = self.root / "distribution.tar.gz"
        with tarfile.open(archive, "w:gz") as bundle:
            for path in (source, library):
                bundle.add(path, arcname=path.name)
        destination = self.root / "extracted"
        gate.extract_archive(archive, destination, {source.name, library.name})
        for path in (source, library):
            self.assertEqual((destination / path.name).read_bytes(), path.read_bytes())
            self.assertEqual((destination / path.name).stat().st_mode & 0o777,
                             path.stat().st_mode & 0o777)

    def test_unexpected_packaging_error_is_recorded_as_failed(self):
        target = next(iter(gate.TARGETS))
        folder = self.root / "failure-evidence"
        self_identity = dict(self.identity, target=target)
        (folder / "inputs").mkdir(parents=True)
        gate.write(folder / "inputs/identity.json", self_identity)
        with patch.object(gate, "identity", return_value=self_identity), \
                patch.object(gate.subprocess, "run", side_effect=TypeError("unsupported API")), \
                contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            self.assertFalse(gate.run_target("package", target, folder))
        result = gate.read(folder / target / "package/result.json")
        self.assertEqual(result["status"], "failed")
        self.assertEqual(result["error"], "TypeError: unsupported API")

    def test_every_target_and_report_is_required(self):
        for target in gate.TARGETS:
            for name in ("suite/result.json", "package/result.json", "suite/acid3.json", "suite/wpt.json",
                         "suite/web-api.json", "suite/native.json", "package/ffi.json", "package/probe/probe.json"):
                with self.subTest(target=target, report=name):
                    path = self.root / target / name
                    original = path.read_bytes()
                    path.unlink()
                    self.assertFalse(self.decision())
                    path.write_bytes(original)

    def test_failed_cancelled_and_skipped_jobs_never_pass(self):
        for result in ("failure", "cancelled", "skipped"):
            self.assertFalse(self.decision(result))

    def test_mismatched_revision_lock_or_host_never_passes(self):
        path = self.root / next(iter(gate.TARGETS)) / "package/result.json"
        original = gate.read(path)
        for key in ("revision", "lock_sha256", "target", "source_dirty"):
            value = copy.deepcopy(original)
            value["identity"][key] = True if key == "source_dirty" else "different"
            gate.write(path, value)
            self.assertFalse(self.decision())
        gate.write(path, original)

    def test_native_fallback_duplicate_or_wrong_result_never_passes(self):
        target = next(iter(gate.TARGETS))
        baseline = probe(target)
        for key, value in (("compiled_entries", 0), ("property_guard_hits", 0), ("actual", 43),
                           ("generated_code_bytes", 0), ("compile_time_ns", 0)):
            altered = copy.deepcopy(baseline)
            row = next(row for row in altered["cases"] if row["jit_enabled"] and row["shape"] == "prop-mono")
            row[key] = value
            self.assertFalse(gate.valid_probe(altered, target), key)
        duplicate = copy.deepcopy(baseline)
        duplicate["cases"][-1] = duplicate["cases"][0]
        self.assertFalse(gate.valid_probe(duplicate, target))

    def test_regressions_or_different_library_never_pass(self):
        target = next(iter(gate.TARGETS))
        for name, altered in (("suite/acid3.json", {"faithful": {"score": 99, "total": 100}}),
                              ("suite/wpt.json", {"summary": {"total": 69, "regression": 1}}),
                              ("suite/web-api.json", {"total": 78, "regressions": ["DOM"]}),
                              ("package/ffi.json", {"status": "passed", "cases": [{}, {}], "library_sha256": "wrong"})):
            path = self.root / target / name
            original = path.read_bytes()
            gate.write(path, altered)
            self.assertFalse(self.decision())
            path.write_bytes(original)


if __name__ == "__main__":
    unittest.main()
