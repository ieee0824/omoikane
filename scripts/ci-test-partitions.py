#!/usr/bin/env python3
"""Plan the complete default Cargo test scope without linking it all at once.

Examples normally compile without a test harness; preserve that distinction.
See https://doc.rust-lang.org/cargo/commands/cargo-test.html#target-selection.
"""

import argparse
from collections import Counter
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import time


PARTITIONS = ("unit-build", "integration-0", "integration-1")
# These are the existing normal CI's manual utilities, not regression tests.
MANUAL_FIXTURES = (
    "refresh_acid2_baseline_png",
    "refresh_fullscreen_top_layer_baseline_png",
    "refresh_popover_top_layer_baseline_png",
    "profile_inline_style_subtree_invalidation_against_full_cache_clear",
    "profile_hidden_subtree_geometry_retention_against_full_rebuild",
    "profile_static_document_shadow_frames",
    "profile_repeated_and_distinct_frame_shadow_masks",
    "profile_offcanvas_outer_shadow_fixed_fixture",
    "profile_fixed_transform_property_lookup_hot_paths",
    "manual_image_scaling_reference_ab",
    "static_transition_sampling_microbenchmark",
    "compound_scanline_microbenchmark",
)
LIBRARY_KINDS = {"lib", "rlib", "cdylib", "dylib", "staticlib", "proc-macro"}
ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;]*m")
TEST_RESULT = re.compile(r"^test result: (\w+)\. (\d+) passed; (\d+) failed;", re.MULTILINE)


def default_features(features):
    """Resolve root feature names; dependency feature edges are not root flags."""
    pending = ["default"] if "default" in features else []
    enabled = set()
    while pending:
        feature = pending.pop()
        if feature in enabled:
            continue
        if feature not in features:
            raise ValueError(f"unknown root feature: {feature}")
        enabled.add(feature)
        pending.extend(edge for edge in features[feature]
                       if "/" not in edge and not edge.startswith("dep:"))
    return sorted(enabled)


def target_kind(target):
    kinds = set(target["kind"])
    if kinds & LIBRARY_KINDS:
        return "lib"
    if len(kinds) != 1:
        raise ValueError(f"unexpected target kinds: {target['kind']}")
    return next(iter(kinds))


def inventory_from_metadata(metadata, root, package_name="omoikane"):
    """Select exactly the default-feature package targets from explicit metadata."""
    packages = [p for p in metadata["packages"] if p["name"] == package_name]
    if len(packages) != 1 or not root.is_absolute():
        raise ValueError("need one package and an absolute repository root")
    package = packages[0]
    enabled = default_features(package["features"])
    targets, excluded, seen = [], [], set()
    for target in package["targets"]:
        kind = target_kind(target)
        if kind == "custom-build":
            continue
        if kind not in {"lib", "bin", "test", "example", "bench"}:
            raise ValueError(f"unknown target kind: {kind}")
        if kind == "bench" and target["test"]:
            raise ValueError("benchmark tests need an explicit normal CI plan")
        key = (kind, target["name"])
        if key in seen:
            raise ValueError(f"duplicate target: {key}")
        seen.add(key)
        required = target.get("required-features", [])
        if not set(required) <= set(enabled):
            excluded.append({"kind": kind, "name": target["name"],
                             "required_features": sorted(required)})
            continue
        source = Path(target["src_path"]).relative_to(root)
        if ".." in source.parts:
            raise ValueError("target source escapes repository")
        targets.append({"kind": kind, "name": target["name"],
                        "source": source.as_posix(), "test": target["test"],
                        "doctest": target.get("doctest", False)})
    return {"package": package_name, "features": enabled,
            "targets": sorted(targets, key=lambda t: (t["kind"], t["name"])),
            "excluded": sorted(excluded, key=lambda t: (t["kind"], t["name"]))}


def integration_partition(inventory, index):
    if index not in (0, 1):
        raise ValueError("integration partition must be 0 or 1")
    targets = [t for t in inventory["targets"] if t["kind"] == "test" and t["test"]]
    return [t for t in targets
            if int.from_bytes(hashlib.sha256(t["name"].encode()).digest()[:8], "big") % 2 == index]


def test_arguments():
    arguments = ["--", "--include-ignored"]
    for name in MANUAL_FIXTURES:
        arguments.extend(["--skip", name])
    return arguments


def runnable_step(name, targets):
    command = ["cargo", "test", "--locked"]
    selectors = {"lib": "--lib", "bin": "--bin", "test": "--test", "example": "--example"}
    for target in targets:
        command.append(selectors[target["kind"]])
        if target["kind"] != "lib":
            command.append(target["name"])
    return {"name": name, "command": command + test_arguments(),
            "runs": [t["source"] for t in targets]}


def partition_plan(inventory, partition):
    """Keep normal test execution, compile-only examples, docs, Gate2 and build."""
    if partition not in PARTITIONS:
        raise ValueError(f"unknown partition: {partition}")
    if partition.startswith("integration-"):
        targets = integration_partition(inventory, int(partition[-1]))
        if not targets:
            raise ValueError("empty integration partition")
        return [runnable_step("integration", targets)]
    targets = inventory["targets"]
    unit = [t for t in targets if t["kind"] in {"lib", "bin", "example"} and t["test"]]
    steps = [runnable_step("unit", unit)] if unit else []
    examples = [t for t in targets if t["kind"] == "example" and not t["test"]]
    if examples:
        command = ["cargo", "build", "--locked", "--profile", "test"]
        for target in examples:
            command.extend(["--example", target["name"]])
        steps.append({"name": "examples", "command": command + ["--message-format=json-render-diagnostics"],
                      "builds": [t["source"] for t in examples]})
    if any(t["kind"] == "lib" and t["doctest"] for t in targets):
        steps.append({"name": "doc", "command": ["cargo", "test", "--locked", "--doc"] + test_arguments(),
                      "doctest": inventory["package"]})
    differential = [t for t in targets if t["kind"] == "test" and t["name"] == "jit_differential"]
    if len(differential) != 1:
        raise ValueError("normal CI requires the Gate2 differential target")
    steps.extend([
        {"name": "gate2", "command": ["cargo", "test", "--locked", "--features", "jit-differential",
                                      "--test", "jit_differential"], "runs": [differential[0]["source"]]},
        {"name": "build", "command": ["cargo", "build", "--locked"]},
    ])
    return steps


def current_inventory():
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], text=True))
    return inventory_from_metadata(metadata, Path.cwd())


def complete_step(planned, result, log, root):
    """An exit code alone cannot certify execution of the complete selected scope."""
    if (result.get("name") != planned["name"] or result.get("command") != planned["command"]
            or result.get("exit") != 0):
        return False
    log = ANSI_ESCAPE.sub("", log)
    summaries = TEST_RESULT.findall(log)
    if any(status != "ok" or failed != "0" for status, _, failed in summaries):
        return False
    if "runs" in planned:
        ran = re.findall(r"^\s*Running\s+(?:unittests\s+)?(.+?)\s+\([^\n]+\)\s*$", log, re.MULTILINE)
        return (Counter(ran) == Counter(planned["runs"])
                and len(summaries) == len(planned["runs"]))
    if "doctest" in planned:
        docs = re.findall(r"^\s*Doc-tests\s+(\S+)\s*$", log, re.MULTILINE)
        return docs == [planned["doctest"].replace("-", "_")] and len(summaries) == 1
    if "builds" in planned:
        built, finished = [], []
        for line in log.splitlines():
            try:
                event = json.loads(line)
            except ValueError:
                continue
            if event.get("reason") == "build-finished":
                finished.append(event.get("success"))
            if event.get("reason") != "compiler-artifact" or event.get("target", {}).get("kind") != ["example"]:
                continue
            if event.get("profile", {}).get("test") is not False:
                return False
            try:
                built.append(Path(event["target"]["src_path"]).relative_to(root).as_posix())
            except (KeyError, ValueError):
                return False
        return Counter(built) == Counter(planned["builds"]) and finished == [True]
    return bool(re.search(r"Finished\s+`dev` profile.*target\(s\)", log))


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    temporary.replace(path)


def identity():
    def output(*args):
        return subprocess.check_output(args, text=True).strip()
    return {"revision": output("git", "rev-parse", "HEAD"),
            "lock_sha256": hashlib.sha256(Path("Cargo.lock").read_bytes()).hexdigest(),
            "rustc": output("rustc", "--version", "--verbose"),
            "tracked_diff_sha256": hashlib.sha256(subprocess.check_output(["git", "diff", "--binary", "HEAD"])).hexdigest()}


def snapshot(output, stage):
    """Persist filesystem and cache/target sizes before a runner can fill its disk."""
    output.mkdir(parents=True, exist_ok=True)
    sizes = {}
    paths = {"target": Path(os.environ.get("CARGO_TARGET_DIR", "target"))}
    if stage in {"before-cache", "after-cache", "after-fonts", "after-partition"}:
        paths["cargo_home"] = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    for name, path in paths.items():
        if path.exists():
            try:
                completed = subprocess.run(["du", "-sk", str(path)], capture_output=True, text=True, timeout=10)
            except subprocess.TimeoutExpired:
                sizes[name] = {"path": str(path.absolute()), "error": "du exceeded 10 seconds"}
                continue
            sizes[name] = {"path": str(path.absolute()), "exit": completed.returncode,
                           "du": completed.stdout.strip(), "error": completed.stderr.strip()}
    usage = shutil.disk_usage(Path.cwd())
    write_json(output / (stage + ".json"), {"stage": stage, "time_unix": time.time(),
               "filesystem": {"total": usage.total, "used": usage.used, "free": usage.free},
               "sizes": sizes, "image_version": os.environ.get("ImageVersion"),
               "cargo_incremental": os.environ.get("CARGO_INCREMENTAL"),
               "cache": {"shared_key": "workspace-v2", "exact_hit": os.environ.get("OMOIKANE_CI_CACHE_HIT")}})


def capacity_limit(value):
    """Local small-fixture tests may tighten, but never enlarge, the CI limit."""
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or not 0 < value <= 10:
        raise ValueError("target capacity limit must be finite, positive and at most 10 GiB")
    return value


def cache_binding_path():
    target = Path(os.environ["CARGO_TARGET_DIR"]).resolve()
    return target / ".cargo-space-guard-worktree.json"


def save_cache_binding(backup):
    """Keep the guard's own empty-target binding outside cache maintenance."""
    path = cache_binding_path()
    if path.is_symlink():
        raise ValueError("guard binding must not be a symlink")
    binding = json.loads(path.read_text())
    if binding.get("workspace") != str(Path.cwd().resolve()) or binding.get("uid") != os.geteuid():
        raise ValueError("guard binding is not for this workspace and user")
    write_json(backup, {"target": str(path.parent), "binding": binding})


def restore_cache_binding(backup):
    """Restore only our saved metadata; never overwrite a foreign cache binding.

    rust-cache removes target-root files except CACHEDIR.TAG on a partial hit:
    https://github.com/Swatinem/rust-cache/blob/v2/src/cleanup.ts
    The workflow immediately re-runs the guard's ownership and capacity checks.
    """
    saved = json.loads(backup.read_text())
    path = cache_binding_path()
    binding = saved.get("binding")
    if (saved.get("target") != str(path.parent) or not isinstance(binding, dict)
            or binding.get("workspace") != str(Path.cwd().resolve())
            or binding.get("uid") != os.geteuid() or path.is_symlink()):
        raise ValueError("saved cache binding is not for this target, workspace and user")
    if path.exists():
        if json.loads(path.read_text()) != binding:
            raise ValueError("cache contains a different guard binding; refusing to overwrite it")
    else:
        write_json(path, binding)


def prepare(output, maximum_target_gib=10):
    maximum_target_gib = capacity_limit(maximum_target_gib)
    output.mkdir(parents=True, exist_ok=False)
    inventory = current_inventory()
    plans = {name: partition_plan(inventory, name) for name in PARTITIONS}
    write_json(output / "inputs.json", {"identity": identity(), "inventory": inventory, "plans": plans,
                                       "maximum_target_gib": maximum_target_gib})


def run_partition(inputs, output, partition):
    prepared = json.loads((inputs / "inputs.json").read_text())
    maximum_target_gib = capacity_limit(prepared["maximum_target_gib"])
    if prepared["identity"] != identity() or prepared["inventory"] != current_inventory():
        raise ValueError("partition source, toolchain, lockfile or inventory differs from preparation")
    plan = partition_plan(prepared["inventory"], partition)
    if prepared["plans"].get(partition) != plan:
        raise ValueError("prepared commands differ from the complete current plan")
    output.mkdir(parents=True, exist_ok=False)
    result = {"partition": partition, "identity": prepared["identity"], "steps": [], "complete": False}
    browser = output / "browser"
    browser.mkdir()
    (browser / "revision.txt").write_text(prepared["identity"]["revision"] + "\n")
    (browser / "toolchain.txt").write_text(prepared["identity"]["rustc"] + "\n")
    shutil.copyfile("Cargo.lock", browser / "Cargo.lock")
    with (browser / "dependencies.json").open("w") as metadata:
        subprocess.run(["cargo", "metadata", "--locked", "--format-version", "1"], stdout=metadata, check=True)
    env = dict(os.environ,
               OMOIKANE_JS_BENCH_REPORT=str((output / "js-benchmark/report.json").absolute()),
               OMOIKANE_BROWSER_REPORT_DIR=str((browser / "journeys").absolute()),
               OMOIKANE_JIT_GATE_REPORT_DIR=str((browser / "acid3").absolute()))
    for step in plan:
        snapshot(output / "space", step["name"] + "-before")
        command = ["python3", "scripts/cargo-space-guard.py", "run", "--required-free-gib", "2",
                   "--maximum-target-gib", str(maximum_target_gib), "--"] + step["command"]
        print(partition, step["name"], "starting", flush=True)
        log_path = output / (step["name"] + ".log")
        start = time.monotonic()
        with log_path.open("w") as log:
            status = subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT).returncode
        actual = {"name": step["name"], "command": step["command"], "exit": status,
                  "wall_seconds": time.monotonic() - start}
        actual["coverage"] = complete_step(step, actual, log_path.read_text(errors="replace"), Path.cwd())
        print(partition, step["name"], "exit", status, "coverage", actual["coverage"], flush=True)
        result["steps"].append(actual)
        snapshot(output / "space", step["name"] + "-after")
        write_json(output / "result.json", result)
        if status or not actual["coverage"]:
            return 1
    result["source_unchanged"] = identity() == prepared["identity"]
    result["complete"] = result["source_unchanged"]
    write_json(output / "result.json", result)
    return 0 if result["complete"] else 1


def complete_source_inputs(prepared, directory, root):
    """Require the original browser build-input evidence as well as its JSON ID."""
    browser = directory / "browser"
    try:
        current = prepared["identity"]
        if ((browser / "revision.txt").read_text().strip() != current["revision"]
                or (browser / "toolchain.txt").read_text().strip() != current["rustc"]
                or hashlib.sha256((browser / "Cargo.lock").read_bytes()).hexdigest() != current["lock_sha256"]):
            return False
        metadata = json.loads((browser / "dependencies.json").read_text())
        return inventory_from_metadata(metadata, root) == prepared["inventory"]
    except (OSError, KeyError, TypeError, ValueError):
        return False


def complete_partition(prepared, partition, result, directory, root):
    """Re-read every log; uploaded success booleans are insufficient evidence."""
    plan = partition_plan(prepared["inventory"], partition)
    if (result.get("partition") != partition or result.get("identity") != prepared["identity"]
            or result.get("complete") is not True or result.get("source_unchanged") is not True
            or len(result.get("steps", [])) != len(plan)
            or not complete_source_inputs(prepared, directory, root)):
        return False
    for planned, actual in zip(plan, result["steps"]):
        try:
            log = (directory / (planned["name"] + ".log")).read_text(errors="replace")
        except OSError:
            return False
        if not complete_step(planned, actual, log, root):
            return False
    return True


def aggregate(inputs, partitions, output, partition_job_result="success"):
    prepared = json.loads((inputs / "inputs.json").read_text())
    maximum_target_gib = capacity_limit(prepared["maximum_target_gib"])
    inventory = current_inventory()
    plans = {name: partition_plan(inventory, name) for name in PARTITIONS}
    if (prepared["identity"] != identity() or prepared["inventory"] != inventory
            or prepared["plans"] != plans):
        raise ValueError("aggregate inputs differ from the complete current source plan")
    # A failed retry may crash before uploading a new artifact. Never let an
    # older successful artifact conceal the current matrix failure.
    complete = {"partition_jobs": partition_job_result == "success"}
    passed = {}
    for partition in PARTITIONS:
        directory = partitions / partition
        try:
            result = json.loads((directory / "result.json").read_text())
            complete[partition] = complete_partition(prepared, partition, result, directory, Path.cwd())
        except (OSError, ValueError, TypeError):
            complete[partition] = False
        passed[partition] = {}
        for step in plans[partition]:
            log_path = directory / (step["name"] + ".log")
            if log_path.exists():
                matches = TEST_RESULT.findall(ANSI_ESCAPE.sub("", log_path.read_text(errors="replace")))
                passed[partition][step["name"]] = sum(int(passed) for status, passed, failed in matches
                                                     if status == "ok" and failed == "0")
    benchmarks = [partitions / name / "js-benchmark/report.json" for name in PARTITIONS]
    benchmarks = [path for path in benchmarks if path.is_file()]
    complete["benchmark_once"] = len(benchmarks) == 1
    if complete["benchmark_once"]:
        destination = Path(".artifacts/js-benchmark")
        destination.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(benchmarks[0], destination / "report.json")
    report = {"identity": prepared["identity"], "checks": complete,
              "full_suite": all(complete.values()), "passed_by_partition_step": passed,
              "maximum_target_gib": maximum_target_gib,
              "integration_targets": sum(len(integration_partition(inventory, i)) for i in (0, 1))}
    write_json(output / "gate.json", report)
    return 0 if report["full_suite"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("plan")
    prep = subparsers.add_parser("prepare")
    prep.add_argument("output", type=Path)
    prep.add_argument("--maximum-target-gib", type=float, default=10)
    run = subparsers.add_parser("run")
    run.add_argument("partition", choices=PARTITIONS)
    run.add_argument("inputs", type=Path)
    run.add_argument("output", type=Path)
    join = subparsers.add_parser("aggregate")
    join.add_argument("inputs", type=Path)
    join.add_argument("partitions", type=Path)
    join.add_argument("output", type=Path)
    join.add_argument("--partition-job-result", choices=["success", "failure", "cancelled", "skipped"], default="success")
    space = subparsers.add_parser("space")
    space.add_argument("output", type=Path)
    space.add_argument("stage")
    for name in ["save-cache-binding", "restore-cache-binding"]:
        binding = subparsers.add_parser(name)
        binding.add_argument("backup", type=Path)
    args = parser.parse_args()
    if args.command == "prepare":
        prepare(args.output, args.maximum_target_gib)
    elif args.command == "run":
        return run_partition(args.inputs, args.output, args.partition)
    elif args.command == "aggregate":
        return aggregate(args.inputs, args.partitions, args.output, args.partition_job_result)
    elif args.command == "space":
        snapshot(args.output, args.stage)
    elif args.command == "save-cache-binding":
        save_cache_binding(args.backup)
    elif args.command == "restore-cache-binding":
        restore_cache_binding(args.backup)
    else:
        inventory = current_inventory()
        print(json.dumps({"inventory": inventory,
                          "partitions": {name: partition_plan(inventory, name) for name in PARTITIONS}}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
