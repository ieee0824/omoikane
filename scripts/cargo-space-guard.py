#!/usr/bin/env python3
"""Run Cargo with disk limits and clean only validated Cargo target caches."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import fcntl
import math
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time
from typing import Callable, Protocol, Sequence


GIB = 1024 ** 3
CAPACITY_EXIT = 75
LOCK_NAME = ".cargo-space-guard.lock"
PRESERVE_NAME = ".cargo-space-guard-preserve"
TARGET_MARKERS = ("CACHEDIR.TAG", ".rustc_info.json")
SOURCE_MARKERS = (".git", "Cargo.toml", ".artifacts", "images", "evidence")
CACHE_TAG = """Signature: 8a477f597d28d172789f06886806bc55
# This file is a cache directory tag created by cargo-space-guard.
"""


@dataclass(frozen=True)
class TargetCache:
    path: Path
    last_used: float
    size: int


class DiskUsage(Protocol):
    free: int


def gib(value: str | float) -> int:
    try:
        number = float(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("size must be a number") from error
    if not math.isfinite(number) or number < 0:
        raise argparse.ArgumentTypeError("size must be a finite non-negative number")
    return int(number * GIB)


def display_size(value: int) -> str:
    return f"{value / GIB:.1f} GiB"


def directory_size(root: Path) -> int:
    total = 0
    seen = set()
    if not root.exists():
        return total
    for directory, _, files in os.walk(root):
        for name in files:
            try:
                path = Path(directory, name)
                if not path.is_symlink():
                    details = path.stat()
                    identity = (details.st_dev, details.st_ino)
                    if identity not in seen:
                        seen.add(identity)
                        total += details.st_blocks * 512
            except FileNotFoundError:
                pass
    return total


def validate_run_target(path: Path, workspace: Path) -> Path:
    if path.expanduser().is_symlink():
        raise ValueError("target must not be a symbolic link")
    target = path.expanduser().resolve()
    source = workspace.resolve()
    if target == Path(target.anchor) or target.parent == Path(target.anchor):
        raise ValueError("target must be a dedicated directory below a cache root")
    if target == source or source in target.parents or target in source.parents:
        raise ValueError("target must be separate from the source worktree")
    return target


def mark_target(target: Path) -> None:
    target.mkdir(parents=True, exist_ok=True)
    marker = target / "CACHEDIR.TAG"
    if not marker.exists():
        marker.write_text(CACHE_TAG)
    else:
        marker.touch()


def lock_target(target: Path, blocking: bool):
    lock = (target / LOCK_NAME).open("a+")
    operation = fcntl.LOCK_EX | (0 if blocking else fcntl.LOCK_NB)
    try:
        fcntl.flock(lock.fileno(), operation)
    except BlockingIOError:
        lock.close()
        return None
    return lock


def stop_process_group(process: subprocess.Popen) -> None:
    if process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=10)
    except (ProcessLookupError, subprocess.TimeoutExpired):
        if process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()


def run_guarded(
    command: Sequence[str],
    target: Path,
    workspace: Path,
    required_free: int,
    minimum_free: int,
    maximum_target: int,
    poll_seconds: float,
    usage: Callable[[Path], DiskUsage] = shutil.disk_usage,
    size: Callable[[Path], int] = directory_size,
) -> int:
    target = validate_run_target(target, workspace)
    mark_target(target)
    target_lock = lock_target(target, blocking=False)
    if target_lock is None:
        print(f"cargo-space-guard: target is already in use: {target}", file=sys.stderr)
        return CAPACITY_EXIT

    process = None
    try:
        initial = usage(target)
        initial_target_size = size(target)
        effective_required = max(
            required_free,
            minimum_free + max(0, maximum_target - initial_target_size),
        )
        if initial.free < effective_required:
            print(
                "cargo-space-guard: insufficient space before build: "
                f"free={display_size(initial.free)}, required={display_size(effective_required)}, "
                f"target={target}",
                file=sys.stderr,
            )
            return CAPACITY_EXIT
        if initial_target_size > maximum_target:
            print(
                "cargo-space-guard: target already exceeds limit: "
                f"size={display_size(initial_target_size)}, limit={display_size(maximum_target)}, "
                f"target={target}",
                file=sys.stderr,
            )
            return CAPACITY_EXIT

        environment = dict(os.environ)
        environment["CARGO_TARGET_DIR"] = str(target)
        process = subprocess.Popen(command, cwd=workspace, env=environment, start_new_session=True)
        while process.poll() is None:
            current = usage(target)
            target_size = size(target)
            reason = None
            if current.free < minimum_free:
                reason = (
                    f"free space {display_size(current.free)} is below "
                    f"{display_size(minimum_free)}"
                )
            elif target_size > maximum_target:
                reason = (
                    f"target size {display_size(target_size)} exceeds "
                    f"{display_size(maximum_target)}"
                )
            if reason is not None:
                stop_process_group(process)
                print(
                    f"cargo-space-guard: stopped build: {reason}; target={target}",
                    file=sys.stderr,
                )
                return CAPACITY_EXIT
            time.sleep(poll_seconds)
        return process.returncode
    except KeyboardInterrupt:
        if process is not None:
            stop_process_group(process)
        return 130
    finally:
        target_lock.close()


def target_caches(cache_root: Path) -> list[TargetCache]:
    root = cache_root.expanduser().resolve()
    if root == Path(root.anchor) or not root.is_dir():
        raise ValueError("cache root must be an existing directory below the filesystem root")
    caches = []
    for path in root.iterdir():
        if path.is_symlink() or not path.is_dir():
            continue
        if (path / PRESERVE_NAME).exists() or any((path / name).exists() for name in SOURCE_MARKERS):
            continue
        if not (path / LOCK_NAME).is_file():
            continue
        markers = [path / marker for marker in TARGET_MARKERS if (path / marker).is_file()]
        if not markers:
            continue
        caches.append(
            TargetCache(
                path,
                max(marker.stat().st_mtime for marker in markers),
                directory_size(path),
            )
        )
    return caches


def clean_targets(
    cache_root: Path,
    keep: set[Path],
    retention_seconds: float,
    maximum_total: int,
    execute: bool,
    now: float | None = None,
) -> list[TargetCache]:
    keep = {path.expanduser().resolve() for path in keep}
    caches = target_caches(cache_root)
    total = sum(cache.size for cache in caches)
    projected = total
    cutoff = (time.time() if now is None else now) - retention_seconds
    selected = []
    removed = []
    for cache in sorted(caches, key=lambda item: item.last_used):
        if cache.path in keep:
            continue
        if cache.last_used >= cutoff and projected <= maximum_total:
            continue
        if not execute:
            selected.append(cache)
            projected -= cache.size
            continue
        target_lock = lock_target(cache.path, blocking=False)
        if target_lock is None:
            continue
        try:
            shutil.rmtree(cache.path)
            removed.append(cache)
            projected -= cache.size
        finally:
            target_lock.close()
    return removed if execute else selected


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    commands = result.add_subparsers(dest="operation", required=True)

    run = commands.add_parser("run", help="run a command with Cargo target disk limits")
    run.add_argument("--target-dir", type=Path, default=os.environ.get("CARGO_TARGET_DIR"))
    run.add_argument("--required-free-gib", type=gib, default=gib(22))
    run.add_argument("--minimum-free-gib", type=gib, default=gib(2))
    run.add_argument("--maximum-target-gib", type=gib, default=gib(20))
    run.add_argument("--poll-seconds", type=float, default=5)
    run.add_argument("command", nargs=argparse.REMAINDER)

    clean = commands.add_parser("clean", help="list or remove validated Cargo target caches")
    clean.add_argument("--cache-root", type=Path, required=True)
    clean.add_argument("--keep", type=Path, action="append", default=[])
    clean.add_argument("--retention-hours", type=float, default=24 * 7)
    clean.add_argument("--maximum-total-gib", type=gib, default=gib(60))
    clean.add_argument("--execute", action="store_true")
    return result


def main(arguments: Sequence[str] | None = None) -> int:
    options = parser().parse_args(arguments)
    try:
        if options.operation == "run":
            command = list(options.command)
            if command and command[0] == "--":
                command.pop(0)
            if options.target_dir is None or not command:
                parser().error(
                    "run requires --target-dir (or CARGO_TARGET_DIR) and a command after --"
                )
            if options.poll_seconds <= 0:
                parser().error("--poll-seconds must be positive")
            if options.minimum_free_gib > options.required_free_gib:
                parser().error("minimum free space must not exceed required starting space")
            return run_guarded(
                command,
                options.target_dir,
                Path.cwd(),
                options.required_free_gib,
                options.minimum_free_gib,
                options.maximum_target_gib,
                options.poll_seconds,
            )

        if not math.isfinite(options.retention_hours) or options.retention_hours < 0:
            parser().error("--retention-hours must be a finite non-negative number")
        removed = clean_targets(
            options.cache_root,
            set(options.keep),
            options.retention_hours * 3600,
            options.maximum_total_gib,
            options.execute,
        )
        action = "removed" if options.execute else "would remove"
        for cache in removed:
            print(f"cargo-space-guard: {action} {cache.path} ({display_size(cache.size)})")
        if not removed:
            print("cargo-space-guard: no eligible target caches")
        return 0
    except (OSError, ValueError) as error:
        print(f"cargo-space-guard: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
