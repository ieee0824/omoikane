# Issue #307 Gate 1 baseline

This document records the Gate 1 decision inputs for the Boa-fork path versus
a new JavaScript engine. It is intentionally a baseline, not an ADR: the
single-path decision belongs to #521 after #516--#520 have been reviewed.

The executable public-embedding probe is
[tests/jit_gate1.rs](../../tests/jit_gate1.rs), and the migration inventory
is [gate1-scope.json](gate1-scope.json). The probe is also an Omoikane
smoke test; it constructs JsRuntime, executes DOM bootstrap and host
bindings, drains Promise/timer work, and forces Boa GC between phases.

## Reproduction point

The Omoikane baseline is main commit 05bba9d (the commit used to start the
Gate 1 worktree). Both direct Boa dependencies in Cargo.toml point at:

~~~
1674beed49e671b991d092a9c4448fd019c275f5
~~~

Cargo.lock is intentionally ignored by this repository, so the revision in
Cargo.toml is the reproducible dependency pin. The current Boa checkout is
the 1674bee directory under Cargo's git checkout cache; it must be treated as
read-only. The checkout itself is a fork branch whose first-parent tip is
1674beed, merged from Boa-fork PR #56.

The source audit commands are:

~~~bash
rg -n 'boa_engine|boa_gc' Cargo.toml src tests --glob '*.rs'
rg -n 'pub\(crate\).*Opcode|pub\(crate\).*bytecode|pub\(crate\).*ic|pub\(crate\).*frame' \
  "$BOA_CHECKOUT/core/engine/src"
rg -n 'mmap|VirtualAlloc|executable|codegen|JIT|jit' \
  "$BOA_CHECKOUT/core" --glob '*.rs'
/usr/local/cargo/bin/cargo test --test jit_gate1
/usr/local/cargo/bin/cargo test --test boa_inline_cache
~~~

The source audit at this baseline found no machine-code allocator, executable
memory layer, or JIT code-generation dependency in the pinned Boa tree or in
Omoikane's direct dependency list.

## What the current pin contains

The pin is not stock Boa 0.21. The current fork lineage includes the changes
that Omoikane relies on:

| Area | Evidence in the pinned fork | Omoikane consumer |
| --- | --- | --- |
| Native temporary rooting | fork commits 5da9b8f7 and 0eb8a748 | native builtins and callbacks in src/js/mod.rs |
| Explicit roots and heap edges | Rooted, GcEdge, RootProvider, split realm/module/function roots | HostState root provider and retained host values |
| Generational GC | fork PR #56, tip 1674beed; nursery, remembered old parents, ephemerons | every JsRuntime, WPT/Acid3 forced-collection tests |
| Native suspension | fork commits 672547c0, 850f821c, 8b0305ee and related continuation guards | dialog, geolocation, worker and page-task suspension |
| Async module jobs | fork PR #53 and subsequent async entry-point changes | HttpModuleLoader and module evaluation |
| Shape/IC correctness | prototype-shape guard in InlineCache; Omoikane regression fixture for #057/#058 | tests/boa_inline_cache.rs and property-heavy bootstrap |

The separation is important: fork changes live in the Boa revision, while the
Omoikane-specific embedding policy lives in src/js/mod.rs. Updating the Boa
pin therefore changes both the VM/GC implementation and the API contract used
by Omoikane; the latter cannot be reviewed as a normal crates.io upgrade.

## Omoikane embedding inventory

src/js/mod.rs is 36,897 lines at this baseline and dom_bootstrap.js is
19,307 lines. The direct Boa surface is concentrated in five layers:

| Layer | Main symbols/files | Migration implication |
| --- | --- | --- |
| Script and VM entry | Context, Script, Source, JsValue, JsError; JsRuntime::eval, eval_async, run_jobs | preserve synchronous evaluation, cooperative async yielding, errors and runtime limits |
| Host state and roots | HostState, unsafe Trace, RootProvider, JsRuntime::Drop | native retained values and worker/dialog state need a precise root lifetime |
| Modules and host suspension | Module, ModuleLoader, AsyncContext, NativeFunction, NativeCallSuspension, NativeCallContinuation | a new engine needs an explicit host ABI and resumable call representation |
| DOM/Web API binding | register_host_bindings, JsObject, JsArrayBuffer, JsPromise, JsUint8Array, dom_bootstrap.js | largest compatibility surface; not part of a minimal arithmetic VM |
| Realms and task sources | WorkerRuntime, SharedWorkerRuntime, worklet runtime, event loop and module loader | dual-engine and migration boundaries must be per realm, not process-global |

The direct Rust consumers are src/js/mod.rs, src/js/event_loop.rs,
src/platform_dialog.rs, src/cdp/mod.rs, tests/acid3_common/harness.rs,
tests/js_benchmark.rs, tests/wpt_smoke.rs, and the platform dialog tests.
The exact list is kept discoverable by the first rg command above rather than
copied into a second hand-maintained list.

## #307 premise updates

Several statements in the original #307 text are now stale:

- the fork pin is 1674beed, not the old 4fc54b5c reference;
- the current benchmark has 11 shapes in baseline.json, not the nine-shape
  table in the original issue text;
- #315's GC-threshold work and #491's related close state are already on main;
- synchronous evaluation has a deterministic Boa loop-iteration limit, while
  eval_async/page-task paths have cooperative wall-clock deadline checks;
  the old blanket statement that no execution-time enforcement exists is no
  longer true, although a native JIT still needs interrupt/safepoint support;
- the existing #057/#058/#059 work protects interpreter inline-cache
  correctness. It does not provide a machine-code IC or a JIT invalidation API.

The current timeout behavior is therefore a solved embedding baseline, not a
reason to choose either Gate 1 architecture. It remains a contract that every
future engine must preserve.

## Probe results: #516 and #517

The public Boa API can parse a Script, compile its CodeBlock, and evaluate the
arithmetic workload. That is the positive part of the VM probe.

The JIT entry/return part cannot be implemented from the current external
embedding API. In the pinned tree:

- vm::Opcode and Instruction are pub(crate);
- CodeBlock.bytecode, CodeBlock.register_count, CodeBlock.ic, constants,
  handlers, and source mapping internals are pub(crate);
- Vm.frame, Vm.frames, Vm.stack, runtime state, and continuation vectors are
  pub(crate);
- InlineCache itself and match_or_reset are pub(crate);
- CallFrame::code_block() and position() are public, but the program counter,
  register pointer, environment frame pointer, and native root scan remain
  private.

This is a hard fork boundary, not a missing test. A minimum Boa-fork JIT
adapter would touch the VM/code-block/opcode/IC modules (roughly 4--6 existing
modules), add a stable read-only bytecode contract, and add per-codeblock
hotness/compiled-entry/fallback metadata. The x86_64 backend and code memory
implementation are deliberately not counted in this Gate 1 estimate; they are
Gate 3 work. The Omoikane side can remain one embedding adapter in
src/js/mod.rs if the fork owns the contract.

The safe Gate 1 outcome is therefore: the Boa fork can be a host for this work
only after an explicit private-API adapter; Omoikane cannot patch or enter a
JIT through today's public dependency.

## Probe results: #518

The existing interpreter IC has a useful mono guard contract:

1. cache the receiver WeakShape and a Slot;
2. for a prototype property, also cache the immediate holder prototype shape;
3. on a hit, use the slot; on a receiver/prototype-shape mismatch, reset the
   weak handles and take the normal property path;
4. shape insertion, deletion, attribute changes, and prototype changes create
   shape transitions, so the guard observes the relevant identity change.

There is one InlineCache record per call site with one receiver shape, one
prototype shape, and one slot. There is no current public mono/poly/mega state
machine or machine-code patch target. A future prop-mono baseline can reuse the
guard semantics, but must put the guard and fallback entry under the same fork
owned contract. Accessors, proxies, non-cacheable prototype paths, descriptor
changes, and any guard miss must stay on the interpreter fallback until a later
gate explicitly proves them.

tests/boa_inline_cache.rs remains the regression oracle for prototype
reindexing. tests/jit_gate1.rs adds transition/delete/redefine coverage while
running through Omoikane, so the probe does not bypass the browser embedding.

## Probe results: #519

boa_gc already has the pieces needed to reason about a JIT root boundary:

- RootProvider traces a heap-external structure, but its unsafe contract
  requires a stable address and forbids mutable aliasing during collection;
- Vm itself is a root provider and traces the value stack, return value,
  pending exception, native call state, frames, and environments;
- Trace is unsafe and native frame edges must be enumerated explicitly;
- the collector has young/old generations, remembered old-parent writes,
  weak/ephemeron registries, and NoGcScope for bounded graph construction.

The missing pieces are JIT-specific: stack maps or frame descriptors, a
safepoint protocol, register/native-slot enumeration, deopt frame ownership,
and a rule for a JIT frame that calls a suspendable native function or re-enters
the VM. No such metadata or machine stack scan exists in this Boa revision.

The Omoikane probe verifies the observable parts that are safe to verify now:
the global root survives forced collection, a WeakMap key/value remains valid,
an old object receives a new child and survives the next collection, and
Promise/timer callbacks still see the rooted graph. It cannot prove soundness of
a native stack map before a real JIT frame exists. That residual soundness risk
must be a hard stop in Gate 4, not silently assumed away in Gate 1.

The least invasive first design is an engine-owned, stable JIT-frame object
registered with RootProvider, with all live GC edges represented in its
descriptor. Direct scanning of native registers would instead require changes
to the VM/GC boundary and is higher risk. Neither design is production-ready
from the current public API.

## #520: new-engine scope and migration estimate

The machine-checkable workload inventory is in gate1-scope.json. It is kept
in lockstep with tests/js_benchmark/baseline.json by
gate1_scope_covers_every_current_benchmark_shape, so adding a benchmark shape
forces an explicit migration entry. The current 11 workloads cover arithmetic,
calls, property mono/mega, prototype lookup, strings, arrays, allocation, and
GC pressure.

The minimum new-engine semantic subset for Gate 2 is only:

- the parser/compiler needed by those 11 workloads;
- numeric/string primitives, ordinary objects, arrays, functions and closures;
- property descriptors, prototype lookup, shape transitions, mono miss/fallback;
- allocation and a traced heap with a testable root protocol;
- exceptions and a deterministic execution budget;
- a Rust embedding API sufficient to run the probe beside Boa.

It is not enough for Omoikane production. Full migration additionally includes
the inventory's script/host ABI, Promise jobs, modules and dynamic loading,
DOM bootstrap and Web IDL-like wrappers, binary buffers, dialogs, storage,
network-facing APIs, workers/shared workers/worklets, realms, event-loop task
sources, structured clone, CDP-facing values, and all existing test oracles.

| Option | Gate 2 reach | Full Omoikane reach | Main risk / rollback |
| --- | --- | --- | --- |
| Boa fork adapter | smaller initial semantic delta; reuse parser, builtins, GC and existing tests | retain Boa compatibility while moving JIT/VM/GC internals under the fork | private API drift and unsafe GC/JIT integration; pin rollback is straightforward |
| New engine | smallest owned core only; the 11-shape probe is a bounded prototype | large semantic and embedding migration; dual-engine per realm is required | missing web semantics and host ABI; keep Boa as the only production engine until parity |

The estimate is intentionally expressed as gates rather than false precision:
Gate 2 is a bounded prototype; Gate 3 adds one native backend and two
workloads; Gate 4 adds the soundness-critical runtime integration; Gates 5--6
add the second backend and the complete embedding migration. No Gate 1 result
justifies a production cutover or parallel unbounded implementation. The #521
ADR must choose one path and assign the remaining risks to the existing gate
issues.

## Omoikane compatibility gate

This batch is accepted only if the Omoikane runtime itself remains usable. The
minimum local checks are:

~~~bash
/usr/local/cargo/bin/cargo test --test jit_gate1
/usr/local/cargo/bin/cargo test --test boa_inline_cache
/usr/local/cargo/bin/cargo test --test js_benchmark
/usr/local/cargo/bin/cargo test --lib
/usr/local/cargo/bin/cargo build
~~~

CI additionally runs all tests including ignored tests, the build, WPT smoke,
and the benchmark report. No Boa checkout files are modified by these probes.

## Gate 1 disposition

The five investigations are complete when this document and the probes are
reviewed together:

- #516: current pin, fork lineage, embedding consumers, and stale #307
  assumptions are recorded;
- #517: arithmetic bytecode compilation is runnable, and the private VM
  boundary plus required adapter scope is recorded;
- #518: the current shape/IC guard contract and invalidation cases are tested;
- #519: GC/rooting behavior is tested through Omoikane, with native-stack-map
  soundness explicitly left as a Gate 4 stop condition;
- #520: the minimum subset, full migration inventory, rollback strategy, and
  gate-shaped estimate are recorded.

The architecture decision is intentionally deferred to #521. After these five
issues are merged, #307 must receive a five-issue review that states whether
the work remains on the original roadmap, where the current main has diverged,
and whether Gate 2 should start.
