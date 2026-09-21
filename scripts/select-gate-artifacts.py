#!/usr/bin/env python3
"""Select the latest attempt of each gate artifact before aggregation."""

import argparse
import filecmp
import json
from pathlib import Path
import re
import shutil
import sys


ATTEMPT_SUFFIX = re.compile(r"^(.+)-attempt-([1-9][0-9]*)$")


def latest_artifacts(source: Path, prefix: str) -> dict[str, tuple[int, Path]]:
    """Return the highest numbered attempt for every logical artifact."""
    latest: dict[str, tuple[int, Path]] = {}
    if not source.is_dir():
        raise ValueError(f"artifact source directory does not exist: {source}")
    for path in sorted(source.iterdir()):
        if not path.is_dir() or not path.name.startswith(prefix):
            continue
        match = ATTEMPT_SUFFIX.fullmatch(path.name.removeprefix(prefix))
        if match is None:
            raise ValueError(f"invalid attempted artifact name: {path.name}")
        logical_name, attempt_text = match.groups()
        attempt = int(attempt_text)
        previous = latest.get(logical_name)
        if previous is None or attempt > previous[0]:
            latest[logical_name] = (attempt, path)
    if not latest:
        raise ValueError(f"no attempted artifacts with prefix {prefix!r} in {source}")
    return latest


def copy_file_checked(source: str, destination: str) -> str:
    """Copy a file while rejecting conflicting evidence from another shard."""
    destination_path = Path(destination)
    if destination_path.exists():
        if not filecmp.cmp(source, destination, shallow=False):
            raise ValueError(f"conflicting artifact path: {destination_path}")
        return destination
    return shutil.copy2(source, destination)


def select_and_merge(source: Path, destination: Path, prefix: str) -> dict[str, int]:
    selected = latest_artifacts(source, prefix)
    destination.mkdir(parents=True, exist_ok=True)
    for logical_name in sorted(selected):
        _, artifact = selected[logical_name]
        shutil.copytree(
            artifact,
            destination,
            dirs_exist_ok=True,
            copy_function=copy_file_checked,
        )
    return {name: selected[name][0] for name in sorted(selected)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--prefix", required=True)
    args = parser.parse_args()
    try:
        selected = select_and_merge(args.source, args.destination, args.prefix)
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1
    print(json.dumps({"selected_attempts": selected}, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
