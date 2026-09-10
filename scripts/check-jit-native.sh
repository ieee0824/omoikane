#!/usr/bin/env bash
set -euo pipefail
mkdir -p .artifacts/jit-native
printf '{"status":"running"}\n' > .artifacts/jit-native/gate.json
# Include child-process reproduction files even when a native contract fails.
collect_stress() {
  if [[ -d .artifacts/js-benchmark/jit-stress ]]; then
    mkdir -p .artifacts/jit-native/stress
    cp -R .artifacts/js-benchmark/jit-stress/. .artifacts/jit-native/stress/
  fi
}
finish() {
  code=$?
  trap - EXIT
  collect_stress || code=1
  if [[ "$code" == 0 ]]; then status=passed; else status=failed; fi
  printf '{"status":"%s","exit_code":%s}\n' "$status" "$code" > .artifacts/jit-native/gate.json
  exit "$code"
}
trap finish EXIT
# Resolve once when the repository's intentionally untracked lockfile is absent.
if [[ ! -f Cargo.lock ]]; then
  cargo generate-lockfile 2>&1 | tee .artifacts/jit-native/resolve.log
fi
cp Cargo.lock .artifacts/jit-native/Cargo.lock
# Probe before other binaries so denied JIT permissions get a classification.
cargo test --locked --features baseline-jit --test jit_native_gate \
  -- --nocapture --test-threads=1 2>&1 | tee .artifacts/jit-native/contracts.log
cargo test --locked --features baseline-jit \
  --test jit_code_memory --test jit_baseline_lowering \
  --test jit_arithmetic --test jit_stack_map --test jit_runtime_call \
  --test jit_gc_roots --test jit_deopt -- --nocapture --test-threads=1 \
  2>&1 | tee -a .artifacts/jit-native/contracts.log
python3 - <<'PY'
import json, pathlib, re
root = pathlib.Path('.artifacts/jit-native')
report = json.loads((root / 'native-report.json').read_text())
assert report['status'] == 'passed', report
assert report['jit_policy'] == 'execution-confirmed', report
assert len(report['cases']) == 16 and all(case['success'] for case in report['cases']), report
log = (root / 'contracts.log').read_text()
totals = re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored', log)
assert len(totals) == 8 and all(int(p) > 0 and f == i == '0' for p, f, i in totals), totals
for required in [
    'fixed_stub_uses_the_frozen_abi_and_rx_mapping',
    'reproducible_stress_matrix',
    'issue_305_function_reports_a_compiled_entry',
    'issue_305_prop_mono_shape_uses_guarded_native_slots',
    'jit_only_root_survives_minor_and_major_then_weak_ref_clears',
    'property_store_before_deopt_is_committed_exactly_once',
    'nested_dom_exception_finally_rethrow_and_opaque_identity_match_interpreter',
    'synchronous_and_asynchronous_jit_loops_share_the_sandbox_timeout',
]:
    assert re.search(r'test [\w:]*' + re.escape(required) + r' \.\.\.', log), required
print('All eight contract binaries and 16 matched native/interpreter cases passed.')
PY
