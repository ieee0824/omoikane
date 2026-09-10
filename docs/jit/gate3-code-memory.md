# Native code-memory and ABI contract

Issues: #529 (x86_64), #541 (aarch64)

Parents: #512 / #514 / #307

## Boundary

The code-memory substrate is owned by the Boa fork and exposed to Omoikane only
through the opt-in `baseline-jit` Cargo feature. The default execution path stays
with the interpreter. Safe entry APIs accept constructor-validated code objects,
not arbitrary machine bytes.

| Target | Fixed entry | Result | Runtime-call frame argument |
| --- | --- | --- | --- |
| Linux/macOS x86_64 | System V AMD64 `extern C fn() -> u64` | rax | rdi |
| Linux/macOS arm64 | AAPCS64 / Apple arm64 `extern C fn() -> u64` | x0 | x0 |

The ARM64 runtime-call stub saves x29/x30, creates a frame record, calls the fixed
helper through x16 and restores the frame. It reserves x18 and preserves the
callee-saved registers. Both backends align the native stack to 16 bytes.
`JitFrameDescriptor::register_map()` records the architecture register convention;
GC-capable runtime helpers retain roots in the common engine-owned spill slots.

## Memory and lifetime

Linux and macOS x86_64 allocate anonymous private RW pages and publish them as RX
with `mprotect`. Linux arm64 also flushes instruction caches through the compiler
runtime. macOS arm64 uses `MAP_JIT`, an allowlisted write callback with per-thread
W^X, and `sys_icache_invalidate` before publication. On ARM64 virtual machines
without per-thread JIT protection, it removes execute permission before writing
and uses `mprotect` to publish RX pages. Both paths must reject writes after
publication. The ARM64 feature requires
macOS 11.4 or later.

The current macOS allocator supports normal/ad-hoc signed execution. Hardened
Runtime configurations limited to a single JIT mapping need an arena allocator;
allocation errors currently propagate. See the
[Boa ARM64 contract](https://github.com/ieee0824/boa/blob/main/docs/aarch64-jit.md)
for the host callback and signing requirements.

Each cache has a runtime identity and each insertion has a generation.
Replacement and invalidation reject old handles before native entry. Dropping a
code object unmaps its owned pages. Published code cannot be modified through the
safe API. The cache key combines engine-owned code identity with bytecode/IC
version, so invalidation can replace code without modifying retired memory.

## Verification

Boa runs the native foundation and runtime-call tests on Linux x86_64/aarch64 and
macOS Intel/ARM64. It records compiler, architecture, tested revision, code dumps
and test logs. The tests include actual register preservation, branches, changing
instructions/literals, write protection and unmapping, as well as common frame,
GC, allocation and exception contracts.

Omoikane repeats the public adapter contracts with:

```bash
cargo test --features baseline-jit --test jit_code_memory \
  --test jit_stack_map --test jit_baseline_lowering \
  --test jit_runtime_call --test jit_gc_roots
```

These foundation tests run on ARM64 too. JavaScript native-entry evidence comes
from the [arithmetic](gate3-arithmetic.md) and [property](gate5-properties.md)
execution contracts introduced by #542 and #543.
