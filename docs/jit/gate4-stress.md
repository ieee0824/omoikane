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

The existing native CI deoptimization target includes this corpus (eight seeds),
the Acid3 harness and Web API surface checks. The optional GC profiling feature
does not change the default build. The complete gate additionally runs every
test with JIT enabled, including ignored tests and the required pinned WPT
subset, followed by a build. Stress continues through new seeds until it has
completed at least 64 seeds and run for at least ten minutes:

```sh
scripts/check-jit-gate4.sh
```

The `jit-stress` CI job runs this full gate on Ubuntu 24.04 x86_64 with Rust
1.98.1 and uploads the decision and failure artifacts even when a test fails.

The script accepts no test filters. It saves the exact revision, full test/build
logs, compiler version, dependency lockfile, working-tree status, Acid3 scores,
WPT and Web API reports, stress results, and `gate.json` in
a new `.artifacts/js-benchmark/gate4-*` directory. A `go` requires the full
suite and build to pass, both Acid3 drive modes to reach 100/100, zero WPT/Web
API regressions, and all 64 or more profiled stress seeds to succeed over at
least ten minutes. The seed loop does not sleep to satisfy the duration target.
`OMOIKANE_JIT_STRESS_MIN_SECONDS` controls duration for local experiments; a
shorter run cannot produce a full-gate `go`. Unsupported
JIT architectures do not produce a native gate result.

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

The final decision and observed counts are recorded here after the complete
gate has run on the integrated revision. A partial or unsupported-target run
does not establish Gate 4 completion.
