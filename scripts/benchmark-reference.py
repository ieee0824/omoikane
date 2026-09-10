#!/usr/bin/env python3
"""Validate Firefox benchmark values before producing reference timing data."""

import argparse
import hashlib
import json
import math
from pathlib import Path
import platform
import re

ROOT = Path(__file__).resolve().parents[1]


def fixture_contract(source):
    """Read the fixture's deliberately JSON-shaped declarations."""
    expected = json.loads(re.search(r'globalThis.BENCH_EXPECTED = (\{.*?\});', source, re.S)[1])
    version = int(re.search(r'globalThis.BENCH_FIXTURE_VERSION = (\d+);', source)[1])
    passes = int(re.search(r'globalThis.BENCH_PASSES = (\d+);', source)[1])
    return expected, passes, {"version": version, "sha256": hashlib.sha256(source.encode()).hexdigest()}


def validate_samples(raw, runs, expected):
    """Reject incomplete, duplicate, misidentified, or incorrectly computed rows."""
    if runs < 1:
        raise ValueError("runs must be positive")
    samples = []
    seen = set()
    for line in raw.splitlines():
        fields = line.split('|')
        if len(fields) != 7:
            raise ValueError(f"malformed benchmark row: {line}")
        mode, run, shape, iterations, elapsed, ns_per_op, result = fields
        run, iterations = int(run), int(iterations)
        elapsed, ns_per_op, result = float(elapsed), float(ns_per_op), float(result)
        key = (mode, run, shape)
        if mode not in ('interpreter', 'jit') or not 1 <= run <= runs or shape not in expected:
            raise ValueError(f"unexpected benchmark identity: {key}")
        if key in seen:
            raise ValueError(f"duplicate benchmark shape: {key}")
        if not all(math.isfinite(v) for v in (elapsed, ns_per_op, result)) or elapsed <= 0 or ns_per_op <= 0:
            raise ValueError(f"invalid benchmark measurement: {line}")
        if [iterations, result] != expected[shape]:
            raise ValueError(f"incorrect benchmark result or iterations: {line}")
        seen.add(key)
        samples.append(dict(mode=mode, run=run, id=shape, iterations=iterations,
                            elapsed_ms=elapsed, ns_per_op=ns_per_op, result=result))
    required = {(m, r, s) for m in ('interpreter', 'jit') for r in range(1, runs + 1) for s in expected}
    if seen != required:
        raise ValueError(f"missing benchmark shapes: {sorted(required - seen)}")
    return samples


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('raw', type=Path)
    parser.add_argument('--runs', required=True, type=int)
    parser.add_argument('--firefox-version', required=True)
    parser.add_argument('--report', type=Path)
    parser.add_argument('--expected-fixture-sha256', required=True)
    args = parser.parse_args()
    expected, passes, fixture = fixture_contract((ROOT / 'tests/js_benchmark/shapes.js').read_text())
    if fixture['sha256'] != args.expected_fixture_sha256:
        raise ValueError("fixture changed during measurement")
    samples = validate_samples(args.raw.read_text(), args.runs, expected)
    minima = {mode: {shape: min(s['ns_per_op'] for s in samples if s['mode'] == mode and s['id'] == shape)
                     for shape in expected} for mode in ('interpreter', 'jit')}
    report = dict(fixture=fixture, passes=passes, measurement_runs=args.runs,
                  reference_engine=args.firefox_version, target_arch=platform.machine(),
                  target_os=platform.system().lower(), samples=samples, minimum_ns_per_op=minima)
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(f"fixture_version|{fixture['version']}\nfixture_sha256|{fixture['sha256']}")
    print(f"reference_engine|{args.firefox_version}\nmeasurement_runs|{args.runs}\nminimum_ns_per_op")
    for mode, values in minima.items():
        for shape, value in values.items():
            print(f"{mode}|{shape}|{value}")


if __name__ == '__main__':
    main()
