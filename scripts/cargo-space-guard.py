#!/usr/bin/env python3
"""Run Cargo with isolated disk limits and recover validated target caches."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import fcntl
import json
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
BINDING_NAME = ".cargo-space-guard-worktree.json"
BINDING_VERSION = 1
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


@dataclass(frozen=True)
class TargetBinding:
    workspace: str
    uid: int
    gid: int


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


def worktree_root(path: Path) -> Path:
    source = path.resolve()
    for candidate in (source, *source.parents):
        if (candidate / ".git").exists():
            return candidate
    return source


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
    source = worktree_root(workspace)
    if target == Path(target.anchor) or target.parent == Path(target.anchor):
        raise ValueError("target must be a dedicated directory below a cache root")
    if target == source or source in target.parents or target in source.parents:
        raise ValueError("target must be separate from the source worktree")
    return target


def current_binding(workspace: Path) -> TargetBinding:
    return TargetBinding(str(worktree_root(workspace)), os.geteuid(), os.getegid())


def read_binding(target: Path) -> TargetBinding | None:
    marker = target / BINDING_NAME
    if not marker.exists():
        return None
    try:
        value = json.loads(marker.read_text())
        if (
            not isinstance(value, dict)
            or value.get("version") != BINDING_VERSION
            or not isinstance(value.get("workspace"), str)
            or not isinstance(value.get("uid"), int)
            or not isinstance(value.get("gid"), int)
        ):
            raise ValueError
    except (OSError, UnicodeError, json.JSONDecodeError, ValueError) as error:
        raise ValueError(f"invalid target binding: {marker}") from error
    return TargetBinding(value["workspace"], value["uid"], value["gid"])


def write_binding(target: Path, binding: TargetBinding) -> None:
    marker = target / BINDING_NAME
    temporary = target / f".{BINDING_NAME}.{os.getpid()}.tmp"
    value = {
        "version": BINDING_VERSION,
        "workspace": binding.workspace,
        "uid": binding.uid,
        "gid": binding.gid,
    }
    try:
        temporary.write_text(json.dumps(value, sort_keys=True) + "\n")
        os.replace(temporary, marker)
    finally:
        temporary.unlink(missing_ok=True)


def has_build_artifacts(target: Path) -> bool:
    metadata = {"CACHEDIR.TAG", LOCK_NAME, PRESERVE_NAME, BINDING_NAME}
    return any(path.name not in metadata for path in target.iterdir())


def target_access_problem(target: Path, expected_uid: int) -> str | None:
    def check(path: Path, directory: bool) -> str | None:
        try:
            details = path.lstat()
        except FileNotFoundError:
            return None
        if path.is_symlink():
            return None
        if details.st_uid != expected_uid:
            return f"owner uid {details.st_uid} differs from expected uid {expected_uid}: {path}"
        access = os.W_OK | (os.X_OK if directory else 0)
        if not os.access(path, access, effective_ids=True):
            return f"not writable by uid {expected_uid}: {path}"
        return None

    problem = check(target, True)
    if problem is not None:
        return problem

    def fail(error: OSError) -> None:
        raise error

    for directory, directories, files in os.walk(target, onerror=fail):
        root = Path(directory)
        for name in directories:
            problem = check(root / name, True)
            if problem is not None:
                return problem
        for name in files:
            problem = check(root / name, False)
            if problem is not None:
                return problem
    return None


def prepare_target(target: Path, workspace: Path) -> None:
    target.mkdir(parents=True, exist_ok=True)
    expected = current_binding(workspace)
    binding = read_binding(target)
    if binding is None:
        if has_build_artifacts(target):
            raise ValueError(
                "target contains build artifacts but is not bound to a worktree; "
                f"reset it before reuse: {target}"
            )
        write_binding(target, expected)
    else:
        if binding.workspace != expected.workspace:
            raise ValueError(
                "target belongs to another worktree: "
                f"recorded={binding.workspace}, current={expected.workspace}, target={target}"
            )
        if binding.uid != expected.uid:
            raise ValueError(
                "target belongs to another uid: "
                f"recorded={binding.uid}, current={expected.uid}, target={target}"
            )

    problem = target_access_problem(target, expected.uid)
    if problem is not None:
        raise ValueError(f"target ownership or permissions are inconsistent: {problem}")
    mark_target(target)


def mark_target(target: Path) -> None:
    target.mkdir(parents=True, exist_ok=True)
    marker = target / "CACHEDIR.TAG"
    if not marker.exists():
        marker.write_text(CACHE_TAG)
    else:
        marker.touch()


def lock_target(target: Path, blocking: bool):
    path = target / LOCK_NAME
    try:
        lock = path.open("a+")
    except PermissionError:
        lock = path.open("r")
    operation = fcntl.LOCK_EX | (0 if blocking else fcntl.LOCK_NB)
    try:
        fcntl.flock(lock.fileno(), operation)
    except BlockingIOError:
        lock.close()
        return None
    return lock


def reset_target(
    target: Path,
    workspace: Path,
    execute: bool,
    now: float | None = None,
) -> Path:
    target = validate_run_target(target, workspace)
    if not target.is_dir():
        raise ValueError(f"target does not exist: {target}")
    if (target / PRESERVE_NAME).exists():
        raise ValueError(f"target is marked for preservation: {target}")
    if any((target / name).exists() for name in SOURCE_MARKERS):
        raise ValueError(f"target contains source or evidence markers: {target}")

    binding = read_binding(target)
    expected = current_binding(workspace)
    if binding is not None and binding.workspace != expected.workspace:
        raise ValueError(
            "target belongs to another worktree: "
            f"recorded={binding.workspace}, current={expected.workspace}, target={target}"
        )
    tag = target / "CACHEDIR.TAG"
    if binding is None and (not tag.is_file() or tag.read_text() != CACHE_TAG):
        raise ValueError(f"target was not created by cargo-space-guard: {target}")

    target_lock = lock_target(target, blocking=False)
    if target_lock is None:
        raise ValueError(f"target is already in use: {target}")
    try:
        instant = time.time() if now is None else now
        timestamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime(instant))
        quarantine = target.with_name(f"{target.name}.quarantine-{timestamp}-{os.getpid()}")
        if quarantine.exists():
            raise ValueError(f"quarantine path already exists: {quarantine}")
        if not execute:
            return quarantine

        staging = target.with_name(f".{target.name}.reset-{os.getpid()}")
        if staging.exists():
            raise ValueError(f"reset staging path already exists: {staging}")
        staging.mkdir()
        staging_lock = lock_target(staging, blocking=False)
        if staging_lock is None:
            raise ValueError(f"reset staging target is already in use: {staging}")
        try:
            prepare_target(staging, workspace)
            os.replace(target, quarantine)
            try:
                os.replace(staging, target)
            except BaseException:
                os.replace(quarantine, target)
                raise
        finally:
            staging_lock.close()
            if staging.exists():
                shutil.rmtree(staging)
        return quarantine
    finally:
        target_lock.close()


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
    target.mkdir(parents=True, exist_ok=True)
    target_lock = lock_target(target, blocking=False)
    if target_lock is None:
        print(f"cargo-space-guard: target is already in use: {target}", file=sys.stderr)
        return CAPACITY_EXIT

    process = None
    try:
        prepare_target(target, workspace)
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

    reset = commands.add_parser(
        "reset", help="quarantine a corrupted target and initialize a clean replacement"
    )
    reset.add_argument("--target-dir", type=Path, default=os.environ.get("CARGO_TARGET_DIR"))
    reset.add_argument("--execute", action="store_true")
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

        if options.operation == "reset":
            if options.target_dir is None:
                parser().error("reset requires --target-dir (or CARGO_TARGET_DIR)")
            target = validate_run_target(options.target_dir, Path.cwd())
            quarantine = reset_target(target, Path.cwd(), options.execute)
            action = "quarantined" if options.execute else "would quarantine"
            suffix = " and initialized a clean target" if options.execute else ""
            print(f"cargo-space-guard: {action} {target} as {quarantine}{suffix}")
            return 0

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
