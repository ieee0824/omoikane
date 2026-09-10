# Gate 5-3: ARM64 property and helper execution

Issue #543 ports the existing x86 property and VM helper execution model to
Linux/macOS ARM64. The shared entry code checks shape, IC slot/generation,
descriptors and numeric property values. The ARM64 leaf emits loads/stores over
those checked scalar slots. Completed writes are committed by the common write
barrier before a normal exit or exact-PC deopt; object references remain rooted
in the VM. A Boolean produced in the loop exits before storing it as a Number.

The common guard code handles stale ICs, shape changes, prototype properties,
accessors and megamorphic fallbacks. Hit/miss, bailout and deopt diagnostics use
the same counters on both architectures. This preserves the scalar-buffer design
of the x86 tier rather than adding raw object pointers to generated registers.

Generated VM helper sites use the ARM64 foundation's call ABI. Every callsite
stores its actual return-PC offset, including the ARM64 +16-byte position, in the
safepoint/exception path. GC-visible values stay in the registered VM stack or
helper roots. Nested DOM exceptions, opaque identity, finally/rethrow, allocation
failure, deadline recovery and native helper counters run on both CPU families.

The integration contracts are:

```sh
cargo test --features baseline-jit --test jit_arithmetic --test jit_deopt
cargo test --features jit-stress --test jit_deopt stress:: -- --nocapture
```

The second command reuses the same seeded property/GC/deopt/exception/timeout
corpus on ARM64 and x86_64. The default local eight-seed run is distinct from the
full Gate 4 CI requirement (64 or more seeds and at least ten minutes).
