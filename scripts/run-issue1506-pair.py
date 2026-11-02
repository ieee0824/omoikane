#!/usr/bin/env python3
"""Run the fixed Issue1506 pair sequentially on one native Apple Silicon host.

Never retries failures. Worktrees, raw logs and partial results are retained
for artifact collection. Completed target caches are cleaned with the guard.
"""
import argparse
import datetime
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import time


def utc():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def save(path, value):
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)


def command(argv, cwd, env, log):
    start = time.monotonic_ns()
    wall_start = time.time_ns()
    result = {'command': argv, 'cwd': str(cwd), 'started_utc': utc(),
              'started_monotonic_ns': start}
    with log.open('wb') as output:
        process = subprocess.run(argv, cwd=cwd, env=env, stdout=output,
                                 stderr=subprocess.STDOUT, check=False)
    elapsed = time.monotonic_ns() - start
    wall_elapsed = time.time_ns() - wall_start
    result.update(exit_code=process.returncode, ended_utc=utc(),
                  elapsed_ns=elapsed, wall_elapsed_ns=wall_elapsed,
                  wall_minus_monotonic_ns=wall_elapsed - elapsed, log=str(log))
    return result


def map_original_environment(env, observed, checkout):
    """Map only known filesystem roles; preserve scalar values and unset."""
    mapped = {}
    paths = {'WPT_ROOT', 'WPT_REPORT', 'WPT_JUNIT',
             'OMOIKANE_JIT_GATE_REPORT_DIR', 'OMOIKANE_WEB_API_REPORT'}
    for name, value in observed.items():
        if name in ['CI', 'RUST_TEST_THREADS']:
            continue
        if value is None:
            env.pop(name, None)
            mapped[name] = None
        elif name in paths:
            prefix = '/omoikane/omoikane/'
            if prefix not in value:
                raise ValueError('Unrecognized original path role: ' + name)
            mapped[name] = str(checkout / value.split(prefix, 1)[1])
            env[name] = mapped[name]
        else:
            mapped[name] = value
            env[name] = value
    return mapped


def exact_case_passed(text, selector):
    """Accept interleaved observer stderr only with one exact all-pass case."""
    cases = re.findall(r'^test (.+?) \.\.\. (.*)$', text, re.M)
    summaries = re.findall(
        r'^test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored;', text, re.M)
    return (len(cases) == 1 and cases[0][0] == selector
            and not re.match(r'^(FAILED|ignored)(?:\b|$)', cases[0][1])
            and len(re.findall(r'^running 1 test$', text, re.M)) == 1
            and summaries == [('ok', '1', '0', '0')])


def expected_phases():
    """The original test's outer processing stages, in execution order."""
    return (['runtime-bootstrap', 'setup'] + [f'generation-{i}' for i in range(80)]
            + ['retained-collection', 'retained-content', 'release-references',
               'release-collection', 'release-assertions'])


def phases_complete(rows):
    """Reject missing, duplicate, extra, failed or unwind-tainted samples."""
    return ([r['phase'] for r in rows] == expected_phases()
            and all(r['elapsed_ns'] is not None and r['elapsed_ns'] >= 0
                    and not r['failed'] and not r['tainted_unwind'] for r in rows))


def parse_gc_snapshots(text):
    """Decode the existing thread-local GC API's integer snapshots, no resets."""
    rows = []
    for label, before, after in re.findall(
            r'ISSUE1506_GC_PHASE\t([^\t\n]+)\t(\[[^\t\n]+\])\t(\[[^\t\n]+\])', text):
        before, after = json.loads(before), json.loads(after)
        valid = (len(before) == len(after) == 10
                 and all(type(a) is int and type(b) is int and 0 <= a <= b
                         for a, b in zip(before, after)))
        rows.append({'phase': label, 'before': before, 'after': after,
                     'delta': [b-a for a, b in zip(before, after)] if valid else None,
                     'invalid_reason': None if valid else 'non-monotonic or malformed snapshots'})
    return rows


def execute_variants(allocation, spec, state, worktrees, report, execute=command):
    """Collect all four source/mode results without replacing failed attempts."""
    for mode, key in [('exact', 'exact_mode'), ('profile', 'profile_mode')]:
        for label in ['before', 'after']:
            checkout = worktrees[label]
            target = allocation / 'targets' / label / mode
            if target.exists():
                raise ValueError('Target must start clean')
            out = allocation / 'logs' / label / mode
            out.mkdir(parents=True)
            env = os.environ.copy()
            env.update(CI='true', CARGO_BUILD_JOBS='2', CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=str(target))
            env.pop('RUST_TEST_THREADS', None)
            mapped = map_original_environment(
                env, spec['failing_environment_evidence']['observed_values'], checkout)
            run = {'source': label, 'mode': mode, 'target_policy': 'fresh separate source/mode target',
                   'mapped_original_environment': mapped,
                   'effective_environment': {k: v for k, v in env.items()
                       if k.startswith(('CARGO_PROFILE_', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS'))
                       or k in ['CI', 'CARGO_BUILD_JOBS', 'CARGO_TARGET_DIR', 'RUST_TEST_THREADS']},
                   'RUST_TEST_THREADS': None,
                   'explicit_unset_or_values': {k: env.get(k) for k in [
                       'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER',
                       'RUSTUP_TOOLCHAIN', 'CARGO_INCREMENTAL', 'CARGO_BUILD_TARGET', 'CARGO_PROFILE_DEV_DEBUG',
                       'CARGO_PROFILE_TEST_DEBUG', 'CARGO_PROFILE_TEST_OPT_LEVEL']},
                   'revision': spec['sources'][label], 'host_load_average': list(os.getloadavg())}
            state['runs'].append(run)
            save(report, state)
            run['initial_lock_sha256'] = {p: hashlib.sha256((checkout / p).read_bytes()).hexdigest()
                                           for p in ['Cargo.lock', 'engine/boa/Cargo.lock']}
            if run['initial_lock_sha256'] != state['preflight']['sources'][label]['lock_sha256']:
                raise ValueError('Checkout lock identity mismatch before execution')
            argv = spec[key]['command']
            separator = argv.index('--')
            build = argv[:separator] + ['--no-run']
            guard = ['python3', str(checkout / 'scripts/cargo-space-guard.py'), 'run',
                     '--target-dir', str(target), '--required-free-gib', '8',
                     '--minimum-free-gib', '4', '--maximum-target-gib', '36', '--']
            run['build'] = execute(guard + build, checkout, env, out / 'build.log')
            save(report, state)
            if run['build']['exit_code'] == 0:
                run['test'] = execute(guard + argv, checkout, env, out / 'test.log')
            else:
                run['test'] = {'exit_code': None, 'not_executed_reason': 'build failed; no retry'}
            if run['test']['exit_code'] is not None:
                text = (out / 'test.log').read_text(errors='replace')
                selector = spec[key]['selector'] if 'selector' in spec[key] else spec['exact_mode']['selector']
                run['test']['exact_case_passed'] = exact_case_passed(text, selector)
                run['test']['phase_rows'] = [
                    {'phase': m[0], 'elapsed_ns': None if m[1] == 'null' else int(m[1]),
                     'failed': m[2] == 'true', 'tainted_unwind': m[3] == 'true'}
                    for m in re.findall(r'ISSUE1506_PHASE\t([^\t\n]+)\t(\d+|null)\t(true|false)\t(true|false)', text)]
                rows = run['test']['phase_rows']
                run['test']['all_observer_phases_complete'] = phases_complete(rows)
                gc_rows = parse_gc_snapshots(text)
                run['test']['gc_snapshots'] = gc_rows
                run['test']['gc_profile_complete'] = (not gc_rows if mode == 'exact' else
                    [r['phase'] for r in gc_rows] == expected_phases()
                    and all(r['delta'] is not None for r in gc_rows)
                    and phases_complete(rows))
            # Preserve the exact source delta even after a failed test.
            diff_after = subprocess.check_output(['git', '-C', str(checkout), 'diff', '--binary'])
            run['source_unchanged'] = hashlib.sha256(diff_after).hexdigest() == state['applied_observers'][label]
            run['lock_sha256'] = {p: hashlib.sha256((checkout / p).read_bytes()).hexdigest()
                                  for p in ['Cargo.lock', 'engine/boa/Cargo.lock']}
            run['source_unchanged'] = run['source_unchanged'] and run['lock_sha256'] == run['initial_lock_sha256']
            save(report, state)
            # Only completed allocation-owned targets are eligible. Raw
            # evidence and source checkouts remain available for upload.
            cleanup = ['python3', str(checkout / 'scripts/cargo-space-guard.py'),
                       'clean', '--cache-root', str(target.parent),
                       '--retention-hours', '0', '--maximum-total-gib', '0']
            if target.parent.exists():
                run['target_cleanup_dry_run'] = execute(
                    cleanup, checkout, env, out / 'target-cleanup-dry-run.log')
                if run['target_cleanup_dry_run']['exit_code'] == 0:
                    run['target_cleanup'] = execute(
                        cleanup + ['--execute'], checkout, env, out / 'target-cleanup.log')
                save(report, state)


def summarize_results(state):
    """Keep functional failure distinct from completing diagnostic collection."""
    state['functional_tests_passed'] = len(state['runs']) == 4 and all(
        r['test']['exit_code'] == 0 and r['test'].get('exact_case_passed', False)
        and r['source_unchanged'] for r in state['runs'])
    state['collection_completed'] = len(state['runs']) == 4 and all(
        r['test']['exit_code'] is not None and r['source_unchanged'] for r in state['runs'])
    state['all_observer_phases_complete'] = len(state['runs']) == 4 and all(
        r['test'].get('all_observer_phases_complete', False) for r in state['runs'])
    state['gc_profile_measurements_complete'] = len(state['runs']) == 4 and all(
        r['test'].get('gc_profile_complete', False) for r in state['runs'])
    return 0 if (state['functional_tests_passed'] and state['all_observer_phases_complete']
                 and state['gc_profile_measurements_complete']) else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--allocation', required=True, type=Path)
    parser.add_argument('--execute', action='store_true')
    args = parser.parse_args()
    if not args.execute:
        parser.error('Execution requires --execute; source preflight has a separate entry')
    if platform.system() != 'Darwin' or platform.machine() != 'arm64':
        parser.error('A native arm64 Darwin allocation is required')
    root = Path(__file__).resolve().parent.parent
    allocation = args.allocation.resolve()
    allocation.mkdir(parents=True, exist_ok=False)
    spec = json.loads((root / 'scripts/issue1506-pair-spec.json').read_text())
    module_spec = importlib.util.spec_from_file_location(
        'pair_preflight', root / 'scripts/diagnose-issue1506-pair.py')
    module = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(module)
    observer = root / 'scripts/issue1506-observer.patch'
    state = {'started_utc': utc(), 'terminal': False, 'runs': [],
             'same_allocation': True, 'run_order': 'before-exact,after-exact,before-profile,after-profile',
             'observer_overhead': None, 'observer_overhead_reason': 'not measured',
             'retained_graph_cardinality': None,
             'retained_graph_cardinality_reason': 'not independently sampled; original80-generation reachability and final-reclamation assertions preserved',
             'gc_profile_fields': ['minor_count', 'minor_total_ns', 'minor_mark_ns',
                 'minor_finalize_ns', 'minor_sweep_ns', 'major_count', 'major_total_ns',
                 'major_mark_ns', 'major_finalize_ns', 'major_sweep_ns'],
             'gc_visits_and_thread_cpu': None,
             'gc_visits_and_thread_cpu_reason': 'not exposed by the existing profile snapshot API',
             'unrecorded_original_profile_environment': 'unknown; current effective values captured',
             'power_thermal': {'value': None, 'reason': 'not sampled; host power/thermal state unknown'},
             'host': {'platform': platform.platform(), 'machine': platform.machine(),
                      'runner': {k: os.environ.get(k) for k in
                                 ['RUNNER_NAME', 'RUNNER_OS', 'RUNNER_ARCH', 'ImageOS', 'ImageVersion']}}}
    report = allocation / 'result.json'
    save(report, state)
    try:
        host_commands = [['uname', '-a'], ['sw_vers'], ['sysctl', 'hw.model', 'hw.memsize', 'hw.ncpu'],
                         ['rustc', '--version', '--verbose'], ['cargo', '--version']]
        state['host_commands'] = [command(c, root, os.environ.copy(), allocation / f'host-{i}.log')
                                  for i, c in enumerate(host_commands)]
        rust = (allocation / 'host-3.log').read_text()
        if 'host: aarch64-apple-darwin' not in rust or 'rustc 1.98.1 ' not in rust:
            raise ValueError('Unexpected Rust host/version')
        state['preflight'] = module.preflight(root, observer)
        save(allocation / 'preflight.json', state['preflight'])
        paths = ['scripts/run-issue1506-pair.py', 'scripts/diagnose-issue1506-pair.py',
                 'scripts/issue1506-pair-spec.json', 'scripts/issue1506-observer.patch',
                 'scripts/test_issue1506_pair.py', '.github/workflows/jit-native.yml']
        state['diagnostic_source_sha256'] = {p: hashlib.sha256((root / p).read_bytes()).hexdigest() for p in paths}
        state['diagnostic_revision'] = subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip()
        worktrees = {}
        for label in ['before', 'after']:
            source = state['preflight']['sources'][label]
            checkout = allocation / 'sources' / label
            subprocess.run(['git', '-C', str(root), 'worktree', 'add', '--detach',
                            str(checkout), source['revision']], check=True)
            subprocess.run(['git', '-C', str(checkout), 'apply', str(observer)], check=True)
            # The only checkout change must be the common observer.
            diff = subprocess.check_output(['git', '-C', str(checkout), 'diff', '--binary'])
            (allocation / f'{label}-observer-applied.patch').write_bytes(diff)
            state.setdefault('applied_observers', {})[label] = hashlib.sha256(diff).hexdigest()
            worktrees[label] = checkout
        if len(set(state['applied_observers'].values())) != 1:
            raise ValueError('Applied observers differ')
        execute_variants(allocation, spec, state, worktrees, report)
        state['terminal'] = True
        exit_code = summarize_results(state)
        state['ended_utc'] = utc()
        save(report, state)
        return exit_code
    except BaseException as error:
        state.update(terminal=True, collection_completed=False, error=repr(error), ended_utc=utc())
        save(report, state)
        raise


if __name__ == '__main__':
    sys.exit(main())
