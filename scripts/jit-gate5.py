#!/usr/bin/env python3
"""Build, exercise and aggregate the four native distribution targets for Gate 5."""

import argparse
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


TARGETS = {
    "x86_64-unknown-linux-gnu": ("libomoikane.so", "omoikane-linux-x86_64.tar.gz"),
    "aarch64-unknown-linux-gnu": ("libomoikane.so", "omoikane-linux-aarch64.tar.gz"),
    "x86_64-apple-darwin": ("libomoikane.dylib", "omoikane-macos-x86_64.tar.gz"),
    "aarch64-apple-darwin": ("libomoikane.dylib", "omoikane-macos-aarch64.tar.gz"),
}
SUITE_COMMAND = ["cargo", "test", "--locked", "--features", "baseline-jit,jit-differential",
                 "--", "--include-ignored", "--nocapture", "--test-threads=1"]


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


def run_target(kind, target, root):
    folder = (root / target / kind).resolve()
    folder.mkdir(parents=True, exist_ok=False)
    report = {"identity": identity(), "kind": kind, "steps": [], "status": "running"}
    env = dict(os.environ)

    def run(label, command):
        start = time.monotonic()
        print(f"[{target}/{kind}] {label}: start", flush=True)
        with (folder / f"{label}.log").open("w") as log:
            code = subprocess.run(command, env=env, stdout=log, stderr=log).returncode
        report["steps"].append({"name": label, "command": command, "exit": code,
                                "seconds": time.monotonic() - start})
        write(folder / "result.json", report)
        if code:
            raise RuntimeError(f"{label} failed ({code}); {folder / f'{label}.log'}")

    try:
        assert report["identity"]["target"] == target, report["identity"]
        expected = read(root / "inputs/identity.json")
        assert expected and all(report["identity"][key] == expected[key]
                                for key in ("revision", "lock_sha256")), "revision/lock mismatch"
        if kind == "suite":
            env.update({"WPT_ROOT": str(Path(env.get("WPT_ROOT", ".cache/wpt")).resolve()), "WPT_REQUIRED": "1",
                        "WPT_REPORT": str(folder / "wpt.json"), "WPT_JUNIT": str(folder / "wpt.xml"),
                        "OMOIKANE_JIT_GATE_REPORT_DIR": str(folder),
                        "OMOIKANE_WEB_API_REPORT": str(folder / "web-api.json")})
            run("fetch-wpt", ["bash", "scripts/fetch-wpt.sh"])
            run("full-suite", SUITE_COMMAND)
            report["test_results"] = re.findall(
                r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored",
                (folder / "full-suite.log").read_text(errors="replace"))
            assert report["test_results"] and sum(int(row[0]) for row in report["test_results"]) > 0
            assert all(f == i == "0" for _, f, i in report["test_results"])
            shutil.copyfile(".artifacts/jit-native/native-report.json", folder / "native.json")
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


def aggregate(root, jobs_result):
    expected = read(root / "inputs/identity.json")
    checks = {"jobs_succeeded": jobs_result == "success", "same_revision": bool(expected)
              and expected.get("revision") == output("git", "rev-parse", "HEAD"),
              "source_clean": bool(expected) and expected.get("source_dirty") is False}
    summaries = {}
    for target in TARGETS:
        base = root / target
        suite, package = (read(base / name / "result.json") for name in ("suite", "package"))
        checks[target + ":execution"] = all(
            isinstance(r, dict) and r.get("status") == "passed" and expected
            and all(r.get("identity", {}).get(k) == expected[k] for k in ("revision", "lock_sha256"))
            and r["identity"].get("target") == target and not r["identity"].get("source_dirty", True)
            and [s.get("name") for s in r.get("steps", [])] == steps
            and all(s.get("exit") == 0 for s in r["steps"])
            for r, steps in ((suite, ["fetch-wpt", "full-suite"]),
                             (package, ["default-library", "jit-probe-build", "ffi-smoke", "jit-smoke"])))
        acid = read(base / "suite/acid3.json")
        wpt = read(base / "suite/wpt.json")
        web = read(base / "suite/web-api.json")
        ffi = read(base / "package/ffi.json")
        probe = read(base / "package/probe/probe.json")
        native = read(base / "suite/native.json")
        checks[target + ":full-suite"] = isinstance(suite, dict) and bool(suite.get("test_results")) and (
            sum(int(row[0]) for row in suite["test_results"]) > 0
            and all(f == i == "0" for _, f, i in suite["test_results"])
            and any(s.get("name") == "full-suite" and s.get("command") == SUITE_COMMAND for s in suite.get("steps", [])))
        checks[target + ":native-contracts"] = isinstance(native, dict) and (
            native.get("status") == "passed" and native.get("jit_policy") == "execution-confirmed"
            and native.get("architecture") == target.split("-")[0]
            and len(native.get("cases", [])) == 16
            and all(case.get("success") is True for case in native["cases"]))
        checks[target + ":acid3"] = isinstance(acid, dict) and all(
            acid.get(mode, {}).get("score") == 100 and acid[mode].get("total") == 100
            for mode in ("faithful", "direct"))
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
            and set(manifest.get("files", {})) == {library, "omoikane.h", "omoikane-jit-smoke"})
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
    for kind in ("suite", "package"):
        command = commands.add_parser(kind)
        command.add_argument("target", choices=TARGETS)
        command.add_argument("root", type=Path)
    command = commands.add_parser("aggregate")
    command.add_argument("root", type=Path)
    command.add_argument("--jobs-result", default="success")
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
        return 0 if aggregate(args.root, args.jobs_result) else 1
    return 0 if run_target(args.command, args.target, args.root) else 1


if __name__ == "__main__":
    sys.exit(main())
