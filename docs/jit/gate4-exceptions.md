# Gate 4-5: generated-frame exceptions

Issue #538 connects the Boa fork's verified exception metadata to live VM
execution. With `baseline-jit` enabled on x86_64 or ARM64 Linux/macOS, hot functions use
generated runtime-helper sites for calls, construction, throw/rethrow, object,
array and function allocation, and property access. Arithmetic loops retain
their existing scalar emitter. Other operations use interpreter dispatch.

Each helper site has a native entry, an exact return-PC safepoint, and the
verified bytecode catch/finally ranges and source locations. The generated stub
calls the existing opcode helper, which may allocate, throw, or re-enter script.
The recorded return offset is 9 bytes on x86_64 and 16 bytes on ARM64; exception
lookup uses each site's actual offset.
This is a runtime-call lowering; it does not compile a whole function into
machine code or claim to accelerate these helpers.

The helper's operands and results stay in the registered VM stack. The native
stub holds no JavaScript values, so its stack map has no additional roots.
Allocating helpers retain their own temporary root contracts. Code ownership is
retained by each active invocation even when recursive execution evicts a cache
entry.

On an exception, the unwind plan is applied to the current VM frame's handler
PC and environment depth. Already performed side effects and pending call state
are not replayed. An intervening interpreter frame gets its own handler search
before propagation reaches an older generated caller. Finally and rethrow use
the ordinary VM continuation machinery. Uncatchable runtime-limit errors keep
their existing host-boundary behavior.

Generated source PCs update the existing JavaScript/native shadow stack rather
than adding duplicate frames. Logical generated frames are unwound in inner to
outer order; physical native frames return through the fixed ABI. Rust panics
are caught inside the trampoline and resume unwinding only after the native
entry has returned. This does not use OS native exception unwinding.

The fixed allocation-helper boundary also applies its cleanup plan to every
popped frame and matching spill-root record. Its deterministic allocation budget
provides the OOM-equivalent regression: exhausted allocations use the same
catchable `RangeError` channel as native errors. This does not attempt to recover
from process-wide allocator aborts.

Verification commands:

```sh
cargo test --features baseline-jit --test jit_deopt exceptions
# In the pinned Boa fork:
cargo test -p boa_engine --features baseline-jit jit::
```

The tests compare JIT on/off exception values, object identity, nested handler
selection, lexical environments, finally effects, native errors and host stack
traces. They also force collection during nested generated calls, assert frame
and root cleanup, and exercise DOM bindings through Omoikane. Generated-entry
and unwind counters are asserted on supported targets; unsupported targets
exercise the same observable semantics through the interpreter.
