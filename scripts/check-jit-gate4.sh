#!/usr/bin/env bash
# Run the complete Gate 4 compatibility and reproducible stress matrix.
set -euo pipefail
if (($#)); then
  echo 'Set CARGO_BUILD_TARGET for cross execution; this gate accepts no test filters.' >&2
  exit 2
fi
cd "$(git rev-parse --show-toplevel)"
gate_dir=${OMOIKANE_JIT_GATE_REPORT_DIR:-".artifacts/js-benchmark/gate4-$(date -u +%Y%m%dT%H%M%SZ)-$$"}
mkdir -p "$(dirname "$gate_dir")"
mkdir "$gate_dir"
export OMOIKANE_JIT_GATE_REPORT_DIR="$gate_dir"
export OMOIKANE_JIT_STRESS_SEEDS=${OMOIKANE_JIT_STRESS_SEEDS:-64}
export OMOIKANE_JIT_STRESS_MIN_SECONDS=${OMOIKANE_JIT_STRESS_MIN_SECONDS:-600}
export OMOIKANE_WEB_API_REPORT="$gate_dir/web-api.json"
export WPT_ROOT=${WPT_ROOT:-.cache/wpt}
export WPT_REQUIRED=1 WPT_REPORT="$gate_dir/wpt.json" WPT_JUNIT="$gate_dir/wpt.xml"
export CI=1
git rev-parse HEAD > "$gate_dir/revision.txt"
git status --porcelain > "$gate_dir/source-status.txt"
rustc --version --verbose > "$gate_dir/rustc.txt"
if [[ ! -f Cargo.lock ]]; then
  cargo generate-lockfile > "$gate_dir/lockfile.log" 2>&1
fi
cp Cargo.lock "$gate_dir/Cargo.lock"
scripts/fetch-wpt.sh > "$gate_dir/fetch-wpt.log" 2>&1
suite_status=0
cargo test --locked --features jit-stress,jit-differential -- --include-ignored --nocapture > "$gate_dir/full-suite.log" 2>&1 || suite_status=$?
build_status=0
if ((suite_status == 0)); then
  cargo build --locked --features jit-stress > "$gate_dir/build.log" 2>&1 || build_status=$?
else
  build_status=125
fi
python3 - "$gate_dir" "$suite_status" "$build_status" <<'PY'
import json, pathlib, re, sys
root = pathlib.Path(sys.argv[1])
suite_status, build_status = map(int, sys.argv[2:])
def read(path):
    return json.loads(path.read_text()) if path.is_file() else None
acid3 = read(root / 'acid3.json')
wpt = read(root / 'wpt.json')
web_api = read(root / 'web-api.json')
log = (root / 'full-suite.log').read_text(errors='replace')
paths = re.findall(r'JIT stress: \d+ seeds passed in [\d.]+s; (.+)', log)
stress = read(pathlib.Path(paths[-1]) / 'summary.json') if paths else None
checks = {
    'full_suite': suite_status == 0,
    'build': build_status == 0,
    'acid3_100': bool(acid3) and all(acid3[mode]['score'] == 100 and acid3[mode]['total'] == 100 for mode in ('faithful', 'direct')),
    'wpt_regression_zero': bool(wpt) and wpt['summary']['regression'] == 0,
    'web_api_regression_zero': bool(web_api) and not web_api['regressions'],
    'stress_64_seeds': bool(stress) and stress['gc_profile'] and len(stress['seeds']) >= 64 and all(seed['success'] for seed in stress['seeds']),
    'stress_10_minutes': bool(stress) and stress['elapsed_seconds'] >= 600,
}
report = {
    'decision': 'go' if all(checks.values()) else 'no-go',
    'revision': (root / 'revision.txt').read_text().strip(),
    'source_dirty': bool((root / 'source-status.txt').read_text().strip()),
    'checks': checks,
    'suite_exit': suite_status,
    'build_exit': build_status,
    'passed_tests': sum(map(int, re.findall(r'^test result: ok\. (\d+) passed', log, re.M))),
    'acid3': acid3,
    'wpt_summary': wpt['summary'] if wpt else None,
    'web_api': web_api,
    'stress': stress,
    'stress_artifacts': paths[-1] if paths else None,
}
(root / 'gate.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps({'decision': report['decision'], 'checks': checks, 'artifact': str(root / 'gate.json')}, indent=2))
sys.exit(0 if report['decision'] == 'go' else 1)
PY
