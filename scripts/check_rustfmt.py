#!/usr/bin/env python3
"""Check the committed root crate with rustfmt without its diff emitter.

Formatting a temporary archive and comparing hashes avoids rustfmt's
quadratic diff allocation for very large source files. It also leaves the
caller's working tree untouched.
"""

from __future__ import annotations

import argparse
import hashlib
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tarfile
import tempfile


PINNED_RUSTC = "rustc 1.98.1 (48a229cea 2026-09-01)"
PINNED_RUSTFMT = "rustfmt 1.9.0-stable (48a229ceae 2026-09-01)"
ABORT_MARKERS = (
    "memory allocation of",
    "rust_oom",
    "sigabrt",
    "aborted (core dumped)",
    "signal: 6",
)


def has_abort_marker(output: str) -> bool:
    """Return whether a formatter log shows a crash hidden by cargo-fmt."""
    lowered = output.lower()
    return any(marker in lowered for marker in ABORT_MARKERS)


def rust_hashes(root: Path) -> dict[str, str]:
    """Hash every Rust source below `root` without retaining file contents."""
    hashes = {}
    for path in sorted(root.rglob("*.rs")):
        if path.is_file():
            relative = path.relative_to(root).as_posix()
            hashes[relative] = hashlib.sha256(path.read_bytes()).hexdigest()
    return hashes


def command_output(command: list[str], cwd: Path) -> str:
    result = subprocess.run(command, cwd=cwd, check=True, capture_output=True, text=True)
    return result.stdout.strip()


def extract_revision(repository: Path, revision: str, destination: Path) -> None:
    archive_path = destination.parent / "source.tar"
    with archive_path.open("wb") as archive:
        subprocess.run(
            ["git", "-C", str(repository), "archive", "--format=tar", revision],
            check=True,
            stdout=archive,
        )
    with tarfile.open(archive_path) as archive:
        # The archive is produced locally by `git archive`, so its paths are
        # trusted. Avoid the Python 3.12-only extraction filter for CI images
        # and development environments that still use Python 3.11.
        archive.extractall(destination)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--revision", default="HEAD", help="committed revision to check")
    args = parser.parse_args()

    repository = Path(
        subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
    )

    with tempfile.TemporaryDirectory(prefix="omoikane-rustfmt-") as directory:
        checkout = Path(directory) / "source"
        checkout.mkdir()
        extract_revision(repository, args.revision, checkout)

        rustc_version = command_output(["rustc", "--version"], checkout)
        rustfmt_version = command_output(["rustfmt", "--version"], checkout)
        if rustc_version != PINNED_RUSTC or rustfmt_version != PINNED_RUSTFMT:
            print(
                "pinned formatter toolchain is unavailable:\n"
                f"  expected {PINNED_RUSTC}\n"
                f"  actual   {rustc_version}\n"
                f"  expected {PINNED_RUSTFMT}\n"
                f"  actual   {rustfmt_version}",
                file=sys.stderr,
            )
            return 2

        before = rust_hashes(checkout)
        cargo = shlex.split(os.environ.get("OMOIKANE_RUSTFMT_CARGO", "cargo"))
        result = subprocess.run(
            [*cargo, "fmt", "--package", "omoikane"],
            cwd=checkout,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        if result.stdout:
            print(result.stdout, end="")
        if result.returncode != 0 or has_abort_marker(result.stdout):
            print(
                f"rustfmt failed (reported status {result.returncode})",
                file=sys.stderr,
            )
            return 2

        after = rust_hashes(checkout)
        changed = sorted(
            path
            for path in before.keys() | after.keys()
            if before.get(path) != after.get(path)
        )
        if changed:
            print("rustfmt would change these committed files:", file=sys.stderr)
            for path in changed:
                print(f"  {path}", file=sys.stderr)
            return 1

    print(f"rustfmt check passed for {args.revision}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
