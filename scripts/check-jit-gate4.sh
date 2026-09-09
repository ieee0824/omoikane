#!/usr/bin/env bash
# Run every Gate 4 partition locally; CI uses separate runners for the same partitions.
set -euo pipefail
if (($#)); then
  echo 'Set CARGO_BUILD_TARGET for cross execution; this gate accepts no test filters.' >&2
  exit 2
fi
cd "$(git rev-parse --show-toplevel)"
gate_dir=${OMOIKANE_JIT_GATE_REPORT_DIR:-".artifacts/js-benchmark/gate4-$(date -u +%Y%m%dT%H%M%SZ)-$$"}
exec python3 scripts/jit-gate4.py all "$gate_dir"
