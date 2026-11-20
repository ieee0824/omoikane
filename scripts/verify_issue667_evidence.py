#!/usr/bin/env python3
"""Verify archived #667 samples without historical binaries or local /tmp paths.

This validates recorded results and ratios, not CPU causality or PNG pixels.
Only Python's standard library is required. No files are modified.
"""
import argparse
import gzip
import hashlib
import json
from pathlib import Path, PurePosixPath
import statistics


def check_stat(saved, changes):
    """Recompute every paired ratio and its published summary."""
    assert saved['paired_changes'] == changes
    assert saved['median_change'] == statistics.median(changes)
    assert saved['slower_pairs'] == sum(x > 0 for x in changes)


def verify_native(files):
    """Validate all fixed pairs, computed results, diagnostics, and summaries."""
    read = lambda name: json.loads(files[name])
    benches, pages, summary = [read(name) for name in
                              ['benchmark-index.json', 'page-index.json', 'summary.json']]
    assert len(benches) == 40 and len(pages) == 80
    assert all(r['exit'] == 0 for r in benches + pages)
    b = {(r['mode'], r['pair'], r['variant']): r for r in benches}
    p = {(r['mode'], r['pair'], r['variant']): r for r in pages}
    assert len(b) == 40 and len(p) == 80
    common_page = pages[0]
    for r in pages:
        assert r['report']['status'] == 'passed'
        assert r['report'] == common_page['report']
        assert len(r['png_sha256']) == 2 and r['png_sha256'] == common_page['png_sha256']
    for mode in ['off', 'on']:
        reports = {}
        assert len(summary[mode]['shapes']) == 11
        for pair in range(20):
            order = ['baseline', 'candidate'] if pair % 2 == 0 else ['candidate', 'baseline']
            assert all(p[mode, pair, v]['order'] == order for v in order)
        for pair in range(10):
            order = ['baseline', 'candidate'] if pair % 2 == 0 else ['candidate', 'baseline']
            for v in order:
                assert b[mode, pair, v]['order'] == order
                report = read(f'benchmarks/{mode}/{pair}/{v}/benchmark.json')
                assert report['fixture']['sha256'] == '177eb80ae73d1081eb620f747b4703e2a9f2731e0255bb60131b73b7dcac6464'
                assert report['measured_passes'] == 4 and report['measurement_runs'] == 1
                assert report['jit_diagnostics']['enabled'] == (mode == 'on')
                assert len(report['shapes']) == 11
                assert {s['id'] for s in report['shapes']} == set(summary[mode]['shapes'])
                reports[pair, v] = report
            a, c = [reports[pair, v] for v in ['baseline', 'candidate']]
            assert {s['id']: s['result'] for s in a['shapes']} == {s['id']: s['result'] for s in c['shapes']}
            diag = lambda r: {k: v for k, v in r['jit_diagnostics'].items() if k != 'total_compile_time_ns'}
            assert diag(a) == diag(c)
        for shape, saved in summary[mode]['shapes'].items():
            values = lambda pair, v: next(s['ns_per_op'] for s in reports[pair, v]['shapes'] if s['id'] == shape)
            check_stat(saved, [values(i, 'candidate') / values(i, 'baseline') - 1 for i in range(10)])
        check_stat(summary[mode]['whole_benchmark_process'],
                   [b[mode, i, 'candidate']['seconds']/b[mode, i, 'baseline']['seconds']-1 for i in range(10)])
        check_stat(summary[mode]['page_process'],
                   [p[mode, i, 'candidate']['seconds']/p[mode, i, 'baseline']['seconds']-1 for i in range(20)])
    return 120


def verify_gc(files):
    """Check all 320 recorded passes; counts are not elapsed-time attribution."""
    records = json.loads(files['gc-paired/index.json'])
    assert len(records) == 80 and all(r['exit'] == 0 for r in records)
    lookup = {(r['mode'], r['shape'], r['pair'], r['variant']): r for r in records}
    assert len(lookup) == 80
    for mode in ['off', 'on']:
        for shape in ['array', 'closure-alloc']:
            for pair in range(10):
                a, b = [lookup[mode, shape, pair, v] for v in ['baseline', 'candidate']]
                assert len(a['passes']) == len(b['passes']) == 4
                for x, y in zip(a['passes'], b['passes']):
                    for phase in ['minor', 'major']:
                        assert x[phase]['collections'] == y[phase]['collections']
    return 320


def main():
    if not __debug__:
        raise RuntimeError('Run without -O: assertion-based evidence checks must remain enabled')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, default=Path(__file__).resolve().parents[1]/'docs/jit/measurements/issue667')
    args = parser.parse_args()
    manifest = json.loads((args.directory/'manifest.json').read_text())
    assert manifest['version'] == 1 and len(manifest['bundles']) == 7
    for name, meta in manifest['bundles'].items():
        assert PurePosixPath(name).name == name
        data = (args.directory/name).read_bytes()
        assert len(data) == meta['bytes'] and hashlib.sha256(data).hexdigest() == meta['sha256'], name
        files = json.loads(gzip.decompress(data))
        assert len(files) == meta['files']
        assert all(not PurePosixPath(p).is_absolute() and '..' not in PurePosixPath(p).parts for p in files)
        count = verify_gc(files) if name == 'diagnostics.json.gz' else verify_native(files)
        print(f'{name}: verified {count} '+('GC passes' if name == 'diagnostics.json.gz' else 'process records'))


if __name__ == '__main__':
    main()
