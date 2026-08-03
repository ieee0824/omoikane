# Gate 3-3: arithmetic native execution

Issue #531 moves the arithmetic shape from the issue #305 benchmark through
Boa's opt-in baseline JIT. After 32 loop backedges on supported x86-64 Unix
hosts, the synchronous VM enters one generated code object for the remaining
integer arithmetic, comparison, conditional branch, and backedge operations.
The initialization, return sequence, async evaluator, unsupported opcodes, and
unsupported platforms remain on the interpreter.

The native frame contains checked scalar copies rather than raw `JsValue` bits.
Every generated register write records a Number/Boolean type tag, so fallback
restores only operations that really completed and comparison results retain
their ECMAScript type. Type mismatch, int32 overflow, NaN, negative zero
(including multiplication), invalid remainder operands, and loop-limit
exhaustion resume Boa at the exact operation PC. The RX code mapping never owns
or hides a GC edge.

## Verification

```text
cargo test --features baseline-jit --test jit_arithmetic
cargo test --features baseline-jit --test jit_baseline_lowering
```

The integration suite verifies the issue #305 result, installed-entry
diagnostics, overflow, NaN, `-0`, type mismatch, and branch behavior. Boa's full
feature test suite supplies the lower-level machine-code and loop-limit checks.

## Performance evidence

On 2026-08-03, release builds leading to Boa PR #67's merge commit `21f4299f`
ran 20 repetitions of the 2,000,000-iteration issue #305 arithmetic body (40
million iterations total). Seven final-head JIT runs were 0.503-0.540 seconds
(median 0.514); five matched interpreter runs were 7.854-8.252 seconds (median
7.962). The measured speedup remained 15.5x after the final type-tag, negative
zero, async-yield, and bounded-cache fixes. Both builds used the same source,
compiler profile, host, and checksum sink; only the
`boa_engine/baseline-jit` feature differed.

The downstream Omoikane harness also compared matched dev builds after moving
the hook out of the central dispatch loop. `arith` fell from 365.1 to 11.8
ns/op (30.9x). Across the ten unsupported shapes, five became faster, four were
within 2%, and one was 3.1% slower in a single-pass noisy run; there was no
systematic unsupported-shape regression.
