# Gate 2-1 differential boundary

Issue #522 is intentionally a test boundary, not a production dual-engine
switch. The `jit-differential` Cargo feature enables
`tests/jit_differential.rs`, which evaluates the same small JavaScript
programs through two adapters:

- a standalone Boa `Context`, representing the interpreter/fallback side;
- Omoikane's existing `JsRuntime`, including its host bindings and root
  provider.

Both adapters return a serialized value or a normalized error classification.
The cases cover arithmetic and loops, nested calls, closure capture,
control-flow, mutation side effects, caught exceptions, and uncaught errors.
This keeps the comparison at an engine-neutral boundary without duplicating a
heap or changing the production default.

Run the default compatibility check with:

```text
cargo test --test jit_differential
```

Run the differential cases with:

```text
cargo test --features jit-differential --test jit_differential
```

The result is an execution-parity oracle for later interpreter-fallback or JIT
work. It does not claim that either adapter is a native JIT entry point.
