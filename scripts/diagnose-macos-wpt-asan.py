#!/usr/bin/env python3
"""Independent, non-gating macOS WPT ASan evidence; never writes Gate 5 results."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import time

TOOLCHAIN = "1.98.1"
TARGET = "aarch64-apple-darwin"
FEATURES = "baseline-jit,jit-differential"
TEST = "selected_wpt_testharness_cases_match_expectations"
FLAGS = "-Zsanitizer=address -Cforce-frame-pointers=yes -Cdebuginfo=2"


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def record_command(name, command, env, folder, records):
    """Persist both successful and failed invocations without masking the exit."""
    log = folder / f"{name}.log"
    started = time.monotonic()
    record = {"name": name, "command": command, "log": str(log)}
    records.append(record)
    (folder / "steps.json").write_text(json.dumps(records, indent=2) + "\n")
    try:
        with log.open("w") as output:
            process = subprocess.run(command, env=env, stdout=output,
                                     stderr=subprocess.STDOUT, check=False)
        record["exit"] = process.returncode
    except OSError as error:
        record["exit"] = None
        record["error"] = str(error)
        log.write_text(str(error) + "\n")
    record["seconds"] = time.monotonic() - started
    (folder / "steps.json").write_text(json.dumps(records, indent=2) + "\n")
    return record.get("exit") == 0


def diagnostic_environment(folder, target_dir):
    env = os.environ.copy()
    # Cargo's encoded variant overrides RUSTFLAGS; do not accidentally build
    # uninstrumented code when the caller supplied that variable.
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    env.update({
        "CI": "1", "RUSTC_BOOTSTRAP": "1", "RUSTFLAGS": FLAGS,
        "CARGO_INCREMENTAL": "0", "CARGO_TARGET_DIR": str(target_dir),
        "WPT_ROOT": str(Path(".cache/wpt").resolve()), "WPT_REQUIRED": "1",
        "WPT_MANIFEST": str(Path("tests/wpt/manifest.json").resolve()),
        "WPT_REPORT": str(folder / "wpt.json"),
        "WPT_JUNIT": str(folder / "wpt.xml"),
        "ASAN_OPTIONS": f"detect_leaks=0:halt_on_error=1:log_path={folder / 'asan'}",
    })
    # Keep the fixture's normal transport selection and test scheduling.
    for name in ("WPT_UPSTREAM", "RUST_TEST_THREADS", "OMOIKANE_JIT_GATE_REPORT_DIR",
                 "OMOIKANE_WEB_API_REPORT", "ACID3_REPORT"):
        env.pop(name, None)
    return env


def build_command():
    return ["cargo", f"+{TOOLCHAIN}", "test", "--locked", "--target", TARGET,
            "--features", FEATURES, "--test", "wpt_smoke", "--no-run",
            "--message-format=json"]


def test_binary(build_log):
    candidates = set()
    for line in build_log.read_text().splitlines():
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if (message.get("reason") == "compiler-artifact"
                and message.get("target", {}).get("name") == "wpt_smoke"
                and message.get("profile", {}).get("test")
                and message.get("executable")):
            candidates.add(message["executable"])
    if len(candidates) != 1:
        raise ValueError(f"expected exactly one WPT test executable, found {len(candidates)}")
    return Path(candidates.pop()).resolve()


def provenance(env):
    return {
        "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
        "tracked_changes": subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=no"], text=True),
        "lock_sha256": sha256("Cargo.lock"),
        "boa_lock_sha256": sha256("engine/boa/Cargo.lock"),
        "manifest_sha256": sha256(env["WPT_MANIFEST"]),
        "wpt_revision": Path("tests/wpt/revision.txt").read_text().strip(),
        "toolchain": TOOLCHAIN, "target": TARGET, "features": FEATURES,
        "environment": {key: env.get(key) for key in (
            "CI", "RUSTFLAGS", "RUSTC_BOOTSTRAP", "CARGO_TARGET_DIR",
            "WPT_ROOT", "WPT_REQUIRED", "WPT_MANIFEST", "WPT_REPORT",
            "WPT_JUNIT", "ASAN_OPTIONS", "RUST_TEST_THREADS")},
        "standard_library_instrumented": False,
        "jit_generated_machine_code_instrumented": False,
    }


def execute(folder, target_dir, report):
    if platform.system() != "Darwin" or platform.machine() not in ("arm64", "aarch64"):
        raise RuntimeError("this diagnostic requires native Apple Silicon macOS")
    env = diagnostic_environment(folder, target_dir)
    report["provenance"] = provenance(env)
    report["status"] = "running"
    (folder / "diagnostic.json").write_text(json.dumps(report, indent=2) + "\n")
    commands = [("rustc", ["rustc", f"+{TOOLCHAIN}", "--version", "--verbose"]),
                ("fetch-wpt", ["bash", "scripts/fetch-wpt.sh"]),
                ("build", build_command())]
    for name, command in commands:
        if not record_command(name, command, env, folder, report["steps"]):
            report["failed_stage"] = name
            return False
    binary = test_binary(folder / "build.log")
    report["binary"] = {"path": str(binary), "sha256": sha256(binary)}
    success = record_command("run", [str(binary), TEST, "--exact", "--nocapture"],
                             env, folder, report["steps"])
    if not success:
        report["failed_stage"] = "run"
    return success


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("folder", type=Path)
    parser.add_argument("--target-dir", type=Path, required=True)
    args = parser.parse_args()
    folder = args.folder.resolve()
    folder.mkdir(parents=True, exist_ok=False)
    report = {"diagnostic_only": True, "gate_result": False, "steps": []}
    try:
        success = execute(folder, args.target_dir.resolve(), report)
        report["status"] = "passed" if success else "failed"
    except Exception as error:
        report["status"] = "failed"
        report["error"] = f"{type(error).__name__}: {error}"
        success = False
    finally:
        (folder / "diagnostic.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if success else 1


if __name__ == "__main__":
    raise SystemExit(main())
