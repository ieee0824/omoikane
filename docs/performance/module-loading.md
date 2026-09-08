# ES module graph loading (#622)

Module imports now use four lazily created HTTP workers per runtime. Each
worker retains its connection pool; all four share one cookie jar. Only HTTP
requests and responses cross threads. Boa parses and evaluates on its owning
thread, retaining source-order evaluation and one module record per Document
and URL. Simultaneous imports share their download; different Documents retain
separate module records and CSP contexts. Unused dynamic imports are not fetched.

Retiring an iframe Document cancels its queued imports. Runtime teardown does
not join transports: at most four active requests finish under the existing HTTP
timeouts without retaining Boa objects. No new dependency or Boa revision is
required. Inline module CSP lookup retains the script fragment in its root
path, so HTTP URL normalization cannot silently lose that policy.

## Measurement on 2026-09-08

Five fresh processes before and five after, aarch64 Linux, Rust 1.98.0, dev build
with debug information disabled, Firefox user agent, 1366 × 900 viewport. The
before binary is commit `87653bd3d9e3b52db027245e47ca5fde6f4804a4` (same tree as
main merge `55b3f7c664b41cec631c42492cae5c165ac88889`). Results are a live-site
snapshot; network conditions and future site releases can change them.

| Median | Before | After |
| --- | ---: | ---: |
| Navigation + render | 30.973 s | 24.576 s |
| Navigation | 12.072 s | 5.537 s |
| Render | 19.148 s | 18.974 s |
| Entry module load/link/evaluate | 9.921 s | 3.581 s |
| Sum of dependency HTTP durations | 6.114 s | 6.643 s |
| Sum of dependency parse durations | 4.736 s | 4.038 s |

Total time improved by 20.7%; entry module load/link/evaluate by 63.9%.
All ten runs loaded 452 dependency modules / 4,852,493 bytes and produced the
login page with its X logo and QR code. Total time ranges were 30.930–31.692 s
before and 24.146–24.731 s after. The 668,184-byte Castle bundle remains the
largest parse cost (roughly 2 seconds); parsing is still synchronous.

The HTTP durations overlap after this change, so their sum is **not** wall time
and must not be added to parse time to estimate navigation duration. Entry
load/link/evaluate includes static dependency loading, while dependency totals
also include dynamic imports triggered later by the page. Render largely
contains timer processing and was not the target of this change. Parse-time
variation does not represent a new parser optimization.

Raw timings: [module-loading-2026-09-08.json](module-loading-2026-09-08.json).

## Reproduce

Build each revision separately and preserve each binary before rebuilding:

```sh
CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 cargo build --example screenshot
```

For each binary, run five times in fresh processes without simultaneous builds:

```sh
OMOIKANE_LOG_SCRIPTS=1 /path/to/screenshot --firefox-user-agent \
  --dump-html run.html https://x.com/ run.png 1366 900 > run.log 2>&1
```

Keep a separate log, HTML and PNG for each run. Compare the screenshot log's
`navigate` + `render` durations and the deferred entry script's `execute_ms`.
Dependency log fields distinguish actual HTTP time (`fetch_ms`), parsing
(`parse_ms`), and time from queueing to resuming on the JS thread (`wait_ms`,
including queue and scheduling delay). Inspect screenshots and check the login
markup rather than treating a fast error page as a successful result.

The deterministic `module_loading` tests use a local server whose first
response waits for another active request. The serial baseline fails the
overlap assertion with peak concurrency one; the parallel loader passes while
preserving module identity, evaluation order, shared cookies, lazy dynamic
imports, iframe isolation, CSP and private-address restrictions.
