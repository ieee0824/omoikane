#!/usr/bin/env python3
"""Verify fixed Issue #1506 sources and a byte-identical reversible observer.

This preflight performs no builds or tests. Its isolated temporary Git index
never changes the checkout, production branches, or Cargo targets.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

BEFORE = '2a5e1d31a37fdcfd1ba93ba2448f8663a2a017b1'
AFTER = 'f04c59a07d56d782afa2f77ea0742431524f5f24'
MANIFEST_SHA = '75d620d419664adba8a14d395e7833ffdaa04f8c6f71890218c581b3bb4b7436'
OBSERVER_SHA = '001ef76167efc0b5ebd30b9d6b522db60b3c9f2f9a36b21237cd892ada3dcc8d'
DIFF_OPTIONS = ['--binary', '--full-index', '--no-ext-diff', '--no-textconv', '--abbrev=40', '--no-renames', '--diff-algorithm=myers']
FILES = {
    'engine/boa/core/gc/src/internals/ephemeron_box.rs',
    'engine/boa/core/gc/src/lib.rs',
    'engine/boa/core/gc/src/test/generational.rs',
    'engine/boa/core/gc/src/test/mod.rs',
}


def git(root, *args, data=None, env=None):
    return subprocess.check_output(['git', '-C', str(root), *args], input=data, env=env)


def preflight(root, observer):
    """Reject a source or observer that changes the fixed comparison."""
    original = git(root, 'diff', *DIFF_OPTIONS, BEFORE, AFTER)
    manifest = git(root, 'diff', '--raw', '--no-abbrev', '--no-renames', '-z',
                   '--no-ext-diff', '--no-textconv', BEFORE, AFTER)
    if hashlib.sha256(manifest).hexdigest() != MANIFEST_SHA:
        raise ValueError('GC production path/mode/full-blob manifest mismatch')
    names = set(git(root, 'diff', '--name-only', BEFORE, AFTER).decode().splitlines())
    if names != FILES:
        raise ValueError('GC production file inventory mismatch')
    reference = json.loads((root / 'scripts/issue1506-pair-spec.json').read_text())['original_test_reference']
    for revision in [BEFORE, AFTER]:
        blob = git(root, 'rev-parse', revision + ':' + reference['file']).decode().strip()
        if blob != reference['blob']:
            raise ValueError('Original test file identity changed')
        text = git(root, 'show', revision + ':' + reference['file']).decode()
        start = text.index(reference['function_segment'].splitlines()[0])
        segment = text[start:start + len(reference['function_segment'])]
        if hashlib.sha256(segment.encode()).hexdigest() != reference['function_segment_sha256']:
            raise ValueError('Original test function contract changed')
    patch = observer.read_bytes()
    if hashlib.sha256(patch).hexdigest() != OBSERVER_SHA:
        raise ValueError('Observer patch identity mismatch')
    observed = {}
    with tempfile.TemporaryDirectory(prefix='issue1506-index-') as temporary:
        env = dict(os.environ, GIT_INDEX_FILE=str(Path(temporary) / 'index'))
        for label, revision in [('before', BEFORE), ('after', AFTER)]:
            git(root, 'read-tree', revision, env=env)
            original_tree = git(root, 'write-tree', env=env).decode().strip()
            git(root, 'apply', '--cached', '--check', '-', data=patch, env=env)
            git(root, 'apply', '--cached', '-', data=patch, env=env)
            tree = git(root, 'write-tree', env=env).decode().strip()
            paths = git(root, 'diff', '--cached', '--name-only', revision, env=env).decode().splitlines()
            if paths != ['src/js/node_lifetime_tests.rs']:
                raise ValueError('Observer unexpectedly changes another source file')
            git(root, 'apply', '--cached', '--reverse', '-', data=patch, env=env)
            restored = git(root, 'write-tree', env=env).decode().strip()
            if restored != original_tree:
                raise ValueError('Observer removal did not restore original tree')
            observed[label] = {'revision': revision, 'original_tree': original_tree,
                               'observed_tree': tree, 'restoration_verified': True,
                               'lock_sha256': {p: hashlib.sha256(git(root, 'show', revision + ':' + p)).hexdigest()
                                               for p in ['Cargo.lock', 'engine/boa/Cargo.lock']}}
    remaining = git(root, 'diff', *DIFF_OPTIONS, observed['before']['observed_tree'],
                    observed['after']['observed_tree'])
    if remaining != original:
        raise ValueError('Observer changes the production before/after diff')
    return {'source_preflight_passed': True, 'execution_performed': False,
            'gc_manifest_sha256': MANIFEST_SHA,
            'gc_patch_sha256': hashlib.sha256(original).hexdigest(),
            'gc_patch_options': DIFF_OPTIONS,
            'observer_sha256': hashlib.sha256(patch).hexdigest(),
            'sources': observed, 'identical_observed_production_diff': True,
            'unverified': ['Rust compilation', 'native Mac execution',
                           'observer overhead', 'GC internal subphase metrics']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument('--observer', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    observer = args.observer or args.repo / 'scripts/issue1506-observer.patch'
    result = preflight(args.repo, observer)
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
