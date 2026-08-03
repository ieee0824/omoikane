# Gate 3-1 x86_64 code-memory and ABI contract

Issue: #529  
Parent: #512 / #307 Gate 3

## Boundary

The code-memory substrate is owned by the Boa fork and exposed to Omoikane only
through the opt-in `baseline-jit` Cargo feature. The default feature set does
not compile or select a JIT execution path, so production JavaScript continues
to use the existing interpreter.

The first frozen entry contract is:

```text
System V AMD64: extern C fn() -> u64
```

It is intentionally limited to a constructor-validated fixed-return stub. Raw
machine bytes cannot be called through a safe public API. Gate 3-2 owns
bytecode lowering and will add validated emitters rather than weakening this
boundary.

## Memory and lifetime

- code pages begin anonymous, private, and read/write;
- publication is a one-way `mprotect` transition to read/execute, never RWX;
- Linux and macOS x86_64 use the same System V entry ABI;
- x86_64 instruction/data caches are coherent, so publication needs no explicit
  instruction-cache flush;
- each cache has a runtime identity and each insertion has a generation;
- replacement or invalidation rejects old handles before entering native code;
- dropping a code object unmaps its pages.

The cache key combines an engine-owned code identity with the bytecode/IC
version. Later IC invalidation can therefore replace or remove compiled code
without patching stale memory.

## Verification

Boa runs the focused feature tests on Linux x86_64 and an Intel macOS runner.
Omoikane repeats the public adapter contract with:

```bash
cargo test --features baseline-jit --test jit_code_memory
```

The tests cover entry/return, RX diagnostics, replacement, invalidation, stale
handle rejection, and isolation between two runtime-local caches. Opcode
lowering, arbitrary code emission, production tiering, GC stack scanning, and
deoptimization remain disabled and belong to later Gate 3/4 issues.
