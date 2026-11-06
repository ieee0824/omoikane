#!/usr/bin/env python3
"""Run supplemental native libtest isolation with raw evidence and a manifest."""

from __future__ import annotations

import argparse
from dataclasses import asdict, dataclass
import hashlib
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import time
from typing import Any

REPOSITORY = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPOSITORY))
from scripts import local_test_diagnostics as diag

ENVIRONMENT_KEYS = (
    "CI", "RUST_TEST_THREADS", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET",
    "RUSTFLAGS", "RUSTDOCFLAGS", "CARGO_ENCODED_RUSTFLAGS", "WPT_ROOT",
    "WPT_REQUIRED", "WPT_MANIFEST", "WPT_REPORT", "WPT_JUNIT",
)


def digest(path: Path) -> str:
    """Hash a file without loading a native executable into memory."""
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def save_report(output: Path, report: dict[str, Any]) -> None:
    """Atomically replace this fresh run's own manifest."""
    temporary = output / "manifest.tmp"
    temporary.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    temporary.replace(output / "manifest.json")


@dataclass(frozen=True)
class Step:
    """A completed subprocess whose two original streams remain separate."""

    exit_code: int
    stdout: Path
    stderr: Path


class Session:
    """Own one run's subprocesses, immutable inputs and manifest updates."""

    def __init__(self, repository: Path, output: Path, environment: dict[str, str],
                 report: dict[str, Any]):
        self.repository = repository
        self.output = output
        self.environment = environment
        self.report = report

    def save(self) -> None:
        save_report(self.output, self.report)

    def step(self, label: str, command: list[str]) -> Step:
        stdout = self.output / f"{label}.stdout.log"
        stderr = self.output / f"{label}.stderr.log"
        record = {"name": label, "argv": command, "status": "running",
                  "stdout": stdout.name, "stderr": stderr.name}
        self.report["steps"].append(record)
        self.save()
        print(f"{label}: start", flush=True)
        started = time.monotonic()
        try:
            with stdout.open("xb") as out, stderr.open("xb") as err:
                result = subprocess.run(command, cwd=self.repository, env=self.environment,
                                        stdout=out, stderr=err, check=False)
        except OSError as error:
            record.update(status="invalid", error=str(error))
            self.save()
            raise
        record.update(status="completed", exit=result.returncode,
                      seconds=time.monotonic() - started,
                      stdout_sha256=digest(stdout), stderr_sha256=digest(stderr))
        self.save()
        print(f"{label}: exit={result.returncode}", flush=True)
        return Step(result.returncode, stdout, stderr)

    def required(self, label: str, command: list[str]) -> Step:
        result = self.step(label, command)
        if result.exit_code:
            raise RuntimeError(f"{label} failed: exit {result.exit_code}; see its raw logs")
        return result

    def identity(self) -> dict[str, Any]:
        def git(*arguments):
            return subprocess.check_output(["git", *arguments], cwd=self.repository,
                                           env=self.environment)

        untracked = git("ls-files", "--others", "--exclude-standard", "-z").split(b"\0")
        return {
            "revision": git("rev-parse", "HEAD").decode().strip(),
            "diff_sha256": hashlib.sha256(git("diff", "HEAD", "--binary")).hexdigest(),
            "untracked": {os.fsdecode(name): digest(self.repository / os.fsdecode(name))
                          for name in untracked if name},
            "root_lock_sha256": digest(self.repository / "Cargo.lock"),
            "engine_lock_sha256": digest(self.repository / "engine/boa/Cargo.lock"),
        }

    def wpt_inputs(self) -> dict[str, Any]:
        root = (self.repository / self.environment.get("WPT_ROOT", "target/wpt")).resolve()
        manifest = (self.repository / self.environment.get(
            "WPT_MANIFEST", "tests/wpt/manifest.json")).resolve()
        revision = None
        if (root / ".git").exists():
            result = subprocess.run(["git", "-C", str(root), "rev-parse", "HEAD"],
                                    env=self.environment, capture_output=True, text=True)
            if result.returncode == 0:
                revision = result.stdout.strip()
        return {"root": str(root), "testharness_present": (root / "resources/testharness.js").is_file(),
                "required_present": "WPT_REQUIRED" in self.environment,
                "required_value": self.environment.get("WPT_REQUIRED"),
                "manifest": str(manifest),
                "manifest_sha256": digest(manifest) if manifest.is_file() else None,
                "pinned_revision": (self.repository / "tests/wpt/revision.txt").read_text().strip(),
                "checkout_revision": revision,
                "scope": "input facts only; upstream WPT execution is not attested"}

    def prepare(self, profile: diag.Profile) -> tuple[diag.Plan, dict[diag.Target, tuple[diag.Artifact, str]]]:
        features = ["--features", ",".join(profile.features)] if profile.features else []
        self.report["identity_before"] = self.identity()
        self.report["environment"] = {key: self.environment.get(key) for key in ENVIRONMENT_KEYS}
        self.report["wpt_inputs"] = self.wpt_inputs()
        for label, command in (("rustc-version", ["rustc", "--version", "--verbose"]),
                               ("cargo-version", ["cargo", "--version"])):
            self.required(label, command)
        locate = self.required("locate", ["cargo", "locate-project", "--message-format=plain"])
        manifest = Path(locate.stdout.read_text().strip())
        metadata = self.required("metadata", ["cargo", "metadata", "--locked", "--no-deps",
                                               "--format-version=1", *features])
        root_id = diag.root_package_id(metadata.stdout.read_text(), manifest)
        build = self.required("build-tests", ["cargo", "test", "--locked", "--no-run",
                                              "--message-format=json", *features])
        artifacts = diag.parse_artifacts(build.stdout.read_text(), root_id)
        pinned = {artifact.target: (artifact, digest(artifact.executable)) for artifact in artifacts}
        self.report["binaries"] = [
            {"target": asdict(artifact.target), "executable": str(artifact.executable),
             "features": list(artifact.features), "sha256": checksum}
            for artifact, checksum in pinned.values()]
        inventories = []
        for index, artifact in enumerate(artifacts):
            self.verify(pinned[artifact.target])
            base = [str(artifact.executable), "--list", "--format", "terse"]
            all_tests = self.required(f"list-{index:03d}", base)
            ignored = self.required(f"ignored-{index:03d}", [*base, "--ignored"])
            inventories.append(diag.Inventory.from_outputs(
                artifact.target, all_tests.stdout.read_text(), ignored.stdout.read_text()))
            self.verify(pinned[artifact.target])
        plan = diag.build_plan(tuple(inventories), diag.load_sensitive_tests(self.repository), profile)
        self.report["inventory"] = [{"target": asdict(inv.target), "names": inv.names,
                                      "ignored": sorted(inv.ignored)} for inv in plan.inventory]
        self.report["groups"] = [
            {"name": group.label, "target": asdict(group.target), "arguments": group.arguments,
             "selected": [case.name for case in group.cases],
             "ignored": sorted(case.name for case in group.ignored), "status": "not-run"}
            for group in plan.groups]
        self.save()
        return plan, pinned

    @staticmethod
    def verify(pinned: tuple[diag.Artifact, str]) -> None:
        artifact, checksum = pinned
        if digest(artifact.executable) != checksum:
            raise ValueError(f"test executable changed: {artifact.executable}")

    def run_groups(self, plan: diag.Plan, pinned: dict[diag.Target, tuple[diag.Artifact, str]]) -> None:
        inventories = {inventory.target: inventory for inventory in plan.inventory}
        for group, record in zip(plan.groups, self.report["groups"], strict=True):
            record["status"] = "running"
            self.save()
            try:
                self.verify(pinned[group.target])
                artifact, _ = pinned[group.target]
                result = self.step(group.label, [str(artifact.executable), *group.arguments])
                observation = diag.audit_output(group, inventories[group.target],
                                               result.stdout.read_text(), result.exit_code)
                record.update(observation=asdict(observation),
                              status="passed" if observation.passed else
                                     "failed" if observation.valid else "invalid")
            except (OSError, ValueError) as error:
                record.update(status="invalid", error=str(error))
            self.save()

    def run_doctests(self, profile: diag.Profile) -> None:
        record = self.report["doctests"]
        record["status"] = "running"
        self.save()
        features = ["--features", ",".join(profile.features)] if profile.features else []
        command = ["cargo", "test", "--locked", "--doc", *features, "--"]
        if profile.include_ignored:
            command.append("--include-ignored")
        try:
            result = self.step("doctests", command)
            observation = diag.audit_doctests(result.stdout.read_text(), result.exit_code)
            record.update(observation=asdict(observation),
                          status="passed" if observation.passed else "failed")
        except (OSError, ValueError) as error:
            record.update(status="invalid", error=str(error))
        self.save()


def worker(options: argparse.Namespace) -> int:
    """Run under the existing guard; only the outer process finalizes success."""
    report = json.loads((options.output / "manifest.json").read_text())
    session = Session(REPOSITORY, options.output, dict(os.environ), report)
    profile = diag.Profile(tuple(filter(None, re.split(r"[,\s]+", options.features))),
                           options.include_ignored)
    try:
        plan, pinned = session.prepare(profile)
        if options.plan_only:
            report["worker_status"] = "planned"
        else:
            session.run_groups(plan, pinned)
            session.run_doctests(profile)
            report["worker_status"] = "passed" if all(
                row["status"] == "passed" for row in report["groups"]
            ) and report["doctests"]["status"] == "passed" else "failed"
        report["identity_after"] = session.identity()
        report["source_unchanged"] = report["identity_after"] == report["identity_before"]
        if not report["source_unchanged"]:
            report["worker_status"] = "failed"
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        report.update(worker_status="failed", error=f"{type(error).__name__}: {error}")
    session.save()
    return 0 if report["worker_status"] in ("passed", "planned") else 1


def wait_guard(process: subprocess.Popen) -> tuple[int, bool]:
    """Keep the actual guard exit while waiting through outer interrupts."""
    interrupted = False
    while True:
        try:
            return process.wait(), interrupted
        except KeyboardInterrupt:
            # The guard owns child-group cleanup; do not finalize while it lives.
            interrupted = True


def finalize(output: Path, guard_exit: int, interrupted: bool = False) -> int:
    """Finalize after guard termination, preserving incomplete and not-run work."""
    report = json.loads((output / "manifest.json").read_text())
    report["guard_exit"] = guard_exit
    report["interrupted"] = interrupted
    if guard_exit == 75:
        report["status"] = "resource-blocked"
    elif interrupted or guard_exit == 130:
        report["status"] = "interrupted"
    elif guard_exit == 0 and report.get("worker_status") in ("passed", "planned"):
        report["status"] = report["worker_status"]
    else:
        report["status"] = "failed"
    for row in [*report["steps"], *report["groups"], report["doctests"]]:
        if row["status"] == "running":
            row["status"] = "incomplete"
        if row["status"] in ("incomplete", "not-run"):
            row["stop_reason"] = report["status"]
    save_report(output, report)
    if report["status"] in ("passed", "planned"):
        return 0
    if report["status"] == "interrupted":
        return 130
    return guard_exit if guard_exit > 0 else 1


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__, epilog="See docs/local-test-diagnostics.md")
    result.add_argument("output", type=Path, help="fresh evidence directory outside Cargo target")
    result.add_argument("--features", default="", help="explicit Cargo features; default enables none")
    result.add_argument("--include-ignored", action="store_true")
    result.add_argument("--plan-only", action="store_true", help="compile/list only; execute no tests")
    result.add_argument("--baseline-log", type=Path, help="copy an existing default-run log as evidence")
    result.add_argument("--required-free-gib", type=float, default=22)
    result.add_argument("--minimum-free-gib", type=float, default=2)
    result.add_argument("--maximum-target-gib", type=float, default=40)
    result.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    return result


def main(arguments: list[str] | None = None) -> int:
    options = parser().parse_args(arguments)
    options.output = options.output.resolve()
    if options.worker:
        return worker(options)
    target_value = os.environ.get("CARGO_TARGET_DIR")
    if not target_value:
        parser().error("set CARGO_TARGET_DIR to this worktree's dedicated local target")
    target = Path(target_value).resolve()
    if options.output == target or target in options.output.parents:
        parser().error("evidence must be outside the Cargo target")
    limits = (options.required_free_gib, options.minimum_free_gib, options.maximum_target_gib)
    if any(not math.isfinite(value) or value < 0 for value in limits):
        parser().error("guard limits must be finite and nonnegative")
    if REPOSITORY in options.output.parents and subprocess.run(
        ["git", "check-ignore", "-q", str(options.output)], cwd=REPOSITORY,
    ).returncode != 0:
        parser().error("evidence inside the repository must be gitignored")
    if options.baseline_log and not options.baseline_log.is_file():
        parser().error("baseline log must be an existing file")
    options.output.mkdir(parents=True, exist_ok=False)
    report = {"version": 1, "status": "running", "steps": [], "groups": [],
              "doctests": {"status": "not-run"}, "requested_features": options.features,
              "include_ignored": options.include_ignored, "plan_only": options.plan_only,
              "scope": "supplemental libtest and doctest checks; does not replace required CI"}
    if options.baseline_log:
        baseline = options.output / "baseline.log"
        baseline.write_bytes(options.baseline_log.read_bytes())
        report["baseline"] = {"source": str(options.baseline_log.resolve()),
                              "sha256": digest(baseline)}
    save_report(options.output, report)
    command = [sys.executable, str(REPOSITORY / "scripts/cargo-space-guard.py"), "run",
               "--required-free-gib", str(options.required_free_gib),
               "--minimum-free-gib", str(options.minimum_free_gib),
               "--maximum-target-gib", str(options.maximum_target_gib), "--",
               sys.executable, str(Path(__file__).resolve()), str(options.output), "--worker",
               "--features=" + options.features]
    if options.include_ignored:
        command.append("--include-ignored")
    if options.plan_only:
        command.append("--plan-only")
    report["guard_argv"] = command
    save_report(options.output, report)
    environment = dict(os.environ)
    environment["CARGO_TARGET_DIR"] = str(target)
    with (options.output / "guard.stdout.log").open("xb") as out, \
            (options.output / "guard.stderr.log").open("xb") as err:
        process = subprocess.Popen(command, cwd=REPOSITORY, env=environment, stdout=out, stderr=err)
        guard_exit, interrupted = wait_guard(process)
    code = finalize(options.output, guard_exit, interrupted)
    print(f"supplemental {json.loads((options.output / 'manifest.json').read_text())['status']}: {options.output}")
    return code


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError) as error:
        print(f"local-test-diagnostics: {error}", file=sys.stderr)
        sys.exit(2)
