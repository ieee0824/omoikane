"""Trace the CDP/runtime deadlines with an explicit scheduling delay on x86 CI."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

root = Path('.artifacts/issue1142')
root.mkdir(parents=True, exist_ok=True)
cdp_path = Path('src/cdp/mod.rs')
js_path = Path('src/js/mod.rs')
cdp_original = cdp_path.read_text()
js_original = js_path.read_text()

def replace_once(text, old, new):
    assert text.count(old) == 1, (old[:100], text.count(old))
    return text.replace(old, new, 1)

def instant_ns(line, name):
    match = re.search(rf'{name}=Instant \{{ tv_sec: (\d+), tv_nsec: (\d+) \}}', line)
    assert match is not None, (name, line)
    return int(match[1]) * 1_000_000_000 + int(match[2])

def timing_summary(trace):
    requests = [line for line in trace if 'before_delay=' in line]
    runtimes = [line for line in trace if 'runtime_deadline=' in line]
    polls = [line for line in trace if 'poll_at=' in line]
    assert len(requests) == len(runtimes) == 1 and polls, trace
    request = instant_ns(requests[0], 'request_deadline')
    runtime = instant_ns(runtimes[0], 'runtime_deadline')
    final_poll = instant_ns(polls[-1], 'poll_at')
    return {'runtime_minus_request_ms': (runtime - request) / 1_000_000,
            'final_poll_minus_request_ms': (final_poll - request) / 1_000_000,
            'final_poll_minus_runtime_ms': (final_poll - runtime) / 1_000_000}

def instrument_cdp(text):
    text = replace_once(text, '''    Box::pin(async move {
        let result = {
            let mut evaluation =
                Box::pin(session.evaluate_expression_async(&expression, return_by_value));''', '''    Box::pin(async move {
        let result = {
            eprintln!("ISSUE1023 request_deadline={timeout_deadline:?} before_delay={:?}", Instant::now());
            // Model a scheduling gap between accepting the request and starting JS.
            std::thread::sleep(Duration::from_millis(100));
            let mut evaluation =
                Box::pin(session.evaluate_expression_async(&expression, return_by_value));''')
    return replace_once(text, '''            std::future::poll_fn(|context| {
                if cancelled.get() {''', '''            std::future::poll_fn(|context| {
                eprintln!("ISSUE1023 request_deadline={timeout_deadline:?} poll_at={:?}", Instant::now());
                if cancelled.get() {''')

js_traced = replace_once(js_original, '''        let source = source.to_owned();
        let deadline = execution_deadline(self.sandbox.timeout);''', '''        let source = source.to_owned();
        let deadline = execution_deadline(self.sandbox.timeout);
        eprintln!("ISSUE1023 runtime_deadline={deadline:?} runtime_start={:?}", Instant::now());''')

before = replace_once(cdp_original, '''                if Instant::now() >= timeout_deadline {
                    return Poll::Ready(Err(evaluation_timeout_error()));
                }
                let result = evaluation.as_mut().poll(context);
                if Instant::now() >= timeout_deadline {
                    Poll::Ready(Err(evaluation_timeout_error()))
                } else {
                    result
                }''', '''                evaluation.as_mut().poll(context)''')

command = ['cargo', 'test', '--locked', '--features', 'baseline-jit,jit-differential',
           '--lib', 'cdp::tests::browser_session_times_out_a_suspended_runtime_evaluation',
           '--', '--exact', '--nocapture', '--test-threads=1']
report = {'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
          'rustc': subprocess.check_output(['rustc', '--version', '--verbose'], text=True),
          'cdp_source_sha256': hashlib.sha256(cdp_original.encode()).hexdigest(),
          'js_source_sha256': hashlib.sha256(js_original.encode()).hexdigest(),
          'delay_ms': 100, 'command': command, 'runs': []}
load = None
affinity = None
try:
    js_path.write_text(js_traced)
    for stage, source, repeats in [('before', before, 1), ('after', cdp_original, 5)]:
        cdp_path.write_text(instrument_cdp(source))
        patch = subprocess.check_output(['git', 'diff', '--', str(cdp_path), str(js_path)])
        (root / f'{stage}-instrumentation.patch').write_bytes(patch)
        for index in range(repeats):
            if stage == 'after' and index == 1:
                affinity = os.sched_getaffinity(0)
                os.sched_setaffinity(0, {min(affinity)})
                load = subprocess.Popen([sys.executable, '-c', 'while True: sum(range(20000))'])
            log_path = root / f'{stage}-{index}.log'
            started = time.monotonic()
            with log_path.open('w') as log:
                result = subprocess.run(command, stdout=log, stderr=log, timeout=2400)
            output = log_path.read_text(errors='replace')
            trace = [line for line in output.splitlines() if 'ISSUE1023' in line]
            timing = timing_summary(trace)
            row = {'stage': stage, 'index': index, 'exit': result.returncode,
                   'seconds_including_build': time.monotonic() - started, 'trace': trace,
                   'timing': timing,
                   'same_cpu_contention': load is not None,
                   'instrumentation_sha256': hashlib.sha256(patch).hexdigest()}
            report['runs'].append(row)
            (root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
            assert any('runtime_deadline=' in line for line in trace), row
            assert any('poll_at=' in line for line in trace), row
            if stage == 'before':
                assert result.returncode == 101 and 'left: 1' in output and 'right: 0' in output, row
                assert 'test result: FAILED. 0 passed; 1 failed;' in output, row
                assert timing['final_poll_minus_request_ms'] >= 0, row
                assert timing['final_poll_minus_runtime_ms'] < 0, row
            else:
                assert result.returncode == 0 and 'test result: ok. 1 passed;' in output, row
                assert timing['final_poll_minus_request_ms'] >= 0, row
            print(stage, index, result.returncode, 'as expected', flush=True)
    report['status'] = 'passed'
finally:
    if load is not None:
        load.terminate()
        load.wait()
    if affinity is not None:
        os.sched_setaffinity(0, affinity)
    cdp_path.write_text(cdp_original)
    js_path.write_text(js_original)
    (root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
