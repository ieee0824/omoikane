# Historical benchmark inputs

`shapes-v1.js` and `baseline-v7.json` preserve the exact inputs before #658.
The old array body resets after 1025 pushes while its index wraps after 1024,
so its result is NaN. The shared numeric sink then also becomes NaN. These
files support reproduction of historical measurements, including #659;
they are not the current performance baseline.

Gate 2/3 snapshots and `docs/jit/measurements/gc-pointer-sets-2026-09-10.json`
retain their original measurements and inputs. Do not compare those timings
with fixture v2 as evidence of a speedup. Fixture v2 changes the array body
and validates every pass outside the timed interval. Its baseline and new
measurements carry the complete fixture SHA-256, and the current Rust runner
refuses to calculate ratios when the fixture identities differ.
