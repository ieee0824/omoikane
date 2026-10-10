#!/usr/bin/env python3
"""Check extracted CLDR payload pins and hashes before the locked Rust exporter runs."""
import argparse
import hashlib
import json
from pathlib import Path

from generate import CLDR_COMMIT, SOURCE_SHA256
from root_symbols import ROOT_COMMIT, ROOT_SHA256


def validate(directory):
    provenance = json.loads((directory / 'provenance.json').read_bytes())
    if provenance['source']['sha256'] != SOURCE_SHA256 or provenance['source']['commit'] != CLDR_COMMIT:
        raise ValueError('unexpected CLDR JSON source pin')
    if provenance['rootSource']['sha256'] != ROOT_SHA256 or provenance['rootSource']['commit'] != ROOT_COMMIT:
        raise ValueError('unexpected CLDR XML root source pin')
    required = {'number_symbols.json', 'number_patterns.json', 'currency_patterns.json', 'currency_names.json', 'currency_digits.json', 'locale_metadata.json', 'duration_unit_patterns.json', 'all_unit_patterns.json', 'root_number_symbols.json', 'calendar_date_patterns.json', 'calendar_interval_patterns.json', 'compact_patterns.json', 'number_range_patterns.json', 'relative_time_patterns.json', 'display_names.json'}
    if set(provenance['outputFiles']) != required:
        raise ValueError('the exporter requires the complete expanded dataset')
    for name, record in provenance['outputFiles'].items():
        if Path(name).name != name:
            raise ValueError('unexpected dataset filename')
        content = (directory / name).read_bytes()
        if len(content) != record['bytes'] or hashlib.sha256(content).hexdigest() != record['sha256']:
            raise ValueError(f'dataset hash mismatch: {name}')
    for name, expected in provenance['generatorFiles'].items():
        if Path(name).name != name:
            raise ValueError('unexpected generator filename')
        if hashlib.sha256((Path(__file__).parent / name).read_bytes()).hexdigest() != expected:
            raise ValueError(f'generator hash mismatch: {name}')
    print('CLDR payload pins and all fifteen dataset hashes: PASS')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    validate(args.directory)
