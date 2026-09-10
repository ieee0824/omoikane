"""Source retention and dependency-source failures must not pass the import gate."""

import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "engine_source", Path(__file__).parents[1] / "check-engine-source.py")
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)


class SourceTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.source = self.root / "engine/boa"
        self.source.mkdir(parents=True)
        self.file = self.source / "source.rs"
        self.file.write_bytes(b"original source")
        self.file.chmod(0o644)
        origin = {"path": "engine/boa", "revision": "a" * 40, "tree": "b" * 40,
                  "file_count": 1, "bytes": self.file.stat().st_size, "files": {
                      "source.rs": {"sha256": hashlib.sha256(self.file.read_bytes()).hexdigest(),
                                    "mode": "100644", "bytes": self.file.stat().st_size}}}
        (self.root / "engine/boa-origin.json").write_text(json.dumps(origin))
        self.packages = [{"name": name, "version": "0.21.1", "source": None,
                          "manifest_path": str(self.source / name / "Cargo.toml")}
                         for name in sorted(gate.ENGINE_CRATES)]

    def inspect(self, pristine=False):
        with patch.object(gate, "ROOT", self.root), patch.object(
                gate.subprocess, "check_output", return_value=json.dumps({"packages": self.packages})):
            return gate.inspect(pristine)

    def test_original_source_and_in_tree_graph_pass(self):
        self.assertTrue(self.inspect(pristine=True)["passed"])

    def test_missing_source_never_passes(self):
        self.file.unlink()
        report = self.inspect()
        self.assertFalse(report["passed"])
        self.assertEqual(report["missing_files"], ["source.rs"])

    def test_local_development_is_reported_and_fails_pristine_comparison(self):
        self.file.write_bytes(b"reviewed local change")
        report = self.inspect()
        self.assertTrue(report["passed"])
        self.assertEqual(report["modified_since_import"], ["source.rs"])
        self.assertFalse(self.inspect(pristine=True)["passed"])

    def test_file_mode_is_part_of_the_original_source(self):
        self.file.chmod(0o755)
        self.assertFalse(self.inspect(pristine=True)["passed"])

    def test_git_registry_and_external_paths_are_rejected(self):
        for source, path in [("git+https://example.test/old-fork", self.source / "Cargo.toml"),
                             ("registry+https://example.test/index", self.source / "Cargo.toml"),
                             (None, self.root / "outside/Cargo.toml")]:
            with self.subTest(source=source, path=path):
                self.packages[0].update(source=source, manifest_path=str(path))
                report = self.inspect()
                self.assertFalse(report["passed"])
                self.assertEqual(report["invalid_engine_sources"], [self.packages[0]["name"]])

    def test_missing_engine_crate_is_rejected(self):
        removed = self.packages.pop()["name"]
        report = self.inspect()
        self.assertFalse(report["passed"])
        self.assertEqual(report["missing_engine_crates"], [removed])


if __name__ == "__main__":
    unittest.main()
