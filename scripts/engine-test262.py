#!/usr/bin/env python3
"""Run and compare Test262 using current and retained original engine sources."""

import argparse
from collections import Counter
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import time
import tomllib


ROOT = Path(__file__).resolve().parents[1]
TARGETS = ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "aarch64-apple-darwin")


# #655: these ten formerly CPU-dependent cases must pass on every supported host.
# boa_tester records file stems without the .js extension.
FLOAT16_CASES = (
    'test/built-ins/DataView/prototype/setFloat16/set-values-return-undefined',
    'test/built-ins/Math/f16round/value-conversion',
    'test/built-ins/TypedArray/prototype/fill/fill-values-conversion-operations',
    'test/built-ins/TypedArray/prototype/map/return-new-typedarray-conversion-operation',
    'test/built-ins/TypedArray/prototype/set/array-arg-src-tonumber-value-conversions',
    'test/built-ins/TypedArray/prototype/set/typedarray-arg-set-values-diff-buffer-other-type-conversions',
    'test/built-ins/TypedArrayConstructors/ctors/object-arg/conversion-operation',
    'test/built-ins/TypedArrayConstructors/internals/DefineOwnProperty/conversion-operation',
    'test/built-ins/TypedArrayConstructors/internals/Set/conversion-operation',
    'test/staging/sm/Math/f16round',
)


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


def read(path):
    return json.loads(path.read_text())


def case_index(suite, parent=""):
    prefix = parent + suite["n"] + "/"
    result = {}
    for case in suite.get("t", []):
        name = prefix + case["n"]
        assert name not in result and case["r"] in ("O", "F", "I", "P"), name
        result[name] = {"status": case["r"], "ecma_version": case.get("v")}
    for child in suite.get("s", []):
        nested = case_index(child, prefix)
        assert not result.keys() & nested.keys()
        result.update(nested)
    return result


def restore_reference(destination, origin):
    """Restore the original subtree from Omoikane history, without the old fork."""
    archive = subprocess.check_output(
        ["git", "-c", "tar.umask=0022", "archive", origin["tree"]], cwd=ROOT)
    destination.mkdir(parents=True, exist_ok=False)
    with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
        assert len([member for member in bundle.getmembers() if member.isfile()]) == origin["file_count"]
        for name, expected in origin["files"].items():
            member = bundle.getmember(name)
            assert member.isfile() and member.mode & 0o777 == int(expected["mode"], 8) & 0o777
            with bundle.extractfile(member) as source:
                data = source.read()
            assert hashlib.sha256(data).hexdigest() == expected["sha256"], name
            path = destination / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            path.chmod(member.mode & 0o777)


def validate_outcomes(cases, stats, variant):
    """Retain original-engine panics; require the maintained engine to be panic-free."""
    assert variant in ("reference", "current")
    assert len(cases) == stats["t"] > 0, stats
    counts = Counter(case["status"] for case in cases.values())
    assert counts["O"] == stats["o"] and counts["I"] == stats["i"]
    assert counts["P"] == stats["p"], "incorrect panic inventory"
    if variant == "current":
        assert counts["P"] == 0, "maintained engine must not panic"
    return dict(counts)


def run(root, target, variant, test262):
    folder = (root / target / variant).resolve()
    folder.mkdir(parents=True, exist_ok=False)
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    report = {"revision": revision, "target": target, "variant": variant, "status": "running"}
    write(folder / "execution.json", report)
    try:
        host = subprocess.check_output(["rustc", "--version", "--verbose"], text=True)
        assert "host: " + target in host.splitlines(), host
        (folder / "toolchain.txt").write_text(host)
        report["toolchain_sha256"] = hashlib.sha256(host.encode()).hexdigest()
        origin = read(ROOT / "engine/boa-origin.json")
        source = ROOT / "engine/boa"
        if variant == "reference":
            source = folder / "reference-source"
            restore_reference(source, origin)
            # Only the test metadata adapter is shared. The engine, dependency
            # lock and ignore configuration retain the verified original bytes.
            metadata = Path("tests/tester/src/edition.rs")
            original = (source / metadata).read_bytes()
            (folder / "original-tester-edition.rs").write_bytes(original)
            (source / metadata).write_bytes((ROOT / "engine/boa" / metadata).read_bytes())
            report["tester_metadata_overlay"] = str(metadata)
        config_path = source / "test262_config.toml"
        config = tomllib.loads(config_path.read_text())
        # Compare both engines against the same current specification snapshot.
        # Keep the retained reference's engine, lock and ignore configuration
        # untouched; --test262-path selects the common checkout explicitly.
        suite_revision = tomllib.loads(
            (ROOT / "engine/boa/test262_config.toml").read_text())["commit"]
        actual = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=test262, text=True).strip()
        assert actual == suite_revision, (actual, suite_revision)
        report.update(validate_checkout(test262))
        subprocess.run(["git", "update-ref", "refs/heads/main", actual], cwd=test262, check=True)
        report.update(test262_revision=actual, origin_tree=origin["tree"],
                      tester_metadata_sha256=hashlib.sha256(
                          (source / "tests/tester/src/edition.rs").read_bytes()).hexdigest(),
                      source_config_revision=config["commit"],
                      source_config_sha256=hashlib.sha256(config_path.read_bytes()).hexdigest(),
                      lock_sha256=hashlib.sha256((source / "Cargo.lock").read_bytes()).hexdigest())
        (folder / "Cargo.lock").write_bytes((source / "Cargo.lock").read_bytes())
        env = dict(os.environ, RUST_MIN_STACK="8388608", RAYON_NUM_THREADS="2", GITHUB_SHA=revision)
        command = ["cargo", "run", "--locked", "--release", "--bin", "boa_tester", "--",
                   "run", "--test262-path", str(test262.resolve()), "--suite", "test", "-v",
                   "-o", str(folder / "results")]
        report["command"] = command
        start = time.monotonic()
        with (folder / "test262.log").open("w") as log:
            result = subprocess.run(command, cwd=source, env=env, stdout=log, stderr=log)
        report.update(exit=result.returncode, seconds=time.monotonic() - start)
        assert result.returncode == 0, "runner failed; see test262.log"
        paths = list((folder / "results").rglob("latest.json"))
        assert len(paths) == 1, paths
        raw = read(paths[0])
        assert raw["c"] == revision and raw["u"] == actual, "incorrect source/suite identity"
        cases = case_index(raw["r"])
        stats = raw["r"]["a"]
        # Preserve failing current runs and immutable-reference panics verbatim.
        write(folder / "cases.json", cases)
        report.update(panic_count=stats["p"], case_count=len(cases))
        counts = validate_outcomes(cases, stats, variant)
        report.update(status="completed", stats=counts)
    except Exception as error:
        report.update(status="failed", error=f"{type(error).__name__}: {error}")
    write(folder / "execution.json", report)
    print(json.dumps(report), flush=True)
    return report["status"] == "completed"


def validate_checkout(checkout):
    """Reject missing or edited fixture inputs before running the complete suite."""
    inventory = subprocess.check_output(
        ["git", "ls-tree", "-r", "-z", "--name-only", "HEAD", "test", "harness"], cwd=checkout)
    paths = [name.decode() for name in inventory.split(b"\0") if name]
    assert paths, "empty Test262 fixture inventory"
    missing = [name for name in paths if not (checkout / name).is_file()]
    assert not missing, f"incomplete Test262 checkout: {len(missing)} missing files; {missing[:10]}"
    result = subprocess.run(["git", "diff", "--quiet", "HEAD", "--", "test", "harness"], cwd=checkout)
    assert result.returncode == 0, "modified Test262 fixtures"
    return {"suite_file_count": len(paths),
            "suite_inventory_sha256": hashlib.sha256(inventory).hexdigest()}


def compare(root):
    report = {"passed": True, "targets": {}}
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    origin = read(ROOT / "engine/boa-origin.json")
    source = ROOT / "engine/boa"
    suite_revision = tomllib.loads((source / "test262_config.toml").read_text())["commit"]
    lock_sha256 = hashlib.sha256((source / "Cargo.lock").read_bytes()).hexdigest()
    current_cases = {}
    for target in TARGETS:
        try:
            before, after = (read(root / target / variant / "execution.json")
                             for variant in ("reference", "current"))
            assert before["status"] == after["status"] == "completed"
            assert before["target"] == after["target"] == target
            assert before["variant"] == "reference" and after["variant"] == "current"
            assert before["revision"] == revision, "stale source revision"
            assert before["origin_tree"] == origin["tree"], "incorrect original source"
            assert before["test262_revision"] == suite_revision, "incorrect suite revision"
            metadata_sha256 = hashlib.sha256(
                (source / "tests/tester/src/edition.rs").read_bytes()).hexdigest()
            assert before["tester_metadata_sha256"] == after["tester_metadata_sha256"] == metadata_sha256, "incorrect tester metadata adapter"
            assert before["suite_file_count"] == after["suite_file_count"] > 0, "missing suite inventory"
            assert before["suite_inventory_sha256"] == after["suite_inventory_sha256"], "different fixture inventory"
            assert before["source_config_sha256"] == origin["files"]["test262_config.toml"]["sha256"], "incorrect reference tester config"
            assert after["source_config_sha256"] == hashlib.sha256(
                (source / "test262_config.toml").read_bytes()).hexdigest(), "incorrect current tester config"
            # The retained source keeps its original lock. Dependency fixes in the
            # current engine must not overwrite or silently re-resolve that baseline.
            assert before["lock_sha256"] == origin["files"]["Cargo.lock"]["sha256"], "incorrect reference dependency lock"
            assert after["lock_sha256"] == lock_sha256, "incorrect current dependency lock"
            for field in ("revision", "test262_revision", "origin_tree", "toolchain_sha256"):
                assert before[field] == after[field], field
            a, b = (read(root / target / variant / "cases.json") for variant in ("reference", "current"))
            assert len(a) == before["case_count"] and len(b) == after["case_count"]
            assert a and b, "empty case inventory"
            for cases, execution in ((a, before), (b, after)):
                assert all(case["status"] in ("O", "F", "I", "P") for case in cases.values())
                assert dict(Counter(case["status"] for case in cases.values())) == execution["stats"]
            removed = sorted(a.keys() - b.keys())
            regressed = [name for name in sorted(a.keys() & b.keys())
                         if a[name]["ecma_version"] != b[name]["ecma_version"]
                         or (a[name] != b[name] and b[name]["status"] != "O")]
            added_failures = [name for name in sorted(b.keys() - a.keys())
                              if b[name]["status"] in ("F", "P")]
            panics = [name for name, case in b.items() if case["status"] == "P"]
            baseline_panics = [name for name, case in a.items() if case["status"] == "P"]
            unresolved_baseline_panics = [name for name in baseline_panics
                                         if b.get(name, {}).get("status") != "O"]
            changes = [{"case": name, "before": a.get(name), "after": b.get(name)}
                       for name in sorted(a.keys() | b.keys()) if a.get(name) != b.get(name)]
            required_failures = [name for name in FLOAT16_CASES
                                 if b.get(name, {}).get("status") != "O"]
            required_cases = {name: {"before": a.get(name), "after": b.get(name)}
                              for name in FLOAT16_CASES}
            current_cases[target] = b
            passed = not (removed or regressed or added_failures or panics
                          or unresolved_baseline_panics or required_failures)
            report["targets"][target] = {"passed": passed, "before": before["stats"],
                                         "after": after["stats"], "removed": removed,
                                         "regressions": regressed, "added_failures": added_failures,
                                         "panics": panics, "changes": changes,
                                         "baseline_panics": baseline_panics,
                                         "unresolved_baseline_panics": unresolved_baseline_panics,
                                         "reference_lock_sha256": before["lock_sha256"],
                                         "current_lock_sha256": after["lock_sha256"],
                                         "required_cases": required_cases,
                                         "required_failures": required_failures}
        except (OSError, ValueError, KeyError, AssertionError) as error:
            passed = False
            report["targets"][target] = {"passed": False, "error": str(error)}
        report["passed"] &= passed
    # Preserve complete cross-host differences as well as before/after changes.
    # Only the explicitly required cases are a cross-host pass gate; other known
    # Test262 limitations remain visible and are still checked for regressions.
    report["cross_target_differences"] = []
    if len(current_cases) == len(TARGETS):
        names = set().union(*(cases.keys() for cases in current_cases.values()))
        for name in sorted(names):
            results = {target: current_cases[target].get(name) for target in TARGETS}
            if any(result != results[TARGETS[0]] for result in results.values()):
                report["cross_target_differences"].append({"case": name, "results": results})
    write(root / "comparison.json", report)
    print(json.dumps(report), flush=True)
    return report["passed"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("run", "compare"))
    parser.add_argument("root", type=Path)
    parser.add_argument("--target", choices=TARGETS)
    parser.add_argument("--variant", choices=("reference", "current"))
    parser.add_argument("--test262", type=Path)
    args = parser.parse_args()
    if args.command == "run":
        assert args.target and args.variant and args.test262
        passed = run(args.root, args.target, args.variant, args.test262)
    else:
        passed = compare(args.root)
    raise SystemExit(0 if passed else 1)


if __name__ == "__main__":
    main()
