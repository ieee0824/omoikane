#!/usr/bin/env python3
"""Install CI fonts with bounded apt execution and persistent setup evidence."""

import argparse
import json
import math
from pathlib import Path
import shlex
import subprocess
import time


FONT_PACKAGES = (
    "fonts-dejavu", "fonts-liberation", "fonts-noto-core", "fonts-noto-cjk"
)


def bounded_seconds(value):
    """Allow shorter diagnostic deadlines without exceeding the CI budget."""
    seconds = float(value)
    if not math.isfinite(seconds) or not 0 < seconds <= 120:
        raise argparse.ArgumentTypeError("deadline must be finite and in (0, 120]")
    return seconds


def apt_command(phase, seconds):
    """Keep timeout privileged so it can terminate apt and its child processes."""
    command = [
        "sudo", "-n", "env", "DEBIAN_FRONTEND=noninteractive",
        "timeout", "--signal=TERM", "--kill-after=5s", f"{seconds:g}s",
        "apt-get", "-o", "Acquire::Retries=1",
        "-o", "Acquire::http::Timeout=20", "-o", "Acquire::https::Timeout=20",
        "-o", "DPkg::Lock::Timeout=30", "-o", "Dpkg::Use-Pty=0",
    ]
    if phase == "update":
        # apt otherwise may exit 0 after using stale lists on a mirror failure.
        command.extend(["-o", "APT::Update::Error-Mode=any", "update"])
    else:
        command.extend(["install", "-y", *FONT_PACKAGES])
    return command


def save_result(directory, result):
    """Replace the summary atomically; leave logs intact on later failures."""
    temporary = directory / "result.json.tmp"
    temporary.write_text(json.dumps(result, indent=2) + "\n")
    temporary.replace(directory / "result.json")


def run_phase(phase, directory, seconds, result):
    """Try one apt phase at most twice and retain each attempt separately."""
    command = apt_command(phase, seconds)
    for attempt in range(1, 3):
        log_path = directory / f"{phase}-{attempt}.log"
        record = {"phase": phase, "attempt": attempt, "log": log_path.name,
                  "exit_code": None, "outcome": "running"}
        result["attempts"].append(record)
        save_result(directory, result)
        print(f"Starting font preparation {phase} attempt {attempt} "
              f"(deadline {seconds:g}s; log {log_path})", flush=True)
        started = time.monotonic()
        with log_path.open("w") as log:
            log.write(shlex.join(command) + "\n")
            log.flush()
            try:
                code = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT).returncode
            except OSError as error:
                log.write(f"Could not start font preparation: {error}\n")
                code = 127
        # subprocess exposes signal deaths as negative numbers; retain that
        # evidence while also reporting the conventional shell exit status.
        record["returncode"] = code
        if code < 0:
            code = 128 - code
        record.update(exit_code=code, elapsed_seconds=round(time.monotonic() - started, 3),
                      outcome="timeout" if code == 124 else "killed" if code == 137 else "exited")
        save_result(directory, result)
        print(f"Font preparation {phase} attempt {attempt}: exit={code}", flush=True)
        print(log_path.read_text(errors="replace"), flush=True)
        if code == 0:
            return True
        if code in (125, 126, 127):
            break
        if attempt < 2:
            time.sleep(1)
    return False


def install_fonts(directory, seconds):
    """Report preparation independently from Rust tests, which have not run yet."""
    directory.mkdir(parents=True, exist_ok=True)
    result = {"stage": "font_preparation", "status": "in_progress", "attempts": []}
    save_result(directory, result)
    for phase in ("update", "install"):
        if not run_phase(phase, directory, seconds, result):
            result.update(status="infrastructure_failure", failed_phase=phase)
            save_result(directory, result)
            print(f"::error title=CI infrastructure failure::Font preparation failed in {phase}; "
                  f"Rust tests were not started. See {directory}/result.json", flush=True)
            return 1
    result["status"] = "success"
    save_result(directory, result)
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--timeout-seconds", type=bounded_seconds, default=120)
    arguments = parser.parse_args()
    return install_fonts(arguments.output, arguments.timeout_seconds)


if __name__ == "__main__":
    raise SystemExit(main())
