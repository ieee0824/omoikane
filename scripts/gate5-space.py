#!/usr/bin/env python3
"""Record Gate 5 filesystem and cache usage without changing test commands."""

import argparse
from contextlib import contextmanager
import importlib.util
import json
import math
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time


def measured_paths(root, environment):
    """Resolve the relevant paths from an explicit runner environment."""
    def path(name, default):
        value = Path(environment.get(name) or default).expanduser()
        return value if value.is_absolute() else root / value

    home = path("HOME", str(root))
    return {
        "workspace": root,
        "target": path("CARGO_TARGET_DIR", "target"),
        "cargo_home": path("CARGO_HOME", str(home / ".cargo")),
        "wpt": path("WPT_ROOT", ".cache/wpt"),
        "runner_temp": path("RUNNER_TEMP", str(root / ".runner-temp")),
    }


def existing_parent(path):
    """Select an existing path on the filesystem that would hold this path."""
    existing = path
    while not existing.exists() and existing != existing.parent:
        existing = existing.parent
    return existing


def measure_path(path):
    """Read filesystem usage and bounded du output; retain measurement failures."""
    record = {"path": str(path), "exists": path.exists()}
    existing = existing_parent(path)
    try:
        usage = shutil.disk_usage(existing)
        record["filesystem"] = dict(zip(("total", "used", "free"), usage))
        record["filesystem_path"] = str(existing)
        record["resolved_filesystem_path"] = str(existing.resolve())
        record["device"] = existing.stat().st_dev
    except OSError as error:
        record["filesystem_error"] = str(error)
    if not record["exists"]:
        return record
    try:
        result = subprocess.run(["du", "-sk", str(path)], capture_output=True,
                                text=True, timeout=10)
        record["du"] = {"exit": result.returncode, "stdout": result.stdout,
                        "stderr": result.stderr}
    except (OSError, subprocess.TimeoutExpired) as error:
        record["du"] = {"error": str(error)}
    return record


def snapshot(output, stage, root, environment):
    """Persist one observation; this I/O helper never removes or modifies caches."""
    if not stage or any(c not in "abcdefghijklmnopqrstuvwxyz0123456789-_" for c in stage):
        raise ValueError("stage must contain only lowercase letters, digits, '-' and '_'")
    output.mkdir(parents=True, exist_ok=True)
    report = {
        "stage": stage, "time_unix": time.time(),
        "image_version": environment.get("ImageVersion"),
        "image_os": environment.get("ImageOS"),
        "paths": {name: measure_path(path)
                  for name, path in measured_paths(root, environment).items()},
    }
    destination = output / (stage + ".json")
    # Keep the previous complete observation if an interrupted write fails.
    temporary = output / (stage + ".json.tmp")
    temporary.write_text(json.dumps(report, indent=2) + "\n")
    temporary.replace(destination)
    return report


def stop_process_group(process):
    """Use the existing guard's TERM/KILL handling for the whole child group."""
    name = "gate5_cargo_space_guard"
    if name not in sys.modules:
        spec = importlib.util.spec_from_file_location(
            name, Path(__file__).with_name("cargo-space-guard.py"))
        module = importlib.util.module_from_spec(spec)
        sys.modules[name] = module
        spec.loader.exec_module(module)
    sys.modules[name].stop_process_group(process, timeout_seconds=1)


class MonitorCancelled(BaseException):
    """A cancellation signal observed while the monitor owns a child group."""


@contextmanager
def cancellation_signals(state):
    """Restore previous handlers after recording cancellation and child cleanup."""
    previous = {}

    def cancelled(signum, _frame):
        if state["signal"] is None:
            state["signal"] = signum
        # Further signals must not interrupt cleanup or authoritative JSON writes.
        for number in (signal.SIGTERM, signal.SIGINT):
            signal.signal(number, signal.SIG_IGN)
        if not state["spawning"] and not state["finishing"]:
            raise MonitorCancelled()

    try:
        for number in (signal.SIGTERM, signal.SIGINT):
            previous[number] = signal.signal(number, cancelled)
        yield
    finally:
        for number, handler in previous.items():
            signal.signal(number, handler)
        if "terminal_signal_mask" in state:
            signal.pthread_sigmask(signal.SIG_SETMASK, state.pop("terminal_signal_mask"))


def poll_command(command, output, root, environment, minimum_free, poll_seconds,
                 usage, state, result):
    """Measure filesystem headroom while owning one unchanged child command."""
    paths = measured_paths(root, environment)
    with (output / "filesystem-samples.jsonl").open("w") as log:
        while True:
            filesystems = {}
            for name, path in paths.items():
                existing = existing_parent(path)
                total, used, free = usage(existing)
                filesystems[name] = {"path": str(path), "total": total,
                                     "used": used, "free": free,
                                     "resolved_path": str(existing.resolve()),
                                     "device": existing.stat().st_dev}
            sample = {"time_unix": time.time(), "paths": filesystems}
            log.write(json.dumps(sample) + "\n")
            log.flush()
            low = [name for name, data in filesystems.items()
                   if data["free"] < minimum_free]
            if low:
                result["capacity_failure"] = {"paths": low, "sample": sample}
                return 75
            if state["process"] is None:
                state["spawning"] = True
                try:
                    state["process"] = subprocess.Popen(
                        command, cwd=root, env=environment, start_new_session=True)
                finally:
                    state["spawning"] = False
                if state["signal"] is not None:
                    raise MonitorCancelled()
            completed = state["process"].poll()
            if completed is not None:
                return completed
            time.sleep(poll_seconds)


def commit_terminal_signals(state, observed_signal):
    """End monitor cancellation acceptance while TERM/INT delivery is blocked.

    Pending signals that were deliverable on entry are handled before committing.
    After the pending check succeeds, later signals belong to the restored caller
    handlers. The surrounding cancellation_signals context restores those handlers
    before restoring this thread's original signal mask.
    """
    numbers = {signal.SIGTERM, signal.SIGINT}
    previous_mask = signal.pthread_sigmask(signal.SIG_BLOCK, numbers)
    pending = signal.sigpending() & (numbers - previous_mask)
    if pending or state["signal"] != observed_signal:
        signal.pthread_sigmask(signal.SIG_SETMASK, previous_mask)
        return False
    state["terminal_signal_mask"] = previous_mask
    return True


def save_terminal_result(output, result, code, state):
    """Save terminal evidence, cleaning on cancellation accepted during writes."""
    cleanup_error = None
    while True:
        number = state["signal"]
        process = state.get("process")
        if (number is not None and process is not None
                and "cleanup" not in result and "cleanup_error" not in result):
            try:
                stop_process_group(process)
                result["cleanup"] = "verified_no_live_members"
            except BaseException as error:
                cleanup_error = error
                result["cleanup_error"] = {
                    "type": type(error).__name__, "message": str(error)
                }
        if number is not None:
            code = 128 + number
            result["signal"] = number
        status = "cancelled" if number is not None else ("passed" if code == 0 else "failed")
        result.update(exit=code, status=status)
        temporary = output / "capacity-result.json.tmp"
        temporary.write_text(json.dumps(result, indent=2) + "\n")
        temporary.replace(output / "capacity-result.json")
        if commit_terminal_signals(state, number):
            if cleanup_error is not None:
                raise cleanup_error
            return code


def run_monitored(command, output, root, environment, minimum_free, poll_seconds,
                  usage=shutil.disk_usage):
    """Run unchanged commands, stopping on measured low disk space with exit 75.

    This protects log headroom, not the size of an arbitrary write between polls.
    It does not bind, alter, clean or change the Cargo target/cache directories.
    """
    if (minimum_free <= 0 or not math.isfinite(poll_seconds)
            or poll_seconds <= 0 or not command):
        raise ValueError("positive free-space/poll limits and a command are required")
    output.mkdir(parents=True, exist_ok=True)
    result = {"command": list(command), "minimum_free_bytes": minimum_free,
              "poll_seconds": poll_seconds, "status": "running", "exit": None}
    state = {"process": None, "signal": None, "spawning": False, "finishing": False}
    code = 75
    with cancellation_signals(state):
        try:
            code = poll_command(command, output, root, environment, minimum_free,
                                poll_seconds, usage, state, result)
        except MonitorCancelled:
            pass
        except KeyboardInterrupt:
            state["signal"] = signal.SIGINT
        except BaseException as error:
            code = 1
            result["monitor_error"] = {"type": type(error).__name__, "message": str(error)}
            raise
        finally:
            state["finishing"] = True
            try:
                process = state["process"]
                if process is not None and (
                    code != 0 or state["signal"] is not None or process.poll() is None
                ):
                    try:
                        stop_process_group(process)
                        result["cleanup"] = "verified_no_live_members"
                    except BaseException as error:
                        code = 1
                        result["cleanup_error"] = {
                            "type": type(error).__name__, "message": str(error)
                        }
                        raise
            finally:
                code = save_terminal_result(output, result, code, state)
    return code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="operation", required=True)
    record = commands.add_parser("snapshot")
    record.add_argument("stage")
    record.add_argument("output", type=Path)
    run = commands.add_parser("run")
    run.add_argument("output", type=Path)
    run.add_argument("--minimum-free-gib", type=float, default=2)
    run.add_argument("--poll-seconds", type=float, default=1)
    run.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.operation == "snapshot":
        snapshot(args.output, args.stage, Path.cwd(), dict(os.environ))
        return 0
    command = args.command
    if command and command[0] == "--":
        command = command[1:]
    code = run_monitored(command, args.output, Path.cwd(), dict(os.environ),
                         int(args.minimum_free_gib * 1024**3), args.poll_seconds)
    return code if code >= 0 else 128 - code


if __name__ == "__main__":
    raise SystemExit(main())
