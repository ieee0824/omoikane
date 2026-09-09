# Gate 4-7: combined execution and lifetime stress

Issue #540 combines allocation-triggered collection, generated execution,
guard deoptimization, nested catch/finally/rethrow and cooperative limits in
one reproducible corpus. Each seed runs against a standalone Boa interpreter,
the Omoikane interpreter, and Omoikane with baseline JIT enabled. The standalone
reference explicitly disables generated entry even in a JIT-enabled build.
Owned results, error classification, payload identity and recovery effects must
agree across all three backends.

The generated programs vary allocation retention, call depth, guard kind and
synchronous/asynchronous execution. A 40,000-object phase collects during
allocation; the `jit-stress` feature enables Boa's GC counters and requires
actual automatic collection. Further collections run before guards, after
exceptions, and between asynchronous slices. The suite asserts generated
entries, runtime-helper entries, exception unwinds, the selected guard's deopt
counter, and interrupt exits. A separate wall-clock phase requires the stable
runtime-limit error, no late catch/finally effects, and successful recovery.

Each seed runs in an isolated child process with a 120-second test watchdog.
A crash, assertion failure or watchdog expiry fails the parent immediately;
there is no automatic retry or conversion of a failed seed into a pass. The
watchdog only bounds the test process; it is not the runtime's interruption
mechanism.

Artifacts live under `.artifacts/js-benchmark/jit-stress/` in a unique run
directory. Each child saves the seed and generated source before evaluation,
then saves the current phase, generated code bytes, stack maps and deopt
recipes before each hazardous phase. Results include per-reason deopt counters.
The first successful seed retains full snapshots; all failed seeds retain
their complete snapshots, exit status and output. A deliberately failing child
tests that the failure artifacts contain actual code and stack-map metadata.
Snapshots contain relative code offsets, not executable memory addresses, and
remain valid after the runtime is dropped.

## Reproduction and full gate

The existing native CI deoptimization target keeps the baseline JIT deopt,
exception and interrupt contracts lightweight. The `jit-stress` feature adds
this corpus, the Acid3 harness and Web API surface checks without changing the
default build. The complete gate runs every test with JIT enabled, including
ignored tests and the required pinned WPT subset, as well as a build.
Every test process still uses `--test-threads=1`: the Gate 4 feature enables
GC/JIT diagnostics with process-wide counters and timeout state. CI parallelism
uses separate Ubuntu 24.04 x86_64 runners with Rust 1.98.1:

| Partition | Coverage |
| --- | --- |
| `unit-0` through `unit-3` | All library tests, assigned by a stable hash of each exact test name |
| `integration` | Remaining integration targets, examples, enabled binaries, doctests, and build |
| `acid3` | Standalone and embedded Acid3 harness, both drive modes |
| `compatibility` | Required pinned WPT and standalone/embedded Web API surface |
| `stress` | Deopt/exception/interrupt contracts and the generated stress corpus |

The unit test lists come from Cargo/libtest. The final aggregator verifies that
all four lists agree and that their selections cover every test exactly once.
New integration targets are discovered through Cargo metadata. Acid3 and Web API
modules embedded in `jit_deopt` run in their corresponding partitions, so its
stress partition does not duplicate those heavy tests.

One preparation job resolves `Cargo.lock` and distributes it to every runner.
Each partition records its revision, compiler, target, lockfile digest, tracked
working-tree status, commands, exit codes, timings and test counts. Unit runners
share a build cache with one writer; other partitions have separate caches,
including on failure. The matrix disables fail-fast so a failure in Acid3 does
not prevent WPT or the ten-minute stress from running. This reduces elapsed time
at the cost of more concurrent runners and compilation on an initially cold
cache; measured timings must distinguish cold and warm runs.

The final `jit-stress` job always runs and retains the existing required-check
name. It collects the partition reports and writes `gate.json`. A missing,
incomplete, failed or cancelled partition, mismatched input, dirty source, or
incomplete unit coverage produces `no-go`. A `go` also requires both Acid3 modes
to reach 100/100, zero WPT/Web API regressions, and all 64 or more profiled stress
seeds to succeed over at least ten minutes. Stress continues generating new
seeds until both limits are met; it does not sleep to satisfy the duration.

Run the same partitions sequentially on one local Cargo target directory with:

```sh
scripts/check-jit-gate4.sh
```

This accepts no test filters and creates a new
`.artifacts/js-benchmark/gate4-*` directory. A failed partition does not suppress
later partitions. CI uploads each partition's logs and failure artifacts as
`jit-gate4-partition-*`, and the combined decision as `jit-gate4-report`.
Stress snapshots remain under `.artifacts/js-benchmark/jit-stress/`.
`OMOIKANE_JIT_STRESS_MIN_SECONDS` controls duration for local experiments; a
shorter run cannot produce a full-gate `go`. Unsupported JIT architectures do
not produce a native gate result.

The partition and aggregation regression tests run before the CI matrix:

```sh
python3 -m unittest discover -s scripts/tests -p 'test_jit_gate4.py'
```

For a particular failing seed, run the parent matrix with a one-seed range;
this creates a fresh artifact directory without overwriting the original:

```sh
OMOIKANE_JIT_STRESS_FIRST_SEED=17 OMOIKANE_JIT_STRESS_SEEDS=1 \
  OMOIKANE_JIT_STRESS_MIN_SECONDS=0 \
  cargo test --features jit-stress --test jit_deopt \
  stress::reproducible_stress_matrix -- --exact --nocapture
```

Cross execution uses Cargo's normal target/linker/runner configuration. Because
the stress test also launches child executables, set its runner as a JSON
argument array, for example:

```sh
export CARGO_BUILD_TARGET=x86_64-unknown-linux-gnu
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER='qemu-x86_64 -L /usr/x86_64-linux-gnu'
export OMOIKANE_JIT_STRESS_RUNNER='["qemu-x86_64","-L","/usr/x86_64-linux-gnu"]'
scripts/check-jit-gate4.sh
```

## Decision

The authoritative decision and observed counts are in `gate.json`, uploaded as
`jit-gate4-report` for the exact CI revision. The associated
[Issue #540](https://github.com/ieee0824/omoikane/issues/540) and
[PR #635](https://github.com/ieee0824/omoikane/pull/635) record that run and its
outcome. A partial or unsupported-target run does not establish Gate 4 completion.

Acid3 uses the normal five-second execution deadline. ID lookup and sibling
position lookup read native DOM data without building JavaScript wrapper lists;
Range removal shares the sibling position across affected ranges. Ordinary DOM
mutations amortize weak-cache maintenance, while browsing-context retirement
still processes discarded wrappers immediately. These remove repeated DOM
walks without relaxing the deadline, score requirement or lifetime checks.
