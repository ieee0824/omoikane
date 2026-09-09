# Gate 4-6: cooperative execution limits

Issue #539 shares `SandboxConfig` wall-clock and loop-iteration limits between
the interpreter, generated arithmetic/property loops, and generated runtime
helpers. Timeout remains an uncatchable native runtime-limit error with the
existing `JavaScript evaluation exceeded wall-clock timeout` message.

The VM checks an absolute `Instant` deadline at entry, every loop backedge and
call/property/return boundary, and at most every 256 straight-line instructions.
It checks again after a native call and before applying a resumed native result.
Native jobs check before and after invocation, including while an exclusive
asynchronous job owns the Context. Intrinsic Promise creation and settlement
retain their infallible bookkeeping contract; they can finish rejecting an
expired module without turning its timeout into a Rust panic.
An expired native result or catchable native error cannot resume JavaScript
catch/finally effects after the limit has been reached.

Generated scalar loops return through exact deoptimization metadata after at
most 4,096 iterations when a deadline is active. Dirty property slots and live
registers are restored before polling; the resumed loop increment is not
replayed. The existing deterministic loop limit still applies inside native
execution. Generated runtime helpers use the VM's call-boundary polls.

Asynchronous execution shares its instruction budget with generated slices.
The maximum slice length is conservatively derived from the loop's opcode
costs. Budgets too small for one generated iteration use the interpreter.
This permits JIT execution without monopolizing a poll or hiding work from the
cooperative scheduler. Budget, deadline, VM frames and roots are restored on
completion, error, cancellation and Rust panic.
Cancelled asynchronous Promise jobs also restore their caller's Realm, and an
expired job releases its future before reporting completion.

Omoikane computes one deadline for each asynchronous evaluation, module or
callback and passes it to both the VM and its waking/cancellation wrapper.
Synchronous script/job/callback execution also enters a deadline scope. Nested
scopes cannot extend an earlier deadline. Trusted DOM bootstrap finishes before
page limits apply, so short page budgets do not abort runtime construction.

This is cooperative interruption: parsing, compilation, a blocking native
function, or one expensive builtin is not preempted. Expiry is checked when
control returns to the VM. No OS signal, thread termination or concurrent
evaluator is used. The experimental feature remains opt-in.

Verification:

```sh
cargo test --features baseline-jit --test jit_deopt interrupts -- --nocapture
# Pinned Boa fork, on a supported x86_64 target:
cargo test -p boa_engine --features baseline-jit
```

The contract includes deadline and iteration failures with JIT on/off,
synchronous/asynchronous execution, recovery, exact deadline boundaries,
nested calls, native errors, suspended calls, cancellation, panic cleanup and
GC between generated slices. Existing Omoikane timeout tests also cover page
tasks, timers, messages, dynamic scripts and forged timeout messages.
The finite 24-Realm lifetime fixture and four-pass performance harness have
explicit 30-second and 60-second budgets respectively; their workloads and
correctness assertions are unchanged. The production default remains five seconds.
The WPT smoke reporter uses the pinned harness's `setup({output:false})` option
to disable the unused interactive HTML results table. Test result and completion
callbacks still collect every subtest, using the normal five-second execution
budget. Recorded timer/task errors participate in result classification, so a
callback timeout cannot be reported as a successful conformance run.

The arithmetic and monomorphic-property probe records nine alternating samples
of 100,000 iterations with the deadline disabled/enabled. It asserts generated
entry and equal results, but imposes no noisy timing pass/fail threshold.
`.artifacts/js-benchmark/jit-interrupt.json` is included in the existing CI
benchmark artifact. Both modes contain the native poll counter; the comparison
measures deadline activation, bounded slices and restoration overhead, not the
cost of the unconditional counter instructions relative to an older revision.

Local verification on 2026-09-09 used x86_64 Linux under QEMU on an ARM64 host,
with dependencies optimized at level 2. On Boa revision `8f0bdfe0`, which has
the same production code as the follow-up `21dde73e`, arithmetic median time was
5.781 ms without a deadline and 5.920 ms with one (1.024x); property access was
4.083 ms and 4.497 ms (1.101x). These are emulated measurements, not native
hardware performance claims. The same probe records native CI measurements as
artifacts.
