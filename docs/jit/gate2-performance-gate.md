# Gate 2 performance gate

Issue #528 closes Gate 2 with a reproducible measurement protocol and a bounded
decision. It does not enable JIT code in production.

## Reproduction

Build once, keep the machine otherwise idle, and pin the test process to one
CPU. The harness runs the same 11 workloads, iteration counts, and four timed
passes used by `tests/js_benchmark/baseline.json`. Five independent runtime
processes are reduced by the per-shape median; every raw sample and its range is
retained in the JSON report.

```sh
cargo test --test js_benchmark --no-run
OMOIKANE_JS_BENCH_RUNS=5 \
OMOIKANE_BENCH_REVISION="$(git rev-parse HEAD)" \
OMOIKANE_BENCH_ENVIRONMENT="linux-x86_64 CPU0, idle" \
OMOIKANE_JS_BENCH_REPORT=.artifacts/js-benchmark/gate2.json \
taskset -c 0 cargo test --test js_benchmark \
  js_execution_benchmark_reports_every_shape -- --nocapture
```

`OMOIKANE_JS_BENCH_RUNS` accepts 1 through 9. CI deliberately keeps the default
of one process so ordinary pull requests do not become several minutes slower;
the five-process command is the release/gate protocol. Reports identify the
revision, environment label, target, profile, pass count, sample count, raw
samples, median, and range. The harness rejects an iteration/pass mismatch and
rejects non-positive or non-finite timings.

The SpiderMonkey interpreter numbers are the recorded Firefox reference
under matching workload iteration/pass conditions. They remain an auxiliary
cross-engine reference: they were measured on another runner and are not used as
a wall-clock CI assertion.

### Recording the SpiderMonkey reference

Run the same `shapes.js` in five fresh Firefox profiles for both interpreter and
JIT modes:

```sh
OMOIKANE_SM_BENCH_SHOW_SAMPLES=0 \
  ./scripts/record-spidermonkey-reference.sh
```

The script disables Baseline/Ion for interpreter mode, leaves normal tiering
enabled for JIT mode, verifies all 11 shapes were emitted, and prints the minimum
`ns/op` for each mode and shape. Copy those minima into
`tests/js_benchmark/baseline.json`, including the reported Firefox version in
`reference_engine`. Run the command a second time on the same idle machine. The
two sets must remain in the same performance cluster for every shape; a
multi-fold difference means the reference must not be committed. Raw
per-process samples are printed by default. Omoikane's own
`baseline_ns_per_op` remains a five-process median because it estimates typical
report runtime and has no optimizing-tier bimodality.

## Result and dominant costs

The checked-in `gate2-performance-snapshot.json` is the final five-process
snapshot. Its median gap to the auxiliary SM-interpreter reference is 10.7x to
33.8x; `prop-mono` is 13.2x and `arith` is 10.7x. The report marks its historical
baseline comparison as non-comparable because this run used a shared host while
baseline v6 used an idle two-core container. The all-shape `regressed` labels are
therefore advisory host drift, not a code-regression claim. Read the result by
workload family rather than treating all gaps as one implementation defect:

| Workload | Dominant boundary | Gate 3 relevance |
| --- | --- | --- |
| `arith` | interpreter dispatch and numeric operations | direct x86_64 arithmetic fast path |
| `prop-mono` | property bytecode dispatch plus shape guard | primary Gate 3 IC target |
| `prop-mega` | polymorphic scan and generic fallback | remains interpreter fallback |
| `call` | frame creation and call dispatch | ABI/fallback overhead |
| `closure-alloc`, `object-alloc` | allocation and GC pressure | Gate 4, not an unsafe Gate 3 shortcut |
| `string-concat` | string storage/growth | existing in-place path; no Gate 3 specialization |
| `array` | indexed property operations and growth | generic fallback in Gate 3 |
| primitive string workloads | boxing/property/method dispatch | generic fallback in Gate 3 |
| `proto-method` | receiver and prototype guards plus call | IC telemetry, then fallback |

The differential harness covers the benchmark syntax and uncaught-error
normalization through the same Boa compiler/runtime used in production. Full
module, async/generator, Proxy/exotic-object, and arbitrary builtin JIT lowering
are not claimed: they remain interpreter paths. This is a deliberate fallback
boundary, not a semantic difference accepted from compiled code.

## Frozen Gate 3 boundary

Gate 3 may depend only on the following Boa-owned contracts:

- `BYTECODE_CONTRACT_VERSION` and verified read-only instruction, register,
  constant, control-flow, handler, source-location, and nested-function data;
- `CodeBlock::jit_metadata()` entry count, fallback count, and
  Interpreter/Queued/Compiled/Disabled lifecycle. The compiled contract version
  must match before dispatch;
- `InterpreterFrameLayout` capture/restore with exact register storage, verified
  PC, and explicit Continue/Return/Throw outcomes;
- stable inline-cache index and owned Empty/Monomorphic/Polymorphic/Megamorphic
  snapshots, with opt-in hit/miss/install counters and replacement count;
- interpreter execution for every unsupported opcode, failed guard, exception,
  allocation slow path, or contract-version mismatch.

Raw GC/shape identities, mutable IC entries, executable pointers, and private
compiler structures do not cross this boundary. Gate 3 owns the dispatcher and
code memory; Gate 4 still owns native-frame roots, safepoints, deopt, exception
unwind, and interrupt soundness.

## Decision

**GO for Gate 3's test-only x86_64 `arith` and `prop-mono` experiment; NO-GO for
production enablement.**

The reason to continue is semantic parity across all 11 current workloads, a
stable verified fallback/IC contract, and measurable headroom concentrated in
the two Gate 3 targets. Reaching SpiderMonkey-interpreter wall time on a different
runner is not treated as proven. The experiment must stop if its own alternating
JIT-off/JIT-on comparison does not improve `arith` and `prop-mono`, cannot
reproduce interpreter results exactly, or bypasses fallback. Production remains
on Boa until the later GC/deopt/release gates pass.
