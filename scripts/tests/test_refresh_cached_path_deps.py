"""Contracts for marking restored Gate 5 path dependency outputs fresh (#1537)."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
from refresh_cached_path_deps import is_path_dependency_output, refresh

OLD_NS = 1_000_000_000 * 1_000_000_000
NEW_NS = 2_000_000_000 * 1_000_000_000

CACHED = [
    '.fingerprint/boa_ast-1a/dep-lib-boa_ast',
    '.fingerprint/boa_ast-1a/lib-boa_ast',
    '.fingerprint/regress-2b/dep-lib-regress',
    '.fingerprint/small_btree-3c/lib-small_btree.json',
    '.fingerprint/tag_ptr-4d/dep-lib-tag_ptr',
    'deps/boa_engine.d',
    'deps/libboa_engine.rlib',
    'deps/libboa_macros-5e.so',
    'deps/libregress-2b.rmeta',
    'deps/small_btree-3c.d',
    'deps/libtag_ptr-4d.rlib',
]
UNRELATED = [
    '.fingerprint/omoikane-6f/dep-lib-omoikane',
    '.fingerprint/smallvec-7a/lib-smallvec',
    'deps/libsmallvec-7a.rlib',
    'deps/libtagged-8b.rlib',
    'deps/omoikane-6f',
    'build/ring-9c/output',
]


def build_tree(root):
    for relative in CACHED + UNRELATED:
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(relative)
    for path in sorted(root.rglob('*'), reverse=True):
        os.utime(path, ns=(OLD_NS, OLD_NS))


class RefreshCachedPathDepsTests(unittest.TestCase):
    def test_only_cached_path_dependency_outputs_get_the_new_timestamp(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            build_tree(root)
            refresh(root, NEW_NS)
            for relative in CACHED:
                self.assertEqual((root / relative).stat().st_mtime_ns, NEW_NS, relative)
                if relative.startswith('.fingerprint/'):
                    # A crate's fingerprint directory is itself a restored output.
                    self.assertEqual((root / relative).parent.stat().st_mtime_ns, NEW_NS)
            for relative in UNRELATED:
                self.assertEqual((root / relative).stat().st_mtime_ns, OLD_NS, relative)
            # Shared output directories are not restored outputs.
            self.assertEqual((root / 'deps').stat().st_mtime_ns, OLD_NS)

    def test_names_match_crate_prefixes_not_similar_crates(self):
        self.assertTrue(is_path_dependency_output('libboa_parser-1a.rlib'))
        self.assertTrue(is_path_dependency_output('regress-2b'))
        self.assertFalse(is_path_dependency_output('smallvec-7a'))
        self.assertFalse(is_path_dependency_output('libregression-1.rlib'))
        self.assertFalse(is_path_dependency_output('omoikane-6f'))

    def test_missing_outputs_fail_instead_of_reporting_success(self):
        with tempfile.TemporaryDirectory() as temp:
            result = subprocess.run(
                [sys.executable, str(SCRIPTS / 'refresh_cached_path_deps.py'), temp],
                capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('no restored path dependency outputs', result.stderr)


if __name__ == '__main__':
    unittest.main()
