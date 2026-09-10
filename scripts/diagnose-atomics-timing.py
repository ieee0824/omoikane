"""Retain diagnostic failures; this is not a compatibility acceptance gate."""
from pathlib import Path
import hashlib
import json
import os
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / '.artifacts/atomics-timing'
ENGINE = ROOT / 'engine/boa'
SUITE = ROOT / 'test262'


def write(name, value):
    (OUT / name).write_text(json.dumps(value, indent=2) + '\n')


def verify_clock_fix(patch):
    """Prove the new regressions fail with the original scheduler clock."""
    subprocess.run(['git', 'apply', '--check', str(patch)], cwd=ROOT, check=True)
    subprocess.run(['git', 'apply', str(patch)], cwd=ROOT, check=True)
    job = ENGINE / 'core/engine/src/job.rs'
    fixed = job.read_text()
    assert fixed.count('clock().monotonic_now()') == 3
    job.write_text(fixed.replace('clock().monotonic_now()', 'clock().now()'))
    (OUT / 'before-scheduler.patch').write_bytes(subprocess.check_output(['git', 'diff'], cwd=ROOT))
    command = ['cargo', 'test', '--locked', '-p', 'boa_engine', '--lib', 'clock_domains', '--', '--nocapture']
    with (OUT / 'before-clock-regression.log').open('w') as log:
        before = subprocess.run(command, cwd=ENGINE, stdout=log, stderr=log)
    text = (OUT / 'before-clock-regression.log').read_text()
    assert before.returncode == 101 and 'test result: FAILED. 1 passed; 2 failed;' in text, 'expected exactly the two clock-domain failures'
    failure_section = text.rsplit('\nfailures:\n', 1)[1].split('\ntest result:', 1)[0]
    failed = {line.strip() for line in failure_section.splitlines() if line.strip()}
    assert failed == {
        'job::tests::clock_domains::forward_wall_adjustment_does_not_expire_a_timer_early',
        'job::tests::clock_domains::backward_wall_adjustment_does_not_delay_a_due_timer',
    }, failed
    job.write_text(fixed)
    (OUT / 'fixed-scheduler.patch').write_bytes(subprocess.check_output(['git', 'diff'], cwd=ROOT))
    for label, selection in [('after-clock-regression', 'clock_domains'), ('after-job-tests', 'job::tests'), ('after-clock-tests', 'context::time')]:
        with (OUT / (label + '.log')).open('w') as log:
            subprocess.run(['cargo', 'test', '--locked', '-p', 'boa_engine', '--lib', selection, '--', '--nocapture'],
                           cwd=ENGINE, stdout=log, stderr=log, check=True)
    text = (OUT / 'after-clock-regression.log').read_text()
    assert 'test result: ok. 3 passed; 0 failed;' in text
    write('clock-regression.json', {'before_scheduler': 'original wall-clock deadlines; new test and clock API present',
                                  'before_passed': 1, 'before_failed': 2, 'after_passed': 3, 'after_failed': 0,
                                  'candidate_patch_sha256': hashlib.sha256(patch.read_bytes()).hexdigest()})


def main():
    OUT.mkdir(parents=True, exist_ok=False)
    identity = {
        'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
        'test262_revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=SUITE, text=True).strip(),
        'toolchain': subprocess.check_output(['rustc', '-Vv'], text=True),
        'lock_sha256': hashlib.sha256((ENGINE / 'Cargo.lock').read_bytes()).hexdigest(),
        'purpose': 'diagnosis only; all failures retained, no acceptance retry',
    }
    assert identity['test262_revision'] == 'a073f479f80b336256b7fc4e04700c827293e2fe'
    patch = ROOT / 'scripts/diagnostics/issue657-monotonic.patch'
    identity['engine_source'] = 'proposed monotonic-clock patch' if patch.exists() else 'unmodified imported original'
    if patch.exists():
        identity['engine_patch_sha256'] = hashlib.sha256(patch.read_bytes()).hexdigest()
    write('identity.json', identity)
    if patch.exists():
        verify_clock_fix(patch)
    with (OUT / 'build.log').open('w') as log:
        subprocess.run(['cargo', 'build', '--locked', '--release', '--bin', 'boa_tester'],
                       cwd=ENGINE, stdout=log, stderr=log, check=True)
    binary = Path(os.environ['CARGO_TARGET_DIR']) / 'release/boa_tester'
    paths = sorted((SUITE / 'test/built-ins/Atomics/waitAsync').rglob('no-spurious-wakeup-on-*.js'))
    assert len(paths) >= 10
    write('original-cases.json', {str(p.relative_to(SUITE)): hashlib.sha256(p.read_bytes()).hexdigest() for p in paths})
    results = []
    for phase in ('original', 'detailed-assertions'):
        if phase == 'detailed-assertions':
            count = 0
            for path in paths:
                source = path.read_text()
                old = "'The result of evaluating `(lapse >= TIMEOUT)` is true'"
                new = "'Expected lapse >= TIMEOUT; lapse=' + lapse + ', TIMEOUT=' + TIMEOUT"
                if old in source:
                    path.write_text(source.replace(old, new))
                    count += 1
            assert count == len(paths), (count, len(paths))
            (OUT / 'diagnostic-messages.patch').write_bytes(subprocess.check_output(['git', 'diff'], cwd=SUITE))
        for repeat in range(10):
            for case in paths:
                name = str(case.relative_to(SUITE))
                log_name = f'{phase}-{repeat}-' + name.replace('/', '_') + '.log'
                command = [str(binary), 'run', '--test262-path', str(SUITE), '--suite', name, '-vvv']
                with (OUT / log_name).open('w') as log:
                    result = subprocess.run(command, cwd=ENGINE, stdout=log, stderr=log, timeout=60)
                text = re.sub(r'\x1b\[[0-9;]*m', '', (OUT / log_name).read_text())
                outcomes = re.findall(r'`[^`]+`(?: \(strict\))?: (Passed|Failed|Ignored|.*Panic.*)', text)
                assert outcomes and result.returncode == 0, log_name
                results.append({'phase': phase, 'repeat': repeat, 'case': name,
                                'outcomes': outcomes, 'log': log_name, 'exit': result.returncode})
                write('results.json', results)
            print(phase, repeat, 'recorded', flush=True)
    probe = OUT / 'clock-probe.rs'
    probe.write_text('''use std::time::{Duration, Instant, SystemTime};
fn main() {
    println!("sample,instant_elapsed_ns,system_elapsed_ns");
    for sample in 0..100 {
        let mono = Instant::now();
        let wall = SystemTime::now();
        let deadline = wall + Duration::from_millis(200);
        while SystemTime::now() < deadline { std::hint::spin_loop(); }
        let system_elapsed = SystemTime::now().duration_since(wall).unwrap();
        println!("{},{},{}", sample, mono.elapsed().as_nanos(), system_elapsed.as_nanos());
    }
}
''')
    executable = OUT / 'clock-probe'
    subprocess.run(['rustc', '-O', str(probe), '-o', str(executable)], check=True)
    with (OUT / 'clock-probe.csv').open('w') as log:
        subprocess.run([str(executable)], stdout=log, check=True)
    failed = [item for item in results if any(value != 'Passed' for value in item['outcomes'])]
    write('summary.json', {'executions': len(results), 'nonpassing_executions': len(failed), 'failures': failed})
    print('diagnostic complete:', len(results), 'executions,', len(failed), 'nonpassing; not an acceptance result')


if __name__ == '__main__':
    main()
