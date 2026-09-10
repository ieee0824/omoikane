# Gate 5 native distribution-target checks

Issue: #544. Requires the ARM64 foundation, arithmetic and properties in #541–#543.

Previously release.yml cross-built Linux ARM64 and only ran cargo test on Linux
x86_64. Native JIT targets now uses four actual hosts: ubuntu-24.04 (x86_64),
ubuntu-24.04-arm, macos-15-intel and macos-14 (ARM64). The Rust host triple and
machine architecture must match the distribution target. No QEMU is required.

The reusable jit-native.yml runs for PRs, main, manual requests and release tags.
Release publication depends on all four native checks as well as the existing
packaging jobs. Package names and release publishing remain unchanged. The native
matrix owns the existing code-memory, lowering, arithmetic/property, stack-map,
runtime-call, GC-root and deopt/exception/timeout integration contracts that used
to run only in the general Linux job. The full Gate 4 stress gate remains separate.

Run `bash scripts/check-jit-native.sh` to reproduce one host's check. The script
resolves a lockfile when absent and stores the exact dependency set. It requires
all eight contract binaries to run nonzero tests and verifies the expected native
property, GC, exception and timeout cases are present. No platform fallback or
zero-test result counts as success on a distribution target.

The first test writes a policy classification before executing the corpus:
unsupported platform, OS permission denial, other OS/construction failure, or
execution confirmed by a fixed stub. Unsupported or denied hosts fail the gate;
they are not skipped. Build or host-verification failures retain their logs and
must not be interpreted as a JIT policy diagnosis. On macOS the supported host
signing/allocator constraints in gate3-code-memory.md still apply.

Artifacts named jit-native-TARGET contain environment/revision, Cargo.lock,
contract logs, the overall gate.json decision, native-report.json, reproducible source seeds, fixed/generated code
dumps and matched JIT on/off measurements. Four fixed seed values parameterize
#305 arithmetic and prop-mono workloads. Each result must equal an independent
Rust checksum; enabled runs must record native entries and property guard hits.
Timings exclude warmup, are observations for that host, and have no speed threshold.
The native binary also reuses eight seeds from the existing forced-GC, deopt,
exception and timeout corpus, including child-failure artifact preservation.
Child-process source and code snapshots are copied into the native artifacts on
both success and failure. These checks do not replace the separately profiled
64-or-more-seed, ten-minute Gate 4 requirement.
