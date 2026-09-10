#!/usr/bin/env bash
set -euo pipefail

runs="${OMOIKANE_SM_BENCH_RUNS:-5}"
firefox_bin="${FIREFOX_BIN:-firefox}"
show_samples="${OMOIKANE_SM_BENCH_SHOW_SAMPLES:-1}"
case "$runs" in
  ''|*[!0-9]*) echo "OMOIKANE_SM_BENCH_RUNS must be a positive integer" >&2; exit 2 ;;
  0) echo "OMOIKANE_SM_BENCH_RUNS must be a positive integer" >&2; exit 2 ;;
esac
command -v "$firefox_bin" >/dev/null 2>&1 || {
  echo "Firefox not found: $firefox_bin" >&2
  exit 2
}
case "$show_samples" in
  0|1) ;;
  *) echo "OMOIKANE_SM_BENCH_SHOW_SAMPLES must be 0 or 1" >&2; exit 2 ;;
esac

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
runner_url="file://${repo_root}/tests/js_benchmark/firefox-runner.html"
fixture_sha256=$(python3 - "$repo_root/tests/js_benchmark/shapes.js" <<'PYHASH'
import hashlib, pathlib, sys
print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())
PYHASH
)
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

record_mode() {
  local mode="$1"
  local run profile output
  for ((run = 1; run <= runs; run++)); do
    profile="$scratch/${mode}-${run}"
    mkdir -p "$profile"
    {
      echo 'user_pref("browser.dom.window.dump.enabled", true);'
      echo 'user_pref("dom.allow_scripts_to_close_windows", true);'
      echo 'user_pref("browser.shell.checkDefaultBrowser", false);'
      echo 'user_pref("browser.startup.homepage_override.mstone", "ignore");'
      echo 'user_pref("privacy.reduceTimerPrecision", false);'
      echo 'user_pref("privacy.resistFingerprinting", false);'
      if [[ "$mode" == interpreter ]]; then
        echo 'user_pref("javascript.options.baselinejit", false);'
        echo 'user_pref("javascript.options.ion", false);'
      else
        echo 'user_pref("javascript.options.baselinejit", true);'
        echo 'user_pref("javascript.options.ion", true);'
      fi
    } >"$profile/user.js"
    output="$scratch/${mode}-${run}.log"
    timeout 120 "$firefox_bin" --headless --no-remote --profile "$profile" \
      "$runner_url" >"$output" 2>&1 || {
        echo "Firefox $mode run $run failed; output follows" >&2
        sed -n '1,160p' "$output" >&2
        exit 1
      }
    awk -v mode="$mode" -v run="$run" '
      $0 == "OMOIKANE_BENCH_BEGIN" { capture = 1; next }
      $0 == "OMOIKANE_BENCH_END" { capture = 0; found = 1; next }
      capture && NF { print mode "|" run "|" $0 }
      END { if (!found) exit 1 }
    ' "$output" || {
      echo "Firefox $mode run $run produced no benchmark block" >&2
      sed -n '1,160p' "$output" >&2
      exit 1
    }
  done
}

raw="$scratch/raw.txt"
{
  record_mode interpreter
  record_mode jit
} >"$raw"

if [[ "$show_samples" == 1 ]]; then
  cat "$raw"
fi

report_args=()
if [[ -n "${OMOIKANE_SM_BENCH_REPORT:-}" ]]; then
  report_args=(--report "$OMOIKANE_SM_BENCH_REPORT")
fi
python3 "$repo_root/scripts/benchmark-reference.py" "$raw" --runs "$runs" --expected-fixture-sha256 "$fixture_sha256" \
  --firefox-version "$("$firefox_bin" --version 2>/dev/null | head -n 1)" "${report_args[@]}"
