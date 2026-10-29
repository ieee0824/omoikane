#!/usr/bin/env python3
"""Mark restored build outputs of in-tree path dependencies as fresh.

Cargo decides whether a path dependency is up to date by comparing the
modification time of its sources with that of its recorded dep-info. A fresh
checkout gives every source the checkout time, which is newer than any
restored output, so Cargo rebuilds Boa and the other path dependencies even
when the cache holds outputs built from identical sources (#1537).

Run this only after an exact cache hit whose key covers every input of these
crates. It sets one timestamp on every restored file of the listed crates, so
no output looks older than its sources or than another restored output.
"""

import argparse
import os
from pathlib import Path
import time

# Name prefixes of the in-tree path dependency crates whose outputs the Gate 5
# Boa cache restores. Keep in sync with the cache paths in
# .github/workflows/jit-release-gate.yml.
CRATE_PREFIXES = ('boa_', 'regress-', 'small_btree-', 'tag_ptr-')
OUTPUT_DIRECTORIES = ('.fingerprint', 'build', 'deps')


def is_path_dependency_output(name):
    """Whether a target entry belongs to one of the cached path dependencies."""
    return name.removeprefix('lib').startswith(CRATE_PREFIXES)


def restored_outputs(profile_directory):
    """Yield every file and directory restored for the cached path dependencies."""
    for directory in OUTPUT_DIRECTORIES:
        root = Path(profile_directory) / directory
        if not root.is_dir():
            continue
        for entry in sorted(root.iterdir()):
            if not is_path_dependency_output(entry.name):
                continue
            yield entry
            if entry.is_dir() and not entry.is_symlink():
                yield from sorted(entry.rglob('*'))


def refresh(profile_directory, timestamp_ns):
    """Give all restored outputs the same timestamp and return how many changed."""
    count = 0
    for path in restored_outputs(profile_directory):
        os.utime(path, ns=(timestamp_ns, timestamp_ns), follow_symlinks=False)
        count += 1
    return count


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('profile_directory', type=Path,
                        help='Cargo profile output directory, such as target/debug')
    args = parser.parse_args()
    count = refresh(args.profile_directory, time.time_ns())
    if count == 0:
        raise SystemExit('no restored path dependency outputs found')
    print(f'refreshed {count} restored path dependency outputs')


if __name__ == '__main__':
    main()
