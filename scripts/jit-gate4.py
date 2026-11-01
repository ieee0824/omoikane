#!/usr/bin/env python3
"""Run fixed Gate 4 partitions and require complete evidence before issuing go."""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time


# Also resolve the shared validator when invoked directly as a script.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from scripts.acid3_expectations import load_expectation, report_matches_expectation


UNIT_SHARDS = 4
INTEGRATION_SHARDS = 2
INTEGRATION_NAMES = [f"integration-{i}" for i in range(INTEGRATION_SHARDS)]
SHARDS = [f"unit-{i}" for i in range(UNIT_SHARDS)] + INTEGRATION_NAMES + [
    "build", "acid3", "compatibility", "stress",
]
FEATURES = "jit-stress,jit-differential"
# print_page_wpt needs the pinned checkout supplied by the separate WPT CI job.
DEDICATED_TARGETS = {
    "acid3_harness", "jit_deopt", "web_api_surface", "wpt_smoke", "print_page_wpt",
}
TEST_ARGS = ["--include-ignored", "--nocapture", "--test-threads=1"]
STRESS_MATRIX_TEST = "stress::reproducible_stress_matrix"
ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;]*m")
STEPS = {**{f"unit-{i}": ["list", "unit"] for i in range(UNIT_SHARDS)},
         **{name: ["integration"] for name in INTEGRATION_NAMES},
         "build": ["examples-and-bins", "doc", "build"],
         "acid3": ["acid3", "embedded-acid3"],
         "compatibility": ["fetch-wpt", "wpt", "web-api", "embedded-web-api"],
         "stress": ["stress"]}


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def read_json(path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


def output(*command):
    return subprocess.check_output(command, text=True).strip()


def identity():
    return {
        "revision": output("git", "rev-parse", "HEAD"),
        "source_dirty": bool(output("git", "status", "--porcelain", "--untracked-files=no")),
        "rustc": output("rustc", "--version", "--verbose"),
        "target": os.environ.get("CARGO_BUILD_TARGET", "host"),
        "lock_sha256": hashlib.sha256(Path("Cargo.lock").read_bytes()).hexdigest(),
    }


def prepare(root):
    root.mkdir(parents=True, exist_ok=False)
    if not Path("Cargo.lock").exists():
        with (root / "lockfile.log").open("w") as log:
            subprocess.run(["cargo", "generate-lockfile"], stdout=log, stderr=log, check=True)
    shutil.copyfile("Cargo.lock", root / "Cargo.lock")
    current = identity()
    write_json(root / "identity.json", current)
    targets = integration_targets_from_metadata()
    if not valid_integration_targets(targets) or "jit_native_gate" not in targets:
        raise ValueError("integration inventory must include the native-policy target exactly once")
    if any(not integration_partition(targets, index) for index in range(INTEGRATION_SHARDS)):
        raise ValueError("empty integration partition")
    write_json(root / "integration-targets.json", targets)
    (root / "revision.txt").write_text(current["revision"] + "\n")
    (root / "rustc.txt").write_text(current["rustc"] + "\n")
    (root / "source-status.txt").write_text(
        output("git", "status", "--porcelain", "--untracked-files=no") + "\n")


def unit_partition(names, index):
    return [name for name in names
            if int.from_bytes(hashlib.sha256(name.encode()).digest()[:8], "big") % UNIT_SHARDS == index]


def integration_partition(targets, index):
    """Assign every prepared target to one stable, source-independent batch."""
    return [name for name in targets
            if int.from_bytes(hashlib.sha256(name.encode()).digest()[:8], "big")
            % INTEGRATION_SHARDS == index]


def valid_integration_targets(targets):
    return (isinstance(targets, list) and bool(targets)
            and all(isinstance(name, str) for name in targets)
            and targets == sorted(set(targets)))


def passed_tests(log):
    return sum(map(int, re.findall(r"^test result: ok\. (\d+) passed", log, re.M)))


def acid3_test26_timing(report):
    """Retain both the driver-step wall time and the fixture's own test time."""
    timing = {}
    for mode in ("faithful", "direct"):
        row = report.get(mode, {}) if isinstance(report, dict) else {}
        if not isinstance(row, dict):
            row = {}
        match = re.search(r"(?m)^Test 26 passed, but took (\d+)ms\b", row.get("log") or "")
        timing[mode] = {
            "driver_ms": row.get("test26_step_wall_ms"),
            "fixture_ms": int(match.group(1)) if match else None,
        }
    return timing


def has_acid3_test26_timing(shard):
    if not isinstance(shard, dict):
        return False
    steps = shard.get("steps", [])
    if [step.get("name") for step in steps] != STEPS["acid3"]:
        return False
    return all(
        isinstance(step.get("test26_ms"), dict)
        and all(
            isinstance(step["test26_ms"].get(mode), dict)
            and isinstance(step["test26_ms"][mode].get("driver_ms"), (int, float))
            and not isinstance(step["test26_ms"][mode]["driver_ms"], bool)
            and math.isfinite(step["test26_ms"][mode]["driver_ms"])
            and step["test26_ms"][mode]["driver_ms"] >= 0
            for mode in ("faithful", "direct")
        )
        for step in steps
    )


def integration_targets(package):
    # Cargo's normal feature closure determines which required-feature targets exist.
    enabled = {"default", *FEATURES.split(",")}
    pending = list(enabled)
    while pending:
        for feature in package["features"].get(pending.pop(), []):
            if feature in package["features"] and feature not in enabled:
                enabled.add(feature)
                pending.append(feature)
    return sorted(target["name"] for target in package["targets"]
                  if target["kind"] == ["test"]
                  and target["name"] not in DEDICATED_TARGETS
                  and set(target.get("required-features", [])) <= enabled)


def integration_targets_from_metadata():
    metadata = json.loads(output("cargo", "metadata", "--locked", "--no-deps",
                                 "--format-version", "1"))
    package = next(p for p in metadata["packages"] if p["name"] == "omoikane")
    return integration_targets(package)


def test_command(selectors, filters=()):
    return ["cargo", "test", "--locked", "--features", FEATURES,
            "--no-fail-fast", *selectors, "--", *TEST_ARGS, *filters]


def integration_command(targets):
    selectors = [arg for target in targets for arg in ("--test", target)]
    return test_command(selectors, ["--skip", STRESS_MATRIX_TEST])


def stress_command():
    return test_command(["--test", "jit_deopt"],
                        ["--skip", "acid3::", "--skip", "web_api_surface::"])


def build_commands():
    """Retain the original examples, documentation and build validation."""
    return [("examples-and-bins", test_command(["--examples", "--bins"])),
            ("doc", test_command(["--doc"])),
            ("build", ["cargo", "build", "--locked", "--features", "jit-stress"])]


def complete_build_coverage(shard):
    steps = shard.get("steps") if isinstance(shard, dict) else None
    expected = build_commands()
    return (isinstance(steps, list) and len(steps) == len(expected)
            and not shard.get("error")
            and all(isinstance(step, dict) and step.get("name") == name
                    and step.get("command") == command and step.get("exit") == 0
                    for step, (name, command) in zip(steps, expected)))


def integration_log_covers_targets(log, targets):
    """Require every selected binary to pass with only the duplicate matrix filtered."""
    if not valid_integration_targets(targets):
        return False
    seen = {}
    active = None
    for line in ANSI_ESCAPE.sub("", log).splitlines():
        match = re.search(r"\bRunning tests/([\w-]+)\.rs\b", line)
        if match:
            active = match.group(1)
            if active in seen:
                return False
            seen[active] = {"filtered": None, "native_policy": False}
        if active == "jit_native_gate" and line.startswith(
                "test native_policy_and_seeded_workloads_produce_execution_evidence ..."):
            seen[active]["native_policy"] = True
        if line.startswith(f"test {STRESS_MATRIX_TEST} ..."):
            return False
        result = re.match(
            r"test result: ok\. \d+ passed; 0 failed; \d+ ignored; "
            r"\d+ measured; (\d+) filtered out;", line)
        if result and active:
            seen[active]["filtered"] = int(result.group(1))
    return (sorted(seen) == targets
            and ("jit_native_gate" not in targets or seen["jit_native_gate"]["native_policy"])
            and all(row["filtered"] == (1 if name == "jit_native_gate" else 0)
                    for name, row in seen.items()))


def complete_integration_shard_coverage(root, name, shard):
    """Verify this batch's exact prepared selection, command and execution log."""
    expected = read_json(root / "inputs/integration-targets.json")
    if (not valid_integration_targets(expected) or "jit_native_gate" not in expected
            or name not in INTEGRATION_NAMES):
        return False
    targets = integration_partition(expected, INTEGRATION_NAMES.index(name))
    selected = read_json(root / name / "targets.json")
    steps = shard.get("steps") if isinstance(shard, dict) else None
    if (not targets or selected != targets or not isinstance(steps, list) or len(steps) != 1
            or not isinstance(steps[0], dict)
            or steps[0].get("name") != "integration" or steps[0].get("exit") != 0
            or steps[0].get("command") != integration_command(targets)):
        return False
    try:
        log = (root / name / "integration.log").read_text(errors="replace")
    except OSError:
        return False
    return integration_log_covers_targets(log, targets)


def complete_integration_coverage(root, shards):
    """Require the exact full inventory once across all integration batches."""
    expected = read_json(root / "inputs/integration-targets.json")
    if not valid_integration_targets(expected) or "jit_native_gate" not in expected:
        return False
    covered = []
    for name in INTEGRATION_NAMES:
        if not complete_integration_shard_coverage(root, name, shards.get(name)):
            return False
        covered.extend(read_json(root / name / "targets.json"))
    return sorted(covered) == expected and len(set(covered)) == len(covered)


def complete_stress_matrix_execution(root, shard):
    steps = shard.get("steps") if isinstance(shard, dict) else None
    if (not isinstance(steps, list) or not steps or not isinstance(steps[0], dict)
            or steps[0].get("command") != stress_command()):
        return False
    try:
        log = (root / "stress/stress.log").read_text(errors="replace")
    except OSError:
        return False
    matches = re.findall(rf"(?m)^test {re.escape(STRESS_MATRIX_TEST)} \.\.\.",
                         ANSI_ESCAPE.sub("", log))
    paths = re.findall(r"JIT stress: \d+ seeds passed in [\d.]+s; (.+)", log)
    return (len(matches) == 1 and len(paths) == 1
            and shard.get("stress_artifacts") == paths[0]
            and isinstance(read_json(root / "stress/stress.json"), dict))


def run_integration_batch(shard, root, result, run):
    """Store the prepared selection, execute it, and verify its retained log."""
    targets = integration_targets_from_metadata()
    if (not valid_integration_targets(targets)
            or targets != read_json(root / "inputs/integration-targets.json")):
        raise ValueError("integration targets differ from prepared target list")
    selected = integration_partition(targets, INTEGRATION_NAMES.index(shard))
    if not selected:
        raise ValueError("empty integration partition")
    write_json(root / shard / "targets.json", selected)
    status, _ = run("integration", integration_command(selected))
    if status == 0 and not complete_integration_shard_coverage(root, shard, result):
        raise ValueError("integration target or stress-matrix selection is incomplete")


def run_shard(shard, root):
    folder = root / shard
    folder.mkdir(parents=True, exist_ok=False)
    result = {"shard": shard, "identity": None, "steps": [], "passed_tests": 0, "error": None}
    env = dict(os.environ)
    env.update({
        "CI": "1",
        "OMOIKANE_JIT_GATE_REPORT_DIR": str(folder),
        "OMOIKANE_BROWSER_REPORT_DIR": str(folder / "browser"),
        "OMOIKANE_WEB_API_REPORT": str(folder / "web-api.json"),
        "OMOIKANE_JIT_STRESS_SEEDS": env.get("OMOIKANE_JIT_STRESS_SEEDS", "64"),
        "OMOIKANE_JIT_STRESS_MIN_SECONDS": env.get("OMOIKANE_JIT_STRESS_MIN_SECONDS", "600"),
        "WPT_ROOT": env.get("WPT_ROOT", ".cache/wpt"),
        "WPT_REQUIRED": "1",
        "WPT_REPORT": str(folder / "wpt.json"),
        "WPT_JUNIT": str(folder / "wpt.xml"),
    })

    def run(label, command):
        start = time.monotonic()
        print(f"[{shard}] {label}: start", flush=True)
        with (folder / f"{label}.log").open("w") as log:
            status = subprocess.run(command, env=env, stdout=log, stderr=log).returncode
        contents = (folder / f"{label}.log").read_text(errors="replace")
        result["steps"].append({
            "name": label, "command": command, "exit": status,
            "seconds": round(time.monotonic() - start, 3),
        })
        result["passed_tests"] += passed_tests(contents)
        write_json(folder / "shard.json", result)
        print(f"[{shard}] {label}: exit={status}", flush=True)
        return status, contents

    def test(label, selectors, filters=()):
        return run(label, test_command(selectors, filters))

    try:
        if not Path("Cargo.lock").exists():
            shutil.copyfile(root / "inputs/Cargo.lock", "Cargo.lock")
        result["identity"] = identity()
        if result["identity"] != read_json(root / "inputs/identity.json"):
            raise ValueError("revision, compiler, target, working tree or Cargo.lock differs from inputs")
        if shard.startswith("unit-"):
            status, listing = run("list", ["cargo", "test", "--locked", "--features", FEATURES,
                                           "--lib", "--", "--list", "--format", "terse"])
            if status:
                raise ValueError("could not enumerate unit tests")
            names = sorted(re.findall(r"^(.+): test$", listing, re.M))
            selected = unit_partition(names, int(shard.removeprefix("unit-")))
            write_json(folder / "all-tests.json", names)
            write_json(folder / "selected-tests.json", selected)
            if not selected or len(set(names)) != len(names):
                raise ValueError("empty partition or duplicate test names")
            status, log = test("unit", ["--lib"], ["--exact", *selected])
            if status == 0 and passed_tests(log) != len(selected):
                raise ValueError("unit test count does not match the selected partition")
        elif shard in INTEGRATION_NAMES:
            run_integration_batch(shard, root, result, run)
        elif shard == "build":
            for label, command in build_commands():
                run(label, command)
        elif shard == "acid3":
            test("acid3", ["--test", "acid3_harness"])
            result["steps"][-1]["test26_ms"] = acid3_test26_timing(
                read_json(folder / "acid3.json"))
            write_json(folder / "shard.json", result)
            # jit_deopt also embeds the harness; keep its existing coverage.
            test("embedded-acid3", ["--test", "jit_deopt"], ["acid3::"])
            result["steps"][-1]["test26_ms"] = acid3_test26_timing(
                read_json(folder / "acid3.json"))
            write_json(folder / "shard.json", result)
        elif shard == "compatibility":
            run("fetch-wpt", ["scripts/fetch-wpt.sh"])
            test("wpt", ["--test", "wpt_smoke"])
            test("web-api", ["--test", "web_api_surface"])
            test("embedded-web-api", ["--test", "jit_deopt"], ["web_api_surface::"])
        elif shard == "stress":
            status, log = run("stress", stress_command())
            paths = re.findall(r"JIT stress: \d+ seeds passed in [\d.]+s; (.+)", log)
            if paths:
                result["stress_artifacts"] = paths[-1]
                shutil.copyfile(Path(paths[-1]) / "summary.json", folder / "stress.json")
            if status == 0 and not complete_stress_matrix_execution(root, result):
                raise ValueError("dedicated stress matrix or its summary is missing")
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        result["error"] = str(error)
        print(f"[{shard}] {error}", file=sys.stderr, flush=True)
    write_json(folder / "shard.json", result)
    return bool(result["steps"]) and not result["error"] and all(s["exit"] == 0 for s in result["steps"])


def complete_unit_coverage(root):
    all_names = read_json(root / "unit-0/all-tests.json")
    if not isinstance(all_names, list) or not all_names or len(set(all_names)) != len(all_names):
        return False
    seen = []
    for i in range(UNIT_SHARDS):
        folder = root / f"unit-{i}"
        selected = read_json(folder / "selected-tests.json")
        if (read_json(folder / "all-tests.json") != all_names
                or selected != unit_partition(all_names, i) or not selected):
            return False
        seen.extend(selected)
    return sorted(seen) == sorted(all_names) and len(set(seen)) == len(seen)


def aggregate(root, jobs_succeeded=True):
    expected = read_json(root / "inputs/identity.json")
    shards = {name: read_json(root / name / "shard.json") for name in SHARDS}
    complete = all(isinstance(row, dict) and row.get("shard") == name
                   and row.get("identity") == expected
                   and [step.get("name") for step in row.get("steps", [])] == STEPS[name]
                   and not row.get("error")
                   and all(step.get("exit") == 0 for step in row["steps"])
                   for name, row in shards.items())
    acid3 = read_json(root / "acid3/acid3.json")
    wpt = read_json(root / "compatibility/wpt.json")
    web_api = read_json(root / "compatibility/web-api.json")
    stress = read_json(root / "stress/stress.json")
    integration_coverage = complete_integration_coverage(root, shards)
    stress_matrix_execution = complete_stress_matrix_execution(root, shards.get("stress"))
    checks = {
        "jobs_succeeded": jobs_succeeded,
        "same_revision": bool(expected) and expected["revision"] == output("git", "rev-parse", "HEAD"),
        "source_clean": bool(expected) and not expected["source_dirty"],
        "full_suite": complete and complete_unit_coverage(root) and integration_coverage,
        "stress_matrix_once": integration_coverage and stress_matrix_execution,
        "build": complete_build_coverage(shards.get("build")),
        "acid3_expected_result": report_matches_expectation(acid3, load_expectation()),
        "acid3_test26_reported": has_acid3_test26_timing(shards.get("acid3")),
        "wpt_regression_zero": isinstance(wpt, dict) and wpt.get("summary", {}).get("regression") == 0,
        "web_api_regression_zero": isinstance(web_api, dict) and web_api.get("regressions") == [],
        "stress_64_seeds": isinstance(stress, dict) and stress.get("gc_profile") is True
            and len(stress.get("seeds", [])) >= 64 and all(s.get("success") is True for s in stress["seeds"]),
        "stress_10_minutes": isinstance(stress, dict) and stress.get("elapsed_seconds", 0) >= 600,
    }
    report = {
        "decision": "go" if all(checks.values()) else "no-go",
        "revision": expected["revision"] if expected else None,
        "source_dirty": expected["source_dirty"] if expected else None,
        "checks": checks, "shards": shards,
        "passed_tests": sum(row.get("passed_tests", 0) for row in shards.values() if isinstance(row, dict)),
        "acid3": acid3, "wpt_summary": wpt.get("summary") if wpt else None,
        "web_api": web_api, "stress": stress,
        "stress_artifacts": (shards.get("stress") or {}).get("stress_artifacts"),
    }
    root.mkdir(parents=True, exist_ok=True)
    write_json(root / "gate.json", report)
    print(json.dumps({"decision": report["decision"], "checks": checks,
                      "artifact": str(root / "gate.json")}, indent=2))
    return report["decision"] == "go"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("matrix")
    commands.add_parser("prepare").add_argument("root", type=Path)
    run = commands.add_parser("run")
    run.add_argument("shard", choices=SHARDS)
    run.add_argument("root", type=Path)
    collect = commands.add_parser("aggregate")
    collect.add_argument("root", type=Path)
    collect.add_argument("--jobs-result", choices=["success", "failure", "cancelled", "skipped"], default="success")
    commands.add_parser("all").add_argument("root", type=Path)
    args = parser.parse_args()
    if args.command == "matrix":
        print(json.dumps(SHARDS))
        return 0
    if args.command == "prepare":
        prepare(args.root)
        return 0
    if args.command == "run":
        return 0 if run_shard(args.shard, args.root) else 1
    if args.command == "all":
        prepare(args.root / "inputs")
        # Local execution shares one Cargo target directory. CI uses isolated runners.
        for shard in SHARDS:
            run_shard(shard, args.root)
    return 0 if aggregate(args.root, getattr(args, "jobs_result", "success") == "success") else 1


if __name__ == "__main__":
    sys.exit(main())
