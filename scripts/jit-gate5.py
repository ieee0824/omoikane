#!/usr/bin/env python3
"""Build, exercise and aggregate the supported distribution targets for Gate 5."""

import argparse
from collections import Counter
import ctypes
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import statistics
import subprocess
import sys
import tarfile
import time
import traceback


# Also resolve the shared validator when invoked directly as a script.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from scripts.acid3_expectations import load_expectation, report_matches_expectation


TARGETS = {
    "x86_64-unknown-linux-gnu": ("libomoikane.so", "omoikane-linux-x86_64.tar.gz"),
    "aarch64-unknown-linux-gnu": ("libomoikane.so", "omoikane-linux-aarch64.tar.gz"),
    "aarch64-apple-darwin": ("libomoikane.dylib", "omoikane-macos-aarch64.tar.gz"),
}
ENGINE_NOTICES = {
    "boa-LICENSE-MIT": Path("engine/boa/LICENSE-MIT"),
    "boa-LICENSE-UNLICENSE": Path("engine/boa/LICENSE-UNLICENSE"),
    "boa-origin.json": Path("engine/boa-origin.json"),
}
SUITE_BASE_COMMAND = ["cargo", "test", "--locked", "--features",
                      "baseline-jit,jit-differential", "--no-fail-fast"]
SENSITIVE_TESTS = (
    ("p1-media", ["--test", "p1_media"],
     "media_changes_visit_nested_shadow_frames_and_skip_detached_documents"),
    ("node-lifetime", ["--lib"],
     "js::node_lifetime_tests::old_document_generations_are_kept_by_references_without_an_eviction_limit"),
    ("service-worker-scope", ["--lib"],
     "js::service_worker_tests::service_worker_checks_response_allowed_scope_and_mime_after_fetch"),
    ("service-worker-trusted-types", ["--lib"],
     "js::service_worker_tests::service_worker_trusted_type_failure_precedes_fetch_and_script_failure"),
    ("iframe-history", ["--lib"],
     "js::tests::iframe_history_rebinds_state_entries_and_reload_uses_current_url"),
    ("acid3", ["--test", "acid3_harness"],
     "runner_matches_selectors4_expectation_in_both_drive_modes"),
    ("iframe-recovery", ["--test", "iframe_policy_introspection"],
     "caught_script_errors_do_not_poison_later_iframe_realm_initialization"),
    ("wpt-custom-elements-adoption", ["--test", "wpt_smoke"],
     "selected_wpt_testharness_cases_match_expectations"),
)
SUITE_COMMAND = [*SUITE_BASE_COMMAND, "--", "--include-ignored", "--nocapture",
                 *(argument for _, _, test in SENSITIVE_TESTS
                   for argument in ("--skip", test))]
SERIAL_SUITE_COMMANDS = [
    (f"sensitive-{label}", [*SUITE_BASE_COMMAND, *selector, "--", test, "--exact",
                            "--include-ignored", "--nocapture", "--test-threads=1"])
    for label, selector, test in SENSITIVE_TESTS
]


SUITE_PARTS = ("suite-core", "suite-wpt")


def suite_commands(kind):
    """Return original Rust argv once per logical suite, partitioned for CI."""
    commands = [("full-suite", SUITE_COMMAND), *SERIAL_SUITE_COMMANDS]
    if kind == "suite":
        return commands
    if kind == "suite-core":
        return commands[:-1]
    if kind == "suite-wpt":
        return commands[-1:]
    raise ValueError(f"unknown suite kind: {kind}")


def output(*args):
    return subprocess.check_output(args, text=True).strip()


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def read(path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def identity():
    rustc = output("rustc", "--version", "--verbose")
    return {"revision": output("git", "rev-parse", "HEAD"),
            "source_dirty": bool(output("git", "status", "--porcelain", "--untracked-files=no")),
            "lock_sha256": digest(Path("Cargo.lock")), "rustc": rustc,
            "run_id": os.environ.get("GITHUB_RUN_ID"),
            "target": re.search(r"^host: (.+)$", rustc, re.M).group(1)}


def prepare(root):
    root.mkdir(parents=True, exist_ok=False)
    if not Path("Cargo.lock").exists():
        subprocess.run(["cargo", "generate-lockfile"], check=True)
    shutil.copyfile("Cargo.lock", root / "Cargo.lock")
    write(root / "identity.json", identity())


def extract_archive(archive, destination, names):
    """Copy this build's regular archive files, retaining executable permissions."""
    destination.mkdir()
    with tarfile.open(archive) as bundle:
        members = bundle.getmembers()
        assert len(members) == len(names) and {m.name for m in members} == names
        assert all(m.isfile() and Path(m.name).name == m.name for m in members)
        for member in members:
            source = bundle.extractfile(member)
            assert source is not None
            path = destination / member.name
            with source, path.open("xb") as output_file:
                shutil.copyfileobj(source, output_file)
            path.chmod(member.mode & 0o777)


def ffi_smoke(library, destination):
    """Exercise the actual extracted shared library in a separate process."""
    lib = ctypes.CDLL(str(library.resolve()))
    lib.omoikane_init.restype = ctypes.c_void_p
    lib.omoikane_free.argtypes = [ctypes.c_void_p]
    lib.omoikane_free.restype = None
    lib.omoikane_evaluate.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
    lib.omoikane_evaluate.restype = ctypes.c_void_p
    lib.omoikane_string_free.argtypes = [ctypes.c_void_p]
    lib.omoikane_string_free.restype = None
    browser = lib.omoikane_init()
    assert browser, "release library must initialize"
    cases = []
    try:
        for source, expected in [("6 * 7", 42),
                ("var e=document.createElement('div'); e.id='gate5'; document.body.appendChild(e); document.getElementById('gate5')===e", True)]:
            pointer = lib.omoikane_evaluate(browser, source.encode())
            assert pointer, source
            try:
                result = json.loads(ctypes.string_at(pointer))
            finally:
                lib.omoikane_string_free(pointer)
            assert "exceptionDetails" not in result, result
            assert result["result"]["value"] == expected, result
            cases.append({"source": source, "expected": expected, "result": result})
    finally:
        lib.omoikane_free(browser)
    write(destination, {"status": "passed", "library_sha256": digest(library), "cases": cases})


def inventory_command(kind):
    """List the full core universe or only the independently compiled WPT harness."""
    if kind == "suite-core":
        return [*SUITE_BASE_COMMAND, "--", "--list", "--include-ignored"]
    command = SERIAL_SUITE_COMMANDS[-1][1]
    boundary = command.index("--")
    test = SENSITIVE_TESTS[-1][2]
    return [*command[:boundary], "--", test, "--exact", "--list", "--include-ignored"]


def run_suite(kind, folder, env, report, run):
    """Run one suite part with independent setup and coverage evidence."""
    env.update({"CARGO_TERM_COLOR": "never", "WPT_ROOT": str(Path(env.get("WPT_ROOT", ".cache/wpt")).resolve()),
                "WPT_REQUIRED": "1", "WPT_REPORT": str(folder / "wpt.json"),
                "WPT_JUNIT": str(folder / "wpt.xml"),
                "OMOIKANE_JIT_GATE_REPORT_DIR": str(folder),
                "OMOIKANE_WEB_API_REPORT": str(folder / "web-api.json")})
    write(folder / "suite-environment.json", {key: env.get(key) for key in
        ("WPT_ROOT", "WPT_REQUIRED", "WPT_MANIFEST", "WPT_REPORT", "WPT_JUNIT",
         "OMOIKANE_JIT_GATE_REPORT_DIR", "OMOIKANE_WEB_API_REPORT", "RUST_TEST_THREADS",
         "CI", "CARGO_TARGET_DIR", "CARGO_BUILD_JOBS", "CARGO_TERM_COLOR", "TMPDIR", "HOME")})
    run("fetch-wpt", ["bash", "scripts/fetch-wpt.sh"])
    if kind in SUITE_PARTS:
        run("list-tests", inventory_command(kind))
        report["expected_tests"] = harness_tests(
            (folder / "list-tests.log").read_text(errors="replace"), listing=True)
        assert report["expected_tests"], "test inventory is empty"
    suite_steps = suite_commands(kind)
    failed_steps = [label for label, command in suite_steps
                    if run(label, command, required=False)]
    logs = {label: (folder / f"{label}.log").read_text(errors="replace")
            for label, _ in suite_steps}
    report["test_results"] = re.findall(
        r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored",
        "\n".join(logs.values()))
    if kind in SUITE_PARTS:
        report["executed_tests"] = {label: harness_tests(log) for label, log in logs.items()}
    if failed_steps:
        raise RuntimeError(f"suite steps failed: {', '.join(failed_steps)}")
    if kind in SUITE_PARTS:
        observed = sum(len(tests) for tests in report["executed_tests"].values())
        assert observed == sum(int(row[0]) for row in report["test_results"]), "unparsed test execution"
    assert report["test_results"] and sum(int(row[0]) for row in report["test_results"]) > 0
    assert all(f == i == "0" for _, f, i in report["test_results"])
    if kind != "suite-wpt":
        shutil.copyfile(".artifacts/jit-native/native-report.json", folder / "native.json")


def run_target(kind, target, root):
    folder = (root / target / kind).resolve()
    folder.mkdir(parents=True, exist_ok=False)
    report = {"identity": identity(), "kind": kind, "steps": [], "status": "running",
              "attempt": os.environ.get("GITHUB_RUN_ATTEMPT")}
    env = dict(os.environ)

    def run(label, command, required=True):
        start = time.monotonic()
        print(f"[{target}/{kind}] {label}: start", flush=True)
        with (folder / f"{label}.log").open("w") as log:
            code = subprocess.run(command, env=env, stdout=log, stderr=log).returncode
        report["steps"].append({"name": label, "command": command, "exit": code,
                                "seconds": time.monotonic() - start})
        write(folder / "result.json", report)
        if code and required:
            raise RuntimeError(f"{label} failed ({code}); {folder / f'{label}.log'}")
        return code

    try:
        assert report["identity"]["target"] == target, report["identity"]
        expected = read(root / "inputs/identity.json")
        assert expected and all(report["identity"][key] == expected[key]
                                for key in ("revision", "lock_sha256")), "revision/lock mismatch"
        if kind in ("suite", *SUITE_PARTS):
            run_suite(kind, folder, env, report, run)
        else:
            library, archive_name = TARGETS[target]
            target_dir = Path(os.environ.get("CARGO_TARGET_DIR", "target")).resolve()
            staging = folder / "staging"
            staging.mkdir()
            # Save the default library before building the opt-in diagnostic.
            # Enabling JIT for that executable does not change the shipped C library.
            run("default-library", ["cargo", "build", "--locked", "--release", "--lib", "--target", target])
            shutil.copy2(target_dir / target / "release" / library, staging / library)
            shutil.copy2("include/omoikane.h", staging / "omoikane.h")
            for name, source in ENGINE_NOTICES.items():
                shutil.copy2(source, staging / name)
            run("jit-probe-build", ["cargo", "build", "--locked", "--release", "--target", target,
                                    "--features", "baseline-jit", "--example", "jit_release_probe"])
            shutil.copy2(target_dir / target / "release/examples/jit_release_probe", staging / "omoikane-jit-smoke")
            manifest = {"identity": report["identity"], "default_library_features": [],
                        "diagnostic_features": ["baseline-jit"],
                        "files": {p.name: {"sha256": digest(p), "bytes": p.stat().st_size}
                                  for p in staging.iterdir()}}
            write(staging / "build.json", manifest)
            archive = folder / archive_name
            with tarfile.open(archive, "w:gz") as bundle:
                for path in sorted(staging.iterdir()):
                    bundle.add(path, arcname=path.name)
            extracted = folder / "extracted"
            extract_archive(archive, extracted, set(manifest["files"]) | {"build.json"})
            for name, details in manifest["files"].items():
                assert digest(extracted / name) == details["sha256"], name
            run("ffi-smoke", [sys.executable, str(Path(__file__).resolve()), "ffi-smoke",
                              str(extracted / library), str(folder / "ffi.json")])
            run("jit-smoke", [str(extracted / "omoikane-jit-smoke"), str(folder / "probe")])
            report["archive"] = {"name": archive_name, "sha256": digest(archive),
                                 "bytes": archive.stat().st_size, "manifest": manifest}
        report["status"] = "passed"
    except Exception as error:
        report["status"] = "failed"
        report["error"] = f"{type(error).__name__}: {error}"
        traceback.print_exc()
    write(folder / "result.json", report)
    return report["status"] == "passed"


def valid_probe(probe, target):
    if not isinstance(probe, dict) or probe.get("status") != "passed":
        return False
    if probe.get("architecture") != target.split("-")[0]:
        return False
    expected = {(seed, shape, sample, enabled) for seed in (305, 541, 542, 543)
                for shape in ("arith", "prop-mono") for sample in range(5) for enabled in (False, True)}
    rows = probe.get("cases", [])
    actual = {(r.get("seed"), r.get("shape"), r.get("sample"), r.get("jit_enabled")) for r in rows}
    return len(rows) == len(expected) and actual == expected and all(
        row.get("success") is True and row.get("actual") == row.get("expected")
        and row.get("elapsed_ns", 0) > 0
        and (row.get("compiled_entries", 0) > 0 and row.get("generated_code_bytes", 0) > 0
             and row.get("compile_time_ns", 0) > 0
             and (row["shape"] != "prop-mono" or row.get("property_guard_hits", 0) > 0)
             if row["jit_enabled"] else row.get("compiled_entries") == 0)
        for row in rows)


def harness_tests(log, listing=False):
    """Read Cargo harness-qualified names, retaining duplicates for coverage checks."""
    harness = None
    tests = []
    for line in log.splitlines():
        line = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", line)
        header = re.match(r"\s*Running (?:unittests )?(.+?) \([^\n]+\)\s*$", line)
        docs = re.match(r"\s*Doc-tests (\S+)\s*$", line)
        if header or docs:
            harness = header.group(1) if header else "doctest:" + docs.group(1)
            continue
        pattern = r"^(.+): test$" if listing else r"\btest (.+?) \.\.\. "
        matches = re.finditer(pattern, line)
        for match in matches:
            if harness is None:
                raise ValueError("test name without Cargo harness identity")
            name = match.group(1)
            if not listing:
                name = re.sub(r" - (?:should panic|compile fail|compile)$", "", name)
            tests.append([harness, name])
    return tests


def valid_suite_coverage(parts):
    """Require each inventory test once, with the full WPT test in its own part."""
    try:
        expected = Counter(tuple(row) for row in parts[0]["expected_tests"])
        wpt_expected = Counter(tuple(row) for row in parts[1]["expected_tests"])
        actual = []
        for kind, part in zip(SUITE_PARTS, parts, strict=True):
            rows = part["executed_tests"]
            if set(rows) != {label for label, _ in suite_commands(kind)}:
                return False
            actual.append(Counter(tuple(row) for tests in rows.values() for row in tests))
        return (bool(expected) and all(len(row) == 2 and count == 1 for row, count in expected.items())
                and len(wpt_expected) == 1 and sum(wpt_expected.values()) == 1
                and next(iter(wpt_expected))[1] == SENSITIVE_TESTS[-1][2]
                and actual[1] == wpt_expected and actual[0] + actual[1] == expected
                and not (actual[0] & actual[1]))
    except (KeyError, TypeError, ValueError):
        return False


def combine_suite_parts(base):
    """Compose validated parts without hiding absent or conflicting evidence."""
    parts = [read(base / kind / "result.json") for kind in SUITE_PARTS]
    if (base / "suite/result.json").exists():
        return None
    for kind, part in zip(SUITE_PARTS, parts, strict=True):
        if not isinstance(part, dict) or part.get("kind") != kind:
            return None
        steps = part.get("steps", [])
        expected = [("fetch-wpt", ["bash", "scripts/fetch-wpt.sh"]),
                    ("list-tests", inventory_command(kind)), *suite_commands(kind)]
        if (part.get("status") != "passed"
                or [(step.get("name"), step.get("command")) for step in steps] != expected
                or any(step.get("exit") != 0 for step in steps)
                or len(part.get("test_results", [])) < len(suite_commands(kind))):
            return None
    if parts[0].get("identity") != parts[1].get("identity") or not valid_suite_coverage(parts):
        return None
    return {"identity": parts[0]["identity"], "status": "passed", "kind": "suite",
            "steps": [parts[0]["steps"][0], *parts[0]["steps"][2:], *parts[1]["steps"][2:]],
            "test_results": [row for part in parts for row in part["test_results"]],
            "parts": dict(zip(SUITE_PARTS, parts, strict=True))}


def aggregate(root, jobs_result, split_suite=False):
    expected = read(root / "inputs/identity.json")
    checks = {"jobs_succeeded": jobs_result == "success", "same_revision": bool(expected)
              and expected.get("revision") == output("git", "rev-parse", "HEAD"),
              "source_clean": bool(expected) and expected.get("source_dirty") is False}
    summaries = {}
    for target in TARGETS:
        base = root / target
        suite = combine_suite_parts(base) if split_suite else read(base / "suite/result.json")
        package = read(base / "package/result.json")
        core_dir = base / ("suite-core" if split_suite else "suite")
        wpt_dir = base / ("suite-wpt" if split_suite else "suite")
        if split_suite:
            reports = [*(suite or {}).get("parts", {}).values(), package]
            run_id = (expected or {}).get("run_id")
            checks[target + ":run-provenance"] = (
                isinstance(run_id, str) and run_id.isdigit() and int(run_id) > 0
                and run_id == os.environ.get("GITHUB_RUN_ID") and len(reports) == 3
                and all(isinstance(part, dict) and part.get("identity", {}).get("run_id") == run_id
                        and isinstance(part.get("attempt"), str) and part["attempt"].isdigit()
                        and int(part["attempt"]) > 0 for part in reports))
            checks[target + ":toolchain"] = bool(suite and package) and (
                bool(suite["identity"].get("rustc"))
                and suite["identity"].get("rustc") == package.get("identity", {}).get("rustc"))
        checks[target + ":execution"] = all(
            isinstance(r, dict) and r.get("status") == "passed" and expected
            and all(r.get("identity", {}).get(k) == expected[k] for k in ("revision", "lock_sha256"))
            and r["identity"].get("target") == target and not r["identity"].get("source_dirty", True)
            and [s.get("name") for s in r.get("steps", [])] == steps
            and all(s.get("exit") == 0 for s in r["steps"])
            for r, steps in ((suite, ["fetch-wpt", "full-suite",
                                      *(label for label, _ in SERIAL_SUITE_COMMANDS)]),
                             (package, ["default-library", "jit-probe-build", "ffi-smoke", "jit-smoke"])))
        acid = read(core_dir / "acid3.json")
        wpt = read(wpt_dir / "wpt.json")
        web = read(core_dir / "web-api.json")
        ffi = read(base / "package/ffi.json")
        probe = read(base / "package/probe/probe.json")
        native = read(core_dir / "native.json")
        checks[target + ":full-suite"] = isinstance(suite, dict) and (
            len(suite.get("test_results", [])) >= 1 + len(SERIAL_SUITE_COMMANDS)
            and sum(int(row[0]) for row in suite["test_results"]) > 0
            and all(f == i == "0" for _, f, i in suite["test_results"])
            and [(s.get("name"), s.get("command")) for s in suite.get("steps", [])[1:]]
            == [("full-suite", SUITE_COMMAND), *SERIAL_SUITE_COMMANDS])
        checks[target + ":native-contracts"] = isinstance(native, dict) and (
            native.get("status") == "passed" and native.get("jit_policy") == "execution-confirmed"
            and native.get("architecture") == target.split("-")[0]
            and len(native.get("cases", [])) == 16
            and all(case.get("success") is True for case in native["cases"]))
        checks[target + ":acid3"] = report_matches_expectation(acid, load_expectation())
        checks[target + ":wpt"] = isinstance(wpt, dict) and (
            wpt.get("summary", {}).get("regression") == 0 and wpt["summary"].get("total", 0) > 0
            and wpt.get("revision") == Path("tests/wpt/revision.txt").read_text().strip())
        checks[target + ":web-api"] = isinstance(web, dict) and web.get("regressions") == [] and web.get("total", 0) > 0
        library = TARGETS[target][0]
        manifest = (package or {}).get("archive", {}).get("manifest", {})
        checks[target + ":ffi"] = isinstance(ffi, dict) and (
            ffi.get("status") == "passed" and len(ffi.get("cases", [])) == 2
            and bool(ffi.get("library_sha256"))
            and ffi["library_sha256"] == manifest.get("files", {}).get(library, {}).get("sha256"))
        checks[target + ":package"] = bool(package) and (
            package.get("archive", {}).get("name") == TARGETS[target][1]
            and package["archive"].get("bytes", 0) > 0
            and manifest.get("identity") == package.get("identity")
            and manifest.get("default_library_features") == []
            and manifest.get("diagnostic_features") == ["baseline-jit"]
            and set(manifest.get("files", {})) == {library, "omoikane.h", "omoikane-jit-smoke"} | set(ENGINE_NOTICES)
            and all(manifest["files"][name] == {"sha256": digest(source), "bytes": source.stat().st_size}
                    for name, source in ENGINE_NOTICES.items()))
        checks[target + ":probe"] = valid_probe(probe, target)
        metrics = {}
        if checks[target + ":probe"]:
            for shape in ("arith", "prop-mono"):
                rows = [r for r in probe["cases"] if r["shape"] == shape]
                on = [r for r in rows if r["jit_enabled"]]
                off = [r for r in rows if not r["jit_enabled"]]
                entries = sum(r["compiled_entries"] for r in on)
                metrics[shape] = {"jit_on_ns": statistics.median(r["elapsed_ns"] for r in on),
                                  "jit_off_ns": statistics.median(r["elapsed_ns"] for r in off),
                                  "compile_ns": statistics.median(r["compile_time_ns"] for r in on),
                                  "code_bytes": statistics.median(r["generated_code_bytes"] for r in on),
                                  "bailout_rate": sum(r["bailouts"] for r in on) / entries,
                                  "interrupt_bailout_rate": sum(r.get("interrupt_deopts", 0) for r in on) / entries,
                                  "non_interrupt_bailout_rate": sum(r["bailouts"] - r.get("interrupt_deopts", 0) for r in on) / entries,
                                  "property_guard_misses": sum(r.get("property_guard_misses", 0) for r in on)}
        summaries[target] = {"metrics": metrics, "suite": suite, "package": package,
                             "acid3": acid, "wpt": wpt.get("summary") if wpt else None,
                             "web_api": web}
    report = {"decision": "go" if all(checks.values()) else "no-go", "checks": checks,
              "identity": expected, "targets": summaries,
              "scope": "Gate 5 target support only; does not enable production JIT or authorize Boa removal"}
    root.mkdir(parents=True, exist_ok=True)
    write(root / "gate.json", report)
    print(json.dumps({"decision": report["decision"], "checks": checks}, indent=2))
    return report["decision"] == "go"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("prepare").add_argument("root", type=Path)
    for kind in ("suite", *SUITE_PARTS, "package"):
        command = commands.add_parser(kind)
        command.add_argument("target", choices=TARGETS)
        command.add_argument("root", type=Path)
    command = commands.add_parser("aggregate")
    command.add_argument("root", type=Path)
    command.add_argument("--jobs-result", default="success")
    command.add_argument("--split-suite", action="store_true")
    command = commands.add_parser("ffi-smoke")
    command.add_argument("library", type=Path)
    command.add_argument("destination", type=Path)
    args = parser.parse_args()
    if args.command == "prepare":
        prepare(args.root)
        return 0
    if args.command == "ffi-smoke":
        ffi_smoke(args.library, args.destination)
        return 0
    if args.command == "aggregate":
        return 0 if aggregate(args.root, args.jobs_result, args.split_suite) else 1
    return 0 if run_target(args.command, args.target, args.root) else 1


if __name__ == "__main__":
    sys.exit(main())
