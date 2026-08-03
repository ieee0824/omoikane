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

expected_rows=$((runs * 11 * 2))
actual_rows="$(awk -F'|' 'NF == 6 { count++ } END { print count + 0 }' "$raw")"
if [[ "$actual_rows" -ne "$expected_rows" ]]; then
  echo "expected $expected_rows benchmark rows, got $actual_rows" >&2
  exit 1
fi

echo "reference_engine|$($firefox_bin --version 2>/dev/null | head -n 1)"
echo "measurement_runs|$runs"
echo "minimum_ns_per_op"
awk -F'|' '
  NF == 6 {
    key = $1 "|" $3
    value = $6 + 0
    if (!(key in minimum) || value < minimum[key]) minimum[key] = value
  }
  END {
    for (key in minimum) print key "|" minimum[key]
  }
' "$raw" | sort
