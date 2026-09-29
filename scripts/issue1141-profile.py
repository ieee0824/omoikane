"""Compare Acid3 test26 with baseline JIT enabled and disabled on one runner."""

import hashlib
import atexit
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import time


OUTPUT = Path(".artifacts/issue1141-profile")
OUTPUT.mkdir(parents=True, exist_ok=True)
PATCH = Path("scripts/issue1141-profile.patch")
shutil.copy2(PATCH, OUTPUT / "instrumentation.patch")
source_hashes = {
    name: hashlib.sha256(Path(name).read_bytes()).hexdigest()
    for name in ("src/js/mod.rs", "tests/acid3_common/harness.rs")
}
subprocess.run(["git", "apply", "--unidiff-zero", "--check", str(PATCH)], check=True)
subprocess.run(["git", "apply", "--unidiff-zero", str(PATCH)], check=True)
atexit.register(lambda: subprocess.run(["git", "apply", "--unidiff-zero", "--reverse", str(PATCH)], check=False))
COMMAND = [
    "cargo", "test", "--locked", "--features", "jit-stress", "--test",
    "acid3_harness", "runner_scores_100_in_both_drive_modes", "--", "--exact",
    "--nocapture", "--test-threads=1",
]
report = {
    "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
    "base_revision": subprocess.check_output(["git", "rev-parse", "HEAD^"], text=True).strip(),
    "platform": platform.platform(),
    "rustc": subprocess.check_output(["rustc", "--version", "--verbose"], text=True),
    "fixture_sha256": hashlib.sha256(Path("tests/fixtures/acid3/acid3.html").read_bytes()).hexdigest(),
    "source_sha256_before_instrumentation": source_hashes,
    "instrumentation_sha256": hashlib.sha256(PATCH.read_bytes()).hexdigest(),
    "command": COMMAND,
    "runs": [],
}

for index, mode in enumerate(("jit", "interpreter", "interpreter", "jit")):
    run_dir = OUTPUT / f"{index}-{mode}"
    run_dir.mkdir()
    env = os.environ.copy()
    env["OMOIKANE_JIT_GATE_REPORT_DIR"] = str(run_dir)
    if mode == "interpreter":
        env["OMOIKANE_ACID3_DISABLE_JIT"] = "1"
    else:
        env.pop("OMOIKANE_ACID3_DISABLE_JIT", None)
    started = time.monotonic()
    with (run_dir / "test.log").open("w") as log:
        result = subprocess.run(COMMAND, env=env, stdout=log, stderr=log, timeout=2400)
    log_text = (run_dir / "test.log").read_text(errors="replace")
    profiles = []
    for line in log_text.splitlines():
        if line.startswith("ACID3_PROFILE "):
            profiles.append(json.loads(line.removeprefix("ACID3_PROFILE ")))
    acid3_path = run_dir / "acid3.json"
    acid3 = json.loads(acid3_path.read_text()) if acid3_path.exists() else None
    row = {
        "index": index,
        "mode": mode,
        "exit": result.returncode,
        "seconds_including_build": round(time.monotonic() - started, 3),
        "profiles": profiles,
        "acid3": acid3,
    }
    report["runs"].append(row)
    (OUTPUT / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(mode, index, "exit", result.returncode,
          "direct_ms", acid3["direct"].get("test26_step_wall_ms") if acid3 else "missing",
          "profiles", len(profiles), flush=True)

report["complete"] = len(report["runs"]) == 4 and all(
    len(row["profiles"]) == 2 and row["acid3"] is not None
    for row in report["runs"]
)
report["all_contract_passed"] = report["complete"] and all(
    row["exit"] == 0
    and all(row["acid3"][drive]["score"] == 100 for drive in ("direct", "faithful"))
    for row in report["runs"]
)
(OUTPUT / "report.json").write_text(json.dumps(report, indent=2) + "\n")
if not report["complete"]:
    raise SystemExit("Acid3 profile incomplete; inspect uploaded logs")
if not report["all_contract_passed"]:
    raise SystemExit("An instrumented Acid3 contract failed; inspect uploaded logs")
