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

awk -F'|' -v runs="$runs" '
  function is_positive_number(value) {
    return value ~ /^([0-9]+([.][0-9]*)?|[.][0-9]+)([eE][+-]?[0-9]+)?$/ && value + 0 > 0
  }
  BEGIN {
    shape_list = "arith prop-mono prop-mega call closure-alloc object-alloc string-concat array primitive-string-property primitive-string-method proto-method"
    shape_count = split(shape_list, shape_ids, " ")
    for (shape_pos = 1; shape_pos <= shape_count; shape_pos++) {
      expected[shape_ids[shape_pos]] = 1
    }
  }
  NF != 6 {
    print "malformed benchmark row: " $0 > "/dev/stderr"
    invalid = 1
    next
  }
  {
    mode = $1
    run = $2
    shape = $3
    if ((mode != "interpreter" && mode != "jit") || run < 1 || run > runs || !(shape in expected)) {
      print "unexpected benchmark identity: " mode "|" run "|" shape > "/dev/stderr"
      invalid = 1
      next
    }
    if ($4 !~ /^[1-9][0-9]*$/ || !is_positive_number($5) || !is_positive_number($6)) {
      print "invalid benchmark measurement: " $0 > "/dev/stderr"
      invalid = 1
      next
    }
    key = mode "|" run "|" shape
    if (key in seen) {
      print "duplicate benchmark shape: " key > "/dev/stderr"
      invalid = 1
    }
    seen[key] = 1
  }
  END {
    modes[1] = "interpreter"
    modes[2] = "jit"
    for (mode_index = 1; mode_index <= 2; mode_index++) {
      for (run = 1; run <= runs; run++) {
        for (shape_index = 1; shape_index <= shape_count; shape_index++) {
          key = modes[mode_index] "|" run "|" shape_ids[shape_index]
          if (!(key in seen)) {
            print "missing benchmark shape: " key > "/dev/stderr"
            invalid = 1
          }
        }
      }
    }
    if (invalid) exit 1
  }
' "$raw"

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
