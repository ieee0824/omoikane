# Boa dependency advisory audit (2026-10-07)

Tracking: #1307. Base revision: `c37c50e9`. Tool: cargo-deny 0.20.2.
The root and `engine/boa` are independent workspaces with independent locks.
A package in a lockfile is not proof that the default browser executes it.
Reachability below comes from cargo-deny's reverse graphs and locked normal /
all-feature `boa_engine` trees, rather than package names alone.

| Package | Advisory suffix (RUSTSEC-) | Before → after | Reachable scope |
| --- | --- | --- | --- |
| bytes | 2026-0007 | 1.10.1 → 1.12.1 | Engine Temporal feature via timezone_provider; runtime HTTP, examples, ICU tool |
| crossbeam-epoch | 2026-0204 | 0.9.18 → 0.9.21 | Engine dev benchmark via criterion/rayon; tester and ICU generator |
| h2 | 2026-0258 | 0.4.12 → 0.4.20 | Runtime optional reqwest backend, CLI/tester/examples using that backend |
| lz4_flex | 2026-0041 | 0.11.5 → 0.11.6 | Engine/macros embedded_lz4 feature |
| rand | 2026-0097 | 0.9.2 → 0.9.5 | Default engine, tests/examples/tools using engine |
| rustls | 2026-0285 | 0.23.31 → 0.23.45 | ICU generator networking via ureq; root HTTPS is a separate lock (#1305) |
| rustls-webpki | 2026-0049/0098/0099/0104 | 0.103.4 → 0.103.15 | ICU generator via rustls |
| tar | 2026-0067/0068 | 0.4.44 → 0.4.46 | ICU generator data download/unpack |
| thin-vec | 2026-0103 | 0.2.14 → 0.2.20 | Default engine and GC; all downstream consumers |
| time | 2026-0009 | 0.3.44 → 0.3.55 | Default engine; all downstream consumers |

Normal root graphs already use patched bytes, crossbeam-epoch, rand,
rustls-webpki, thin-vec and time. They contain no h2, lz4_flex or tar.
The root rustls was updated from 0.23.44 to 0.23.45 by merged PR #1312 (#1305), so the
new audit CI does not introduce a failing known-vulnerability gate.
The updated time 0.3.55 declares Rust 1.88.0, matching Boa's declared MSRV.

## Remaining maintenance advisories

These have no patched version within the current dependency line. They are
maintenance notices, not the vulnerability/unsound findings updated above.
They remain visible in the unconfigured audit; no finding is silently ignored.

| Advisory | Scope | Reason retained / mitigation | Review by |
| --- | --- | --- | --- |
| RUSTSEC-2024-0384 (instant) | boa_examples → isahc → futures-lite 1 → fastrand 1 | Transitive example-only dependency; default browser does not use this backend. Review backend upgrade/replacement before expanding use. | 2027-01-07 |
| RUSTSEC-2024-0436 (paste) | Engine, GC/string/ICU compile-time macros | No fixed paste release. Replacement needs macro/source compatibility review. It is a build-time proc macro; retain pinned version and advisory monitoring. | 2027-01-07 |
| RUSTSEC-2025-0134 (rustls-pemfile) | gen-icu4x-data → icu_provider_source → ureq | Generator-only dependency, not default browser transport. Update the ureq/ICU dependency line with generator verification before removing. | 2027-01-07 |

Yanked spin 0.9.8 (futures-buffered) and 0.9.8/wasmi dependency paths remain
reported separately from security advisories. Moving the relevant dependency
lines requires their own compatibility tests; yanked status does not prove a
vulnerability.

## Reproduction and evidence

```sh
cargo deny --manifest-path engine/boa/Cargo.toml --all-features --locked check advisories
cargo tree --locked --manifest-path engine/boa/Cargo.toml -p boa_engine -e normal
cargo tree --locked --manifest-path engine/boa/Cargo.toml -p boa_engine -e normal --all-features
```

The unconfigured post-update Boa audit exits 1 for the three maintenance advisories;
no vulnerability or unsound finding remains. Local full output is preserved in
`.artifacts/p2/boa-advisories-after.log`. These observations do not substitute
for the engine tests and root tests/build required before merging.

With the shared `deny.toml` maintenance policy, both locked audits exit 0.
The root additionally retains RUSTSEC-2026-0206 (rustybuzz, browser shaping) and
RUSTSEC-2026-0192 (ttf-parser, browser font parsing); replacement requires shaping,
font and rendering comparisons. Both are due for review on 2027-01-07.
Exception-expiry validation and its boundary/error tests pass locally.

The nested `boa_parser`, `boa_gc` and `boa_engine` locked tests completed:
1462 passed, zero failed, four ignored, including doctests. Debug information
was disabled with `CARGO_PROFILE_DEV_DEBUG=0` / `CARGO_PROFILE_TEST_DEBUG=0`
to keep the isolated target within the local disk budget; assertions and test
coverage were retained. Root validation is recorded below; CI remains required before merging.

The nested locked build for `boa_parser`, `boa_gc`, and `boa_engine` also passed.

The baseline-JIT contract command from `engine/README.md` passed 76 tests,
with zero failures. `python3 scripts/check-engine-source.py` also passed with
no missing crates or invalid engine source paths.

The root locked full test run on `c37c50e9` plus this dependency-audit change
passed 3437 tests, zero failed, 15 ignored (`RUST_TEST_THREADS=2`, debug
information disabled). The optional WPT checkout was not configured in that
run; this result does not claim to execute the pinned external WPT suite.
The root and nested workspaces now use separate build artifacts. A previous
shared-target attempt produced incompatible Boa GC crate identities; after
quarantining that target, the clean root run passed. No test assertion,
timeout, or ignored marker was changed to obtain this result.

The clean root `cargo build --locked` also passed (25.59 seconds).
