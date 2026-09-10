# Gate 3-3: arithmetic native execution

Issues #531 and #542 move the arithmetic shape from the issue #305 benchmark
through Boa's opt-in baseline JIT on Linux/macOS x86_64 and ARM64. After 32 hot
loop backedges, the VM enters generated code for the remaining safe-integer
arithmetic, comparisons, conditional branches, and backedge operations. Async
entry requires enough instruction budget for a bounded native segment. Backedge
polls return to the shared deadline, budget and iteration-limit checks. The
initialization, return sequence, unsupported opcodes and unsupported platforms
remain on the interpreter.

The native frame contains checked scalar copies rather than raw `JsValue` bits.
Every generated scalar write records a Number/Boolean type tag, so fallback
restores only operations that really completed and comparison results retain
their ECMAScript type. Type mismatch, results outside the exact safe-integer range, NaN, negative zero
(including multiplication), invalid remainder operands, and loop-limit
exhaustion resume Boa at the exact operation PC. The RX code mapping never owns
or hides a GC edge.

## ARM64 register and spill contract

The ARM64 emitter is a leaf using x0 for the borrowed frame, x9/x10 for scalar
values and side tags, x11/x12 for operands, and x13/x14 for temporaries
and large-index addressing. It preserves x18, all callee-saved registers and the
native stack. Every completed bytecode stores its result and type tag before an
exit. The existing frame descriptors and deopt recipes recover the exact
bytecode PC and completed writes.

Multiplication checks both halves of the signed 128-bit product before accepting
an i64 result, then applies the same 53-bit guard as x86_64. Remainder uses signed
division and multiply-subtract, preserving division-by-zero and negative-zero
fallbacks. Other Number values keep the interpreter's semantics. Property
lowering and rooted object-alias restoration are covered by
[#543's property contract](gate5-properties.md).

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
