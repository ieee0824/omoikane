# Gate 3 performance gate

Issue #533 decides whether the measured x86-64 baseline tier justifies Gate 4.
The production default remains the interpreter; `baseline-jit` is still an
explicit Cargo feature.

## Protocol

Both modes execute `tests/js_benchmark/shapes.js` unchanged: 11 workloads, the
checked-in iteration counts, four timed passes per workload, and five fresh
`JsRuntime` instances per set. JIT-off and JIT-on differ only by the
`baseline-jit` feature. Linux measurements pin the process to CPU 0. The
SpiderMonkey comparison is baseline v7's Firefox 153.0.1 minimum of five fresh
processes, recorded by the #317 procedure.

Two independent Linux sets were recorded. A temporary branch-only Actions
matrix also measured the same source on Linux and Intel macOS; its workflow was
removed after downloading the reports so it does not add recurring CI cost.
Cross-runner absolute numbers are not divided by the local Firefox reference;
the matrix is used for matched JIT-on/off and OS parity only.

## Linux x86-64 result

All values are five-run medians in ns/op.

| Shape | JIT off set 1 | JIT on set 1 | Speedup | JIT off set 2 | JIT on set 2 | Speedup | Firefox JIT | Omo/Firefox |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| arith | 307.16 | 16.13 | 19.0x | 302.99 | 16.34 | 18.5x | 3.54 | 4.6x |
| prop-mono | 800.64 | 8.60 | 93.1x | 838.54 | 9.00 | 93.2x | 1.82 | 4.9x |

The two sets give the same decision. `prop-mono` is below 5x from the corrected
SpiderMonkey JIT reference, comfortably inside the required one-order-of-
magnitude boundary.

The other nine workloads remain interpreter fallbacks. Their matched JIT-on
versus JIT-off medians ranged from 7.3% faster to 3.2% slower in set 1 and 8.4%
faster to 1.3% slower in set 2, with no systematic fallback regression.

## Linux/macOS x86-64 parity

The temporary four-job matrix completed successfully in Actions run
`30851506309`. Absolute numbers are runner-specific; the meaningful comparison
is JIT-on against JIT-off on the same runner.

| Runner | Shape | JIT off | JIT on | Speedup |
| --- | --- | ---: | ---: | ---: |
| Ubuntu x86-64 | arith | 257.78 | 17.42 | 14.8x |
| Ubuntu x86-64 | prop-mono | 800.02 | 21.48 | 37.2x |
| macOS Intel | arith | 699.29 | 20.09 | 34.8x |
| macOS Intel | prop-mono | 1,821.23 | 12.17 | 149.6x |

Both operating systems compiled and entered the same two native sites with the
same 1,703 bytes per runtime and zero guard miss/bailout. Intel macOS compile
time was higher (1,589.66 ms versus Ubuntu's 375.80 ms across five runtimes),
but this remained one-time compilation and did not prevent either workload from
showing a larger matched speedup than Linux local. Gate 3 therefore has no
x86-64 OS parity blocker.

## Compile and guard diagnostics

Each fresh runtime submitted 19 loop sites: two compiled and 17 were rejected
as outside the Gate 3 subset. Each runtime emitted 1,703 executable bytes,
entered native code eight times, and recorded four property guard hits with no
guard miss, general bailout, or property bailout. Across five runtimes:

| Set | Compile time | Generated code | Requests / success / rejection | Entries | Property hit / miss / bailout |
| --- | ---: | ---: | ---: | ---: | ---: |
| Linux set 1 | 323.82 ms | 8,515 B | 95 / 10 / 85 | 40 | 20 / 0 / 0 |
| Linux set 2 | 333.53 ms | 8,515 B | 95 / 10 / 85 | 40 | 20 / 0 / 0 |
| Ubuntu Actions | 375.80 ms | 8,515 B | 95 / 10 / 85 | 40 | 20 / 0 / 0 |
| macOS Intel Actions | 1,589.66 ms | 8,515 B | 95 / 10 / 85 | 40 | 20 / 0 / 0 |

Compile time is about 65--67 ms per fresh runtime and is paid once per code
site, not on native entries. The rejected-site total is observable for future
scope work but was only about 0.4% of the complete 11-shape run and did not cause
a matched fallback regression.

The checked-in raw reports preserve every timing and diagnostic sample:

- `gate3-performance-linux-local-jit-{off,on}.json`
- `gate3-performance-ubuntu-jit-{off,on}.json`
- `gate3-performance-macos-intel-jit-{off,on}.json`

## Decision

Gate 3 is **GO**. The primary `prop-mono` condition passes in both independent
sets, arithmetic also has a large repeatable speedup, and exact-head semantic,
full-suite, WPT smoke, Linux JIT, and Intel macOS JIT execution checks remain
green. Proceed to Gate 4 without enabling the JIT by default. Gate 4's stack
maps, safepoints, GC roots, deopt, exceptions, and interrupt integration remain
hard stop conditions.
