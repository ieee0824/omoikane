#!/usr/bin/env python3
"""Source identity checks; no Cargo build or native execution."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = next(p for p in Path(__file__).resolve().parents
            if (p / 'Cargo.toml').is_file() and (p / 'scripts/diagnose-issue1506-pair.py').is_file())
SPEC = importlib.util.spec_from_file_location('pair', ROOT / 'scripts/diagnose-issue1506-pair.py')
PAIR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PAIR)

RUNNER_SPEC = importlib.util.spec_from_file_location('runner', ROOT / 'scripts/run-issue1506-pair.py')
RUNNER = importlib.util.module_from_spec(RUNNER_SPEC)
RUNNER_SPEC.loader.exec_module(RUNNER)


class SourcePreflightTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix='issue1506-preflight-test-')
        cls.repo = Path(cls.temporary.name) / 'fresh'
        subprocess.run(['git', 'init', '-q', str(cls.repo)], check=True)
        subprocess.run(['git', '-C', str(cls.repo), 'fetch', '--quiet', '--depth=1',
                        str(ROOT), PAIR.BEFORE, PAIR.AFTER], check=True)
        (cls.repo / 'scripts').mkdir()
        (cls.repo / 'scripts/issue1506-pair-spec.json').write_bytes(
            (ROOT / 'scripts/issue1506-pair-spec.json').read_bytes())
        cls.observer = ROOT / 'scripts/issue1506-observer.patch'

    @classmethod
    def tearDownClass(cls):
        cls.temporary.cleanup()

    def test_fresh_shallow_abbreviations(self):
        for abbreviation in ['7', '8', '40']:
            with self.subTest(abbreviation=abbreviation):
                subprocess.run(['git', '-C', str(self.repo), 'config',
                                'core.abbrev', abbreviation], check=True)
                result = PAIR.preflight(self.repo, self.observer)
                self.assertTrue(result['identical_observed_production_diff'])
                self.assertTrue(all(s['restoration_verified'] for s in result['sources'].values()))

    def test_changed_observer_rejected(self):
        altered = Path(self.temporary.name) / 'changed.patch'
        altered.write_bytes(self.observer.read_bytes() + b'\n')
        with self.assertRaisesRegex(ValueError, 'Observer patch identity'):
            PAIR.preflight(self.repo, altered)

    def test_wrong_production_manifest_rejected(self):
        original = PAIR.MANIFEST_SHA
        try:
            PAIR.MANIFEST_SHA = '0' * 64
            with self.assertRaisesRegex(ValueError, 'production path/mode/full-blob manifest'):
                PAIR.preflight(self.repo, self.observer)
        finally:
            PAIR.MANIFEST_SHA = original


class EnvironmentMappingTests(unittest.TestCase):
    def test_scalar_unset_and_paths(self):
        import json
        spec = json.loads((ROOT / 'scripts/issue1506-pair-spec.json').read_text())
        env = {'WPT_MANIFEST': 'must be removed', 'CI': 'true'}
        mapped = RUNNER.map_original_environment(
            env, spec['failing_environment_evidence']['observed_values'], Path('/pair/before'))
        self.assertEqual(env['WPT_REQUIRED'], '1')
        self.assertNotIn('WPT_MANIFEST', env)
        self.assertIsNone(mapped['WPT_MANIFEST'])
        self.assertEqual(env['WPT_ROOT'], '/pair/before/.cache/wpt')
        self.assertEqual(env['CI'], 'true')

    def test_unknown_path_prefix_rejected(self):
        with self.assertRaisesRegex(ValueError, 'Unrecognized original path'):
            RUNNER.map_original_environment({}, {'WPT_ROOT': '/unexpected'}, Path('/pair'))


class VariantCollectionTests(unittest.TestCase):
    def exercise(self, build_failure=False):
        import hashlib
        import json
        with tempfile.TemporaryDirectory(prefix='issue1506-variant-test-') as directory:
            allocation = Path(directory)
            worktrees = {}
            for label in ['before', 'after']:
                checkout = allocation / 'sources' / label
                checkout.mkdir(parents=True)
                subprocess.run(['git', 'init', '-q', str(checkout)], check=True)
                (checkout / 'engine/boa').mkdir(parents=True)
                (checkout / 'Cargo.lock').write_text('root lock\n')
                (checkout / 'engine/boa/Cargo.lock').write_text('boa lock\n')
                subprocess.run(['git', '-C', str(checkout), 'add', '.'], check=True)
                subprocess.run(['git', '-C', str(checkout), '-c', 'user.name=test', '-c',
                                'user.email=test@example.invalid', 'commit', '-qm', 'fixture'], check=True)
                worktrees[label] = checkout
            spec = json.loads((ROOT / 'scripts/issue1506-pair-spec.json').read_text())
            state = {'runs': [], 'applied_observers': {k: hashlib.sha256(b'').hexdigest()
                                                     for k in worktrees}}
            state['preflight'] = {'sources': {label: {'lock_sha256': {
                p: hashlib.sha256((checkout / p).read_bytes()).hexdigest()
                for p in ['Cargo.lock', 'engine/boa/Cargo.lock']}}
                for label, checkout in worktrees.items()}}
            calls = []

            def fake_execute(argv, cwd, env, log):
                self.assertEqual(env['WPT_REQUIRED'], '1')
                self.assertEqual(env['CI'], 'true')
                self.assertEqual(env['CARGO_INCREMENTAL'], '0')
                self.assertNotIn('RUST_TEST_THREADS', env)
                is_build = '--no-run' in argv
                calls.append((str(cwd.name), is_build, list(argv)))
                # The first source fails; subsequent source/mode attempts still run.
                failed = len(calls) == (1 if build_failure else 2)
                text = 'simulated failure\n' if failed else ''
                if not is_build and not failed:
                    text += 'running 1 test\n'
                    text += f"test {spec['exact_mode']['selector']} ... ok\n"
                    text += 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 filtered out;\n'
                    text += 'ISSUE1506_PHASE\tgeneration-0\t42\tfalse\tfalse\n'
                log.write_text(text)
                return {'exit_code': 101 if failed else 0, 'command': argv, 'log': str(log)}

            report = allocation / 'result.json'
            RUNNER.execute_variants(allocation, spec, state, worktrees, report, fake_execute)
            self.assertEqual(len(state['runs']), 4)
            self.assertEqual([(r['source'], r['mode']) for r in state['runs']],
                             [('before', 'exact'), ('after', 'exact'), ('before', 'profile'), ('after', 'profile')])
            self.assertEqual(len(calls), 7 if build_failure else 8)
            for run in state['runs']:
                self.assertEqual(set(run['lock_sha256']), {'Cargo.lock', 'engine/boa/Cargo.lock'})
                self.assertTrue(run['source_unchanged'])
            self.assertEqual(len(json.loads(report.read_text())['runs']), 4)
            if build_failure:
                self.assertIsNone(state['runs'][0]['test']['exit_code'])
            else:
                self.assertEqual(state['runs'][0]['test']['exit_code'], 101)
            self.assertEqual(state['runs'][-1]['test']['exit_code'], 0)
            self.assertTrue(state['runs'][-1]['test']['exact_case_passed'])
            self.assertEqual(state['runs'][-1]['test']['phase_rows'][0]['elapsed_ns'], 42)
            self.assertEqual(RUNNER.summarize_results(state), 1)
            self.assertFalse(state['functional_tests_passed'])
            self.assertFalse(state['all_observer_phases_complete'])

    def test_test_failure_preserved_and_other_variants_run(self):
        self.exercise()

    def test_build_failure_skips_only_that_test(self):
        self.exercise(build_failure=True)


class CleanupSafetyTests(unittest.TestCase):
    def test_only_unlocked_cache_inside_allocation_removed(self):
        import fcntl
        import sys
        with tempfile.TemporaryDirectory(prefix='issue1506-clean-test-') as directory:
            root = Path(directory)
            caches = root / 'allocation/targets/before'
            for name in ['completed', 'active', 'source']:
                target = caches / name
                target.mkdir(parents=True)
                (target / 'CACHEDIR.TAG').write_text('fixture')
                (target / '.cargo-space-guard.lock').touch()
            (caches / 'source/Cargo.toml').write_text('preserve source')
            outside = root / 'outside'
            outside.mkdir()
            (outside / 'proof.log').write_text('keep')
            argv = [sys.executable, str(ROOT / 'scripts/cargo-space-guard.py'), 'clean',
                    '--cache-root', str(caches), '--retention-hours', '0', '--maximum-total-gib', '0']
            with (caches / 'active/.cargo-space-guard.lock').open('r+') as lock:
                fcntl.flock(lock, fcntl.LOCK_EX)
                dry = subprocess.run(argv, capture_output=True, text=True, check=True)
                self.assertTrue((caches / 'completed').exists())
                self.assertIn('would remove', dry.stdout)
                executed = subprocess.run(argv + ['--execute'], capture_output=True, text=True, check=True)
                self.assertIn('removed', executed.stdout)
                self.assertFalse((caches / 'completed').exists())
                self.assertTrue((caches / 'active').exists())
                self.assertTrue((caches / 'source/Cargo.toml').exists())
                self.assertEqual((outside / 'proof.log').read_text(), 'keep')


class PhaseInventoryTests(unittest.TestCase):
    def rows(self):
        return [{'phase': p, 'elapsed_ns': 1, 'failed': False, 'tainted_unwind': False}
                for p in RUNNER.expected_phases()]

    def test_success_requires_all_87_ordered_stages(self):
        rows = self.rows()
        self.assertEqual(len(rows), 87)
        self.assertTrue(RUNNER.phases_complete(rows))
        variants = [rows[1:], rows[:-1], rows + [dict(rows[-1])], list(reversed(rows))]
        for field, value in [('failed', True), ('tainted_unwind', True), ('elapsed_ns', None)]:
            altered = [dict(r) for r in rows]
            altered[0][field] = value
            variants.append(altered)
        for altered in variants:
            with self.subTest(altered=altered[0]):
                self.assertFalse(RUNNER.phases_complete(altered))

    def test_profile_delta_requires_non_decreasing_snapshots(self):
        import json
        def line(before, after):
            return 'ISSUE1506_GC_PHASE\tsetup\t' + json.dumps(before) + '\t' + json.dumps(after)
        self.assertEqual(RUNNER.parse_gc_snapshots(line([1]*10, [3]*10))[0]['delta'], [2]*10)
        self.assertIsNone(RUNNER.parse_gc_snapshots(line([3]*10, [1]*10))[0]['delta'])

    def test_green_requires_complete_observer_and_profile(self):
        state = {'runs': [{'test': {'exit_code': 0, 'exact_case_passed': True,
                                   'all_observer_phases_complete': True, 'gc_profile_complete': True},
                           'source_unchanged': True} for _ in range(4)]}
        self.assertEqual(RUNNER.summarize_results(state), 0)
        state['runs'][0]['test']['all_observer_phases_complete'] = False
        self.assertEqual(RUNNER.summarize_results(state), 1)
        self.assertTrue(state['functional_tests_passed'])


class ExactCaseLogTests(unittest.TestCase):
    def test_interleaved_observer_output(self):
        text = ('running 1 test\ntest sample ... ISSUE1506_PHASE\tsetup\t1\tfalse\tfalse\n'
                'ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 7 filtered out;\n')
        self.assertTrue(RUNNER.exact_case_passed(text, 'sample'))
        self.assertFalse(RUNNER.exact_case_passed(text, 'different'))

    def test_incomplete_duplicate_failed_ignored_or_missing_rejected(self):
        text = ('running 1 test\ntest sample ... ok\n'
                'test result: ok. 1 passed; 0 failed; 0 ignored;\n')
        self.assertTrue(RUNNER.exact_case_passed(text, 'sample'))
        variants = [text.split('test result:')[0], text + 'test sample ... ok\n',
                    text.replace('1 passed; 0 failed', '0 passed; 1 failed'),
                    text.replace('0 ignored', '1 ignored'), text.replace('sample', 'another'),
                    text.replace('running 1 test\n', ''), text.replace(' ... ok', ' ... FAILED')]
        for altered in variants:
            self.assertFalse(RUNNER.exact_case_passed(altered, 'sample'))


if __name__ == '__main__':
    unittest.main()
