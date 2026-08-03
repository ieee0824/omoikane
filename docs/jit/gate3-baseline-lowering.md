# Gate 3-2 baseline lowering and fallback contract

Issue: #530  
Parent: #512 / #307 Gate 3

Boa converts the verified Gate 2 bytecode snapshot into architecture-neutral
basic blocks. Each block carries its bytecode range, referenced VM registers,
successors, and either a compilable or interpreter-fallback disposition.
Unsupported instructions are isolated at their own bytecode offsets and are
never passed to a machine-code emitter.

The common controller owns the hotness threshold, one-shot compile request,
generation-checked compiled entry, bailout counts, invalidation, and retry.
Arithmetic and property emitters in the following Gate 3 issues consume this
contract instead of adding separate tiering or fallback implementations.

Emitters also populate a monotonic bytecode-to-machine-offset map. Together
with the deterministic lowering dump, this provides stable diagnostics before
architecture-specific disassembly is added by an emitter.

The feature remains opt-in. Omoikane's default build has no JIT module and
continues to execute all JavaScript in Boa's interpreter. Both default and
feature-enabled test runs execute the same arithmetic and property workloads
and assert identical results.

```bash
cargo test --test jit_baseline_lowering
cargo test --features baseline-jit --test jit_baseline_lowering
```
