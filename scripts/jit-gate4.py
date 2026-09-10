#!/usr/bin/env python3
"""Run fixed Gate 4 partitions and require complete evidence before issuing go."""

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


UNIT_SHARDS = 4
SHARDS = [f"unit-{i}" for i in range(UNIT_SHARDS)] + [
    "integration", "acid3", "compatibility", "stress",
]
FEATURES = "jit-stress,jit-differential"
DEDICATED_TARGETS = {"acid3_harness", "jit_deopt", "web_api_surface", "wpt_smoke"}
TEST_ARGS = ["--include-ignored", "--nocapture", "--test-threads=1"]
STEPS = {**{f"unit-{i}": ["list", "unit"] for i in range(UNIT_SHARDS)},
         "integration": ["integration", "examples-and-bins", "doc", "build"],
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
    (root / "revision.txt").write_text(current["revision"] + "\n")
    (root / "rustc.txt").write_text(current["rustc"] + "\n")
    (root / "source-status.txt").write_text(
        output("git", "status", "--porcelain", "--untracked-files=no") + "\n")


def unit_partition(names, index):
    return [name for name in names
            if int.from_bytes(hashlib.sha256(name.encode()).digest()[:8], "big") % UNIT_SHARDS == index]


def passed_tests(log):
    return sum(map(int, re.findall(r"^test result: ok\. (\d+) passed", log, re.M)))


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
        return run(label, ["cargo", "test", "--locked", "--features", FEATURES,
                           "--no-fail-fast", *selectors, "--", *TEST_ARGS, *filters])

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
        elif shard == "integration":
            metadata = json.loads(output("cargo", "metadata", "--no-deps", "--format-version", "1"))
            package = next(p for p in metadata["packages"] if p["name"] == "omoikane")
            targets = integration_targets(package)
            write_json(folder / "targets.json", targets)
            if not targets:
                raise ValueError("no integration targets found")
            test("integration", [arg for target in targets for arg in ("--test", target)])
            test("examples-and-bins", ["--examples", "--bins"])
            test("doc", ["--doc"])
            run("build", ["cargo", "build", "--locked", "--features", "jit-stress"])
        elif shard == "acid3":
            test("acid3", ["--test", "acid3_harness"])
            # jit_deopt also embeds the harness; keep its existing coverage.
            test("embedded-acid3", ["--test", "jit_deopt"], ["acid3::"])
        elif shard == "compatibility":
            run("fetch-wpt", ["scripts/fetch-wpt.sh"])
            test("wpt", ["--test", "wpt_smoke"])
            test("web-api", ["--test", "web_api_surface"])
            test("embedded-web-api", ["--test", "jit_deopt"], ["web_api_surface::"])
        elif shard == "stress":
            _, log = test("stress", ["--test", "jit_deopt"],
                          ["--skip", "acid3::", "--skip", "web_api_surface::"])
            paths = re.findall(r"JIT stress: \d+ seeds passed in [\d.]+s; (.+)", log)
            if paths:
                result["stress_artifacts"] = paths[-1]
                shutil.copyfile(Path(paths[-1]) / "summary.json", folder / "stress.json")
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
    checks = {
        "jobs_succeeded": jobs_succeeded,
        "same_revision": bool(expected) and expected["revision"] == output("git", "rev-parse", "HEAD"),
        "source_clean": bool(expected) and not expected["source_dirty"],
        "full_suite": complete and complete_unit_coverage(root),
        "build": any(s.get("name") == "build" and s.get("exit") == 0
                     for s in (shards.get("integration") or {}).get("steps", [])),
        "acid3_100": isinstance(acid3, dict) and all(
            acid3.get(mode, {}).get("score") == 100 and acid3[mode].get("total") == 100
            for mode in ("faithful", "direct")),
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
