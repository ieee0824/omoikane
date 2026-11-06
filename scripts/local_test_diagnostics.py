"""Plan supplemental libtest isolation without changing the required test gate.

Listing parsing and the Gate 5 selector import are input boundaries. Planning
uses immutable values and performs no process, filesystem, or environment I/O.
"""

from collections import Counter
from dataclasses import dataclass
import importlib.util
import json
from pathlib import Path
import re
import sys
from typing import cast


@dataclass(frozen=True, order=True)
class Target:
    """A package-qualified Cargo target, with its kind normalized by the reader."""

    package: str
    kind: str
    name: str


@dataclass(frozen=True, order=True)
class Case:
    """Identify a test by target and complete libtest name."""

    target: Target
    name: str


@dataclass(frozen=True)
class Inventory:
    """Validated complete and ignored listings for one test executable."""

    target: Target
    names: tuple[str, ...]
    ignored: frozenset[str]

    @classmethod
    def from_outputs(cls, target: Target, all_tests: str, ignored_tests: str) -> "Inventory":
        """Parse completed `--list --format terse` process outputs once."""
        names = parse_listing(all_tests)
        ignored = frozenset(parse_listing(ignored_tests))
        if not ignored <= set(names):
            raise ValueError(f"ignored listing contains unknown tests: {target}")
        return cls(target, names, ignored)


@dataclass(frozen=True)
class SensitiveTest:
    """An exact test selected by an existing Gate 5 target selector."""

    label: str
    kind: str
    target_name: str | None
    name: str

    def matches(self, case: Case) -> bool:
        return (case.target.kind == self.kind
                and (self.target_name is None or case.target.name == self.target_name)
                and case.name == self.name)


@dataclass(frozen=True)
class Profile:
    """Explicit requested build features and libtest ignored policy."""

    features: tuple[str, ...] = ()
    include_ignored: bool = False


@dataclass(frozen=True)
class Group:
    """One fresh libtest process and its expected selected/ignored cases."""

    label: str
    target: Target
    arguments: tuple[str, ...]
    cases: tuple[Case, ...]
    ignored: frozenset[Case]

    @property
    def runnable(self) -> frozenset[Case]:
        return frozenset(self.cases) - self.ignored


@dataclass(frozen=True)
class Plan:
    """A complete disjoint partition; doctests must be handled separately."""

    profile: Profile
    inventory: tuple[Inventory, ...]
    groups: tuple[Group, ...]


@dataclass(frozen=True)
class Artifact:
    """A root-package test executable identified by completed Cargo output."""

    target: Target
    executable: Path
    features: tuple[str, ...]


@dataclass(frozen=True)
class Summary:
    """The counters from one completed libtest result line."""

    disposition: str
    passed: int
    failed: int
    ignored: int
    measured: int
    filtered: int


@dataclass(frozen=True)
class Observation:
    """Observed outcomes, retaining invalid/incomplete results as failures."""

    exit_code: int
    outcomes: tuple[tuple[str, str], ...]
    summary: Summary | None
    problems: tuple[str, ...]

    @property
    def valid(self) -> bool:
        return not self.problems

    @property
    def passed(self) -> bool:
        return self.valid and self.exit_code == 0


@dataclass(frozen=True)
class DocObservation:
    """Separate evidence for Cargo's complete doctest invocation."""

    exit_code: int
    summaries: tuple[Summary, ...]
    problems: tuple[str, ...]

    @property
    def passed(self) -> bool:
        return not self.problems and self.exit_code == 0


def parse_listing(output: str) -> tuple[str, ...]:
    """Reject duplicate, unrecognized, or unsupported benchmark listing lines.

    Stable libtest's terse listing has no count trailer, including when empty.
    Its caller must first verify process completion and retain the full output.
    """
    names = []
    for line in output.splitlines():
        if not line:
            continue
        match = re.fullmatch(r"(.+): test", line)
        if match is None:
            raise ValueError(f"unsupported libtest listing line: {line!r}")
        names.append(match[1])
    duplicates = [name for name, count in Counter(names).items() if count > 1]
    if duplicates:
        raise ValueError(f"duplicate libtest names: {duplicates}")
    return tuple(sorted(names))


def load_sensitive_tests(repository: Path) -> tuple[SensitiveTest, ...]:
    """Import the repository's existing Gate 5 definitions, restoring sys.path.

    Gate 5's guarded main is not called. Its module adds a search path at import;
    keeping that change local avoids affecting this tool's subsequent imports.
    """
    path = repository / "scripts/jit-gate5.py"
    spec = importlib.util.spec_from_file_location("local_diagnostics_gate5", path)
    if spec is None or spec.loader is None:
        raise ValueError(f"cannot load Gate 5 selectors: {path}")
    module = importlib.util.module_from_spec(spec)
    search_path = sys.path.copy()
    try:
        spec.loader.exec_module(module)
    finally:
        sys.path[:] = search_path
    result = []
    for label, selector, name in module.SENSITIVE_TESTS:
        if re.fullmatch(r"[a-z0-9-]+", label) is None:
            raise ValueError(f"unsupported sensitive label: {label!r}")
        if selector == ["--lib"]:
            result.append(SensitiveTest(label, "lib", None, name))
        elif len(selector) == 2 and selector[0] == "--test":
            result.append(SensitiveTest(label, "test", selector[1], name))
        else:
            raise ValueError(f"unsupported sensitive target selector: {selector}")
    return tuple(result)


def _group(label: str, inventory: Inventory, names: tuple[str, ...],
           arguments: tuple[str, ...], profile: Profile) -> Group:
    cases = tuple(Case(inventory.target, name) for name in names)
    ignored = frozenset(case for case in cases
                        if not profile.include_ignored and case.name in inventory.ignored)
    return Group(label, inventory.target, arguments, cases, ignored)


def build_plan(inventories: tuple[Inventory, ...], sensitive: tuple[SensitiveTest, ...],
               profile: Profile) -> Plan:
    """Require exact coverage before planning parallel and isolated processes.

    `--skip` in the parallel phase uses substring matching. Reject extra matches
    instead of silently omitting them, while serial processes use `--exact`.
    Ignored tests remain in the selected partition with their policy recorded.
    """
    if not inventories or not sensitive:
        raise ValueError("test inventory and sensitive definitions must not be empty")
    by_target = {inventory.target: inventory for inventory in inventories}
    if len(by_target) != len(inventories):
        raise ValueError("duplicate target inventory")
    labels = [test.label for test in sensitive]
    if len(set(labels)) != len(labels):
        raise ValueError("duplicate sensitive labels")
    all_cases = {Case(inv.target, name) for inv in inventories for name in inv.names}
    isolated = []
    for test in sensitive:
        matches = sorted(case for case in all_cases if test.matches(case))
        if len(matches) != 1:
            raise ValueError(f"sensitive {test.label}: expected 1 exact case, got {len(matches)}")
        isolated.append((test, matches[0]))
    isolated_cases = {case for _, case in isolated}
    if len(isolated_cases) != len(isolated):
        raise ValueError("the same case is selected by multiple sensitive definitions")
    skip_names = tuple(test.name for test in sensitive)
    skipped = {case for case in all_cases if any(name in case.name for name in skip_names)}
    if skipped != isolated_cases:
        raise ValueError(f"substring skip collision: {sorted(skipped - isolated_cases)}")

    common = ("--format", "pretty", "--color", "never")
    if profile.include_ignored:
        common += ("--include-ignored",)
    skip_arguments = tuple(arg for name in skip_names for arg in ("--skip", name))
    ordered = tuple(sorted(inventories, key=lambda inv: inv.target))
    groups = [
        _group(f"parallel-{index:03d}", inv,
               tuple(name for name in inv.names if Case(inv.target, name) not in skipped),
               common + skip_arguments, profile)
        for index, inv in enumerate(ordered)
    ]
    groups.extend(
        _group(f"sensitive-{test.label}", by_target[case.target], (case.name,),
               common + (case.name, "--exact", "--test-threads=1"), profile)
        for test, case in isolated
    )
    selected = Counter(case for group in groups for case in group.cases)
    if set(selected) != all_cases or any(count != 1 for count in selected.values()):
        raise ValueError("partition is incomplete or overlapping")
    return Plan(profile, ordered, tuple(groups))


def _object(value: object, label: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be a JSON object")
    return cast(dict[str, object], value)


def _text(value: object, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise ValueError(f"{label} must be a nonempty string")
    return value


def _texts(value: object, label: str) -> tuple[str, ...]:
    if not isinstance(value, list):
        raise ValueError(f"{label} must be a JSON array")
    result = tuple(_text(item, label) for item in value)
    if len(set(result)) != len(result):
        raise ValueError(f"{label} contains duplicates")
    return result


def _boolean(value: object, label: str) -> bool:
    if type(value) is not bool:
        raise ValueError(f"{label} must be a boolean")
    return value


def root_package_id(metadata_output: str, manifest: Path) -> str:
    """Read Cargo metadata v1, selecting the explicit root manifest path."""
    metadata = _object(json.loads(metadata_output), "metadata")
    version = metadata.get("version")
    if type(version) is not int or version != 1:
        raise ValueError("unsupported Cargo metadata version")
    packages = metadata.get("packages")
    if not isinstance(packages, list):
        raise ValueError("metadata packages must be a JSON array")
    matches = []
    for item in packages:
        package = _object(item, "package")
        path = Path(_text(package.get("manifest_path"), "manifest_path"))
        identifier = _text(package.get("id"), "package id")
        if path == manifest:
            matches.append(identifier)
    if len(matches) != 1:
        raise ValueError(f"expected one root manifest package, got {len(matches)}")
    return matches[0]


def _artifact(message: dict[str, object], root_id: str) -> Artifact | None:
    package = _text(message.get("package_id"), "artifact package_id")
    if package != root_id:
        return None
    profile = _object(message.get("profile"), "artifact profile")
    if not _boolean(profile.get("test"), "profile.test"):
        return None
    executable = message.get("executable")
    if executable is None:
        return None
    path = Path(_text(executable, "executable"))
    if not path.is_absolute():
        raise ValueError("Cargo executable path must be absolute")
    target = _object(message.get("target"), "artifact target")
    kinds = _texts(target.get("kind"), "target.kind")
    library_kinds = {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}
    if kinds and set(kinds) <= library_kinds:
        kind = "lib"
    elif kinds in (("test",), ("bin",), ("example",)):
        kind = kinds[0]
    else:
        raise ValueError(f"unsupported test artifact target kinds: {kinds}")
    name = _text(target.get("name"), "target.name")
    features = tuple(sorted(_texts(message.get("features"), "artifact features")))
    return Artifact(Target(package, kind, name), path, features)


def parse_artifacts(output: str, root_id: str) -> tuple[Artifact, ...]:
    """Validate completed `cargo test --no-run --message-format=json` output.

    Non-JSON build-tool output is preserved by the caller and fails preflight.
    Dependencies and non-test artifacts do not become executable test targets.
    """
    artifacts = []
    finished = False
    for line in output.splitlines():
        if not line:
            continue
        if finished:
            raise ValueError("unexpected output after Cargo build-finished")
        message = _object(json.loads(line), "Cargo message")
        reason = _text(message.get("reason"), "Cargo reason")
        if reason == "build-finished":
            if not _boolean(message.get("success"), "build success"):
                raise ValueError("Cargo build did not succeed")
            finished = True
        elif reason == "compiler-artifact":
            artifact = _artifact(message, root_id)
            if artifact is not None:
                artifacts.append(artifact)
    if not finished or not artifacts:
        raise ValueError("missing completed Cargo test artifacts")
    targets = [artifact.target for artifact in artifacts]
    paths = [artifact.executable for artifact in artifacts]
    if len(set(targets)) != len(targets) or len(set(paths)) != len(paths):
        raise ValueError("duplicate Cargo target or executable")
    return tuple(sorted(artifacts, key=lambda artifact: artifact.target))


def audit_output(group: Group, inventory: Inventory, output: str, exit_code: int) -> Observation:
    """Match pretty libtest output to planned cases, ignored policy and counts.

    Failed-test captured output follows `failures:` and is not interpreted as
    additional case results. A valid failing run remains distinct from an
    incomplete/invalid observation; neither can yield a successful diagnostic.
    """
    problems = []
    preamble = re.split(r"^failures:\s*$", output, maxsplit=1, flags=re.M)[0]
    outcomes = []
    for match in re.finditer(
        r"^test (.+?)(?: - should panic)? \.\.\. (ok|FAILED|ignored)(?:, [^\n]*)?$",
        preamble, re.M,
    ):
        outcomes.append((match[1], match[2]))
    expected = {case.name for case in group.cases}
    names = [name for name, _ in outcomes]
    if len(set(names)) != len(names):
        problems.append("duplicate case results")
    if set(names) != expected:
        problems.append(f"case coverage mismatch: missing={sorted(expected - set(names))}, "
                        f"unexpected={sorted(set(names) - expected)}")
    ignored = {name for name, outcome in outcomes if outcome == "ignored"}
    if ignored != {case.name for case in group.ignored}:
        problems.append("ignored result policy mismatch")
    running = re.findall(r"^running (\d+) tests?$", preamble, re.M)
    if running != [str(len(group.cases))]:
        problems.append("running case count mismatch")
    rows = parse_summaries(output)
    summary = None
    if len(rows) != 1:
        problems.append("missing or duplicate libtest summary")
    else:
        summary = rows[0]
        counts = Counter(outcome for _, outcome in outcomes)
        if (summary.passed, summary.failed, summary.ignored) != (
                counts["ok"], counts["FAILED"], counts["ignored"]):
            problems.append("summary counts do not match observed outcomes")
        if summary.measured != 0 or summary.filtered != len(inventory.names) - len(group.cases):
            problems.append("summary measured/filtered counts do not match inventory")
        expected_disposition = "FAILED" if summary.failed else "ok"
        if summary.disposition != expected_disposition or (exit_code == 0) != (summary.failed == 0):
            problems.append("summary disposition/exit mismatch")
    return Observation(exit_code, tuple(sorted(outcomes)), summary, tuple(problems))


def parse_summaries(output: str) -> tuple[Summary, ...]:
    """Parse complete libtest summary lines, including separate doctest batches."""
    rows = re.findall(
        r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; "
        r"(\d+) measured; (\d+) filtered out;[^\n]*$", output, re.M,
    )
    return tuple(Summary(row[0], *(int(value) for value in row[1:])) for row in rows)


def audit_doctests(output: str, exit_code: int) -> DocObservation:
    """Require completed doctest summaries; retain ignored and failed counters.

    These are invocation results, not a pre-enumerated libtest case inventory.
    """
    summaries = parse_summaries(output)
    problems = []
    if not summaries:
        problems.append("missing doctest summary")
    for summary in summaries:
        if summary.measured or summary.filtered:
            problems.append("unexpected measured/filtered doctests")
        if summary.disposition != ("FAILED" if summary.failed else "ok"):
            problems.append("doctest summary disposition mismatch")
    if (exit_code == 0) != (sum(row.failed for row in summaries) == 0):
        problems.append("doctest exit/count mismatch")
    return DocObservation(exit_code, summaries, tuple(problems))
