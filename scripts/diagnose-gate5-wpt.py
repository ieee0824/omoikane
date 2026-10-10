#!/usr/bin/env python3
"""Capture a Mac WPT crash while preserving the failed Gate 5 result."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time


def main():
    """Replay the recorded WPT executable under LLDB into separate artifacts."""
    parser = argparse.ArgumentParser()
    parser.add_argument("suite", type=Path)
    args = parser.parse_args()
    suite = args.suite.resolve()
    output = suite / "crash-diagnostic"
    output.mkdir(exist_ok=False)
    result = json.loads((suite / "result.json").read_text())
    raw = (suite / "full-suite.log").read_text(errors="replace")
    report = {"original_result": result, "original_failure_preserved": True,
              "normal_full_suite_sha256": hashlib.sha256((suite / "full-suite.log").read_bytes()).hexdigest(),
              "debugger_execution": None, "completed": False}
    proof = output / "inputs.json"
    save = lambda: proof.write_text(json.dumps(report, indent=2) + "\n")
    save()
    assert result["status"] == "failed"
    match = re.search(r"process didn't exit successfully: `([^`]+)` \(signal: 11, SIGSEGV", raw)
    if sys.platform != "darwin" or not match:
        report.update(completed=True, not_run_reason="not a recorded Mac SIGSEGV")
        save()
        return 0
    command = match.group(1).split()
    assert command[1:] == ["--include-ignored", "--nocapture"], command
    binary = Path(command[0]).resolve()
    target = Path(os.environ.get("CARGO_TARGET_DIR", "target")).resolve()
    assert binary.is_relative_to(target) and binary.name.startswith("wpt_smoke-")
    assert binary.is_file() and not binary.is_symlink()
    sha = hashlib.sha256(binary.read_bytes()).hexdigest()
    report.update(binary=str(binary), binary_sha256=sha,
                  git_head=subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
                  lldb_version=subprocess.check_output(["xcrun", "lldb", "--version"], text=True))
    # Preserve the exact normal executable for symbolization after the runner disappears.
    shutil.copy2(binary, output / binary.name)
    helper = Path(__file__).with_name("gate5_lldb_capture.py").resolve()
    normal_environment = json.loads((suite / "suite-environment.json").read_text())
    environment = dict(os.environ)
    for key, value in normal_environment.items():
        if value is None:
            environment.pop(key, None)
        else:
            environment[key] = value
    assert environment["WPT_REQUIRED"] == "1" and Path(environment["WPT_ROOT"]).is_dir()
    # Same checkout/manifest/required policy; keep debugger reports separate from normal artifacts.
    environment.update(WPT_REPORT=str(output / "debugger-wpt.json"),
                       WPT_JUNIT=str(output / "debugger-wpt.xml"),
                       OMOIKANE_JIT_GATE_REPORT_DIR=str(output),
                       OMOIKANE_WEB_API_REPORT=str(output / "debugger-web-api.json"))
    normal_files = {path.name: hashlib.sha256(path.read_bytes()).hexdigest()
                    for path in suite.iterdir() if path.is_file()}
    report["normal_files_before"] = normal_files
    report["normal_environment"] = normal_environment
    report["debugger_report_paths"] = {key: environment[key] for key in
                                      ("WPT_REPORT", "WPT_JUNIT", "OMOIKANE_JIT_GATE_REPORT_DIR")}
    environment.update(OMOIKANE_LLDB_OUTPUT=str(output / "lldb"),
                       OMOIKANE_LLDB_BINARY_SHA256=sha)
    lldb_command = ["xcrun", "lldb", "--batch", "--no-lldbinit", "--file", str(binary),
                    "-o", "command script import " + str(helper),
                    "-o", "script gate5_lldb_capture.capture(lldb.debugger)"]
    started = time.monotonic()
    with (output / "lldb-driver.log").open("w") as log:
        exit_code = subprocess.run(lldb_command, env=environment, stdout=log, stderr=log).returncode
    report["debugger_execution"] = {"command": lldb_command, "exit": exit_code,
                                    "seconds": time.monotonic() - started}
    report["binary_unchanged"] = hashlib.sha256(binary.read_bytes()).hexdigest() == sha
    reports = output / "darwin-reports"
    reports.mkdir()
    report["darwin_reports"] = []
    for source in [Path.home() / "Library/Logs/DiagnosticReports", Path("/Library/Logs/DiagnosticReports")]:
        try:
            for path in source.glob("wpt_smoke*"):
                if path.is_file() and not path.is_symlink() and path.suffix in (".ips", ".crash"):
                    destination = reports / (str(len(report["darwin_reports"])) + "-" + path.name)
                    shutil.copy2(path, destination)
                    report["darwin_reports"].append({"original": str(path), "saved": str(destination), "mtime_unix": path.stat().st_mtime, "attribution_requires_pid_timestamp_match": True})
        except OSError as error:
            report.setdefault("darwin_report_read_errors", []).append(str(error))
    report["normal_files_after"] = {path.name: hashlib.sha256(path.read_bytes()).hexdigest()
                                    for path in suite.iterdir() if path.is_file()}
    report["normal_files_unchanged"] = report["normal_files_after"] == normal_files
    report.update(completed=True, debugger_result_is_not_gate_result=True)
    save()
    assert report["binary_unchanged"] and report["normal_files_unchanged"]
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
