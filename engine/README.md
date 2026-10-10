# JavaScript engine ownership

Omoikane maintains the existing Boa implementation in [`boa/`](boa/). Changes to
the engine and its browser embedding can be reviewed, tested and released in one
Omoikane revision. Development does not depend on merging changes upstream or
publishing another fork revision. Crate names and public APIs are retained.

[`boa-origin.json`](boa-origin.json) records the imported fork revision
`ee98fdacffb38093d9d220c2ac21a4ed6839ce37`, Git tree, and every original tracked
file's hash and mode. All 886 files were copied from verified Git blobs, including
the workspace manifests, fixtures, development tools and original workflows.
The original metadata and copyright notices remain intact. The preserved
[`LICENSE-MIT`](boa/LICENSE-MIT) and [`LICENSE-UNLICENSE`](boa/LICENSE-UNLICENSE)
describe the imported code's licensing.

The root `Cargo.toml` uses path dependencies for `boa_engine` and `boa_gc`.
Their seven related dependencies also resolve inside this tree. The root
`Cargo.lock` fixes the browser dependency graph; `boa/Cargo.lock` independently
fixes the complete engine workspace and its development tools. Keep both under
version control. The initial conversion preserves all 384 root package versions
and only removes the nine former Git source entries.

Run `python3 scripts/check-engine-source.py` from the repository root to check
source retention and the resolved dependency graph. The report lists changes
since import; it does not forbid reviewed engine development. The initial import
also passes `--pristine`, which requires byte-identical files. Do not rewrite the
origin manifest to hide subsequent changes: use normal Omoikane commits to record
them. Engine source identity after import is the Omoikane commit plus this origin.

The nested workspace remains usable directly:

```sh
cd engine/boa
cargo test --locked -p boa_parser -p boa_gc -p boa_engine
cargo test --locked -p boa_engine --features baseline-jit jit:: -- --nocapture
```

When overriding `CARGO_TARGET_DIR`, use separate directories for the root and
nested workspaces (for example, a worktree-specific `-root` and `-boa` suffix).
Their independently locked dependency graphs can differ. Keep each workspace's
build artifacts separate when verifying lockfile updates; the default target
directories already provide this separation. Apply the root repository's
capacity guard to each directory.

The original `boa/.github/workflows/` files are retained as history, but GitHub
does not execute nested workflows. Omoikane's root workflows must exercise the
engine alongside browser compatibility and release gates. Source placement and
successful dependency resolution alone are not completion of #552/#515.

## RegExp exact-position matching

[`regress/`](regress/) preserves the published regress 0.10.5 package, including
its MIT/Apache licenses and tests. [`regress-origin.json`](regress-origin.json)
records the crate archive checksum verified against the root Cargo.lock before
import and the original file hashes. Keep this origin record unchanged when
reviewing local modifications. Both root and Boa workspaces patch regress to
this same local source; the nested lock previously used 0.10.4 and now uses 0.10.5.

The local change adds UTF-16/UCS-2 exact-position matching to the existing
backtracking executor. Boa uses it for sticky regexes, preserving the entire
input for lookbehind, anchors, captures and backreferences. Ordinary global
search continues to use the existing iterator. This avoids scanning the rest
of an input after every failed sticky token match in WebIDL parsers.

The bundled script-extension tables also merge overlapping intervals in Arabic,
Bengali, Cyrillic, Devanagari, Grantha, Gujarati, Gurmukhi and Tamil. This preserves
each table's character membership while satisfying the sorted, disjoint interval
invariant required by the parser and its binary searches. Regression tests cover
the merged ranges through both property names and script aliases, including
complement matches. The original archive hashes remain unchanged in the origin
record; these corrections are local modifications.

In the Boa workspace, validate regress and the embedding with a separate target:

```sh
cargo test --locked -p boa_parser -p boa_gc -p boa_engine -p regress
cargo test --locked -p boa_engine builtins::regexp::tests
```

The regular parser/GC/engine and browser test suites remain required. Adding this
API does not change the production interpreter or enable a JIT feature.

## Native builtins and Intl validation

[Issue #1295](https://github.com/ieee0824/omoikane/issues/1295) adds native
builtins and ICU-backed Intl services. The bundled provider retains its ICU
component datasets and adds shared typed CLDR data plus a standard 77-system
digits dataset. See [provider coverage and licensing](boa/core/icu_provider/README.md)
and [pinned data regeneration](boa/core/icu_provider/scripts/intl-cldr/README.md).
Data coverage does not imply complete ECMA-402 compatibility.

The following results are historical snapshots from 2026-10-09, before the final
source review and example repairs. Current validation status and evidence are
recorded in [Issue #1295](https://github.com/ieee0824/omoikane/issues/1295).

The historical locked release `boa_engine` contract run used
`--no-default-features --features intl_bundled,experimental`:
**182 passed / 0 failed / 0 ignored**, comprising 152 Intl contracts (including
67 NumberFormat contracts), 26 Temporal contracts and four native receiver/GC
contracts. A separate run of the same 146-file source snapshot, with defaults
enabled and `--features annex-b,intl_bundled,experimental,baseline-jit`, passed
80 baseline-JIT contracts, 152 Intl contracts and four receiver contracts.
These are scoped results for that source snapshot, not the final nested-engine
or browser validation.

The historical rebuilt tester at source-set SHA256
`5a951af37edf617a17cf6f6ead35522c253f135874d5ccbb06703746e2678263`
ran **26 selected original API directories**, with an unfiltered config, at
Test262 revision `2e0a56762801e275a9fdf96dc49d90ba0cddcf63`:
**6,415 cases, 6,070 passed, 345 failed, zero ignored and zero panics**. This
records a bounded selection, not a full Test262 run. Remaining failures include:

| Area | Result | Boundary |
| --- | --- | --- |
| NumberFormat | 250 pass / 1 fail | Unicode 17 `tols` digits are absent from pinned CLDR 47 / Unicode 16 data. |
| DateTimeFormat | 135 pass / 110 fail | 57 range/range-to-parts failures; 53 scalar, parts, timezone, legacy-constructor and resolution failures, including Temporal inputs. |
| DurationFormat | 109 pass / 1 fail | The fixture calls the unavailable `Intl.supportedValuesOf` prerequisite. |
| Iterator | 423 pass / 231 fail | All 394 ES2025 constructor/from/helper cases pass; the 231 failures target `join`, `chunks`, `windows`, `includes`, `concat`, `zip` and `zipKeyed`, outside that ES2025 scope. |
| Uint8Array setters | 2 fail | Both fixtures require the unavailable immutable-ArrayBuffer argument factory. |

At that historical snapshot, Temporal's selected original subtree passed all
4,605 cases. The DateTimeFormat failures were implementation gaps in that
snapshot; the extra Iterator APIs were recorded separately without being
excluded or relabeled as passing. The two immutable-buffer fixtures failed
during harness setup, before a setter could run.

Local evidence is retained under `.artifacts/p1/`: the
`final-builtins-native-20261009T165635421274Z` and
`final-builtins-jit-20261009T170506916456Z` directories contain source hashes,
commands and logs; `test262-original-builtins-final-20261009T170116304087Z/execution.json`
records the actual suite pin, per-directory results and tester SHA256
`8ae9248779fc16c3936aad8000c4d18caf38916e5317273da6a8e071ad3183eb`.

A later historical root-default snapshot, recorded on 2026-10-09 before the
example and documentation repairs, passed 3,913 tests with zero failures or
ignored tests. This covered all default lib/bin/integration/example/doc targets
through 121 partitions; a single unpartitioned full-test command was not run
because its outputs exceeded the sandbox capacity. Its owned source inventory,
commands and results are recorded in `native-final-product-20261009T2329/inputs.json`
under the owned-cache evidence directory. The same snapshot passed the exact
Firefox comparison of 18 API-availability checks, 10 semantic checks and 33
locale outputs. These results do not certify a later source snapshot or every
method on those APIs.

The matching-feature whole-runtime probe grew from 36,453,968 to 66,711,896 bytes.
Its runtime-initialization medians were 154.548 and 153.461 ms, with overlapping
sample ranges. This measures the combined native builtins, enabled features,
bootstrap changes and data, rather than a provider-only size difference.

The historical 14-marker provider comparison completed on 2026-10-10 UTC.
Both variants used the same
source, compiler, default features and release settings, differing only in the
14-marker provider blob. Other ICU component and calendar datasets and all 77
digits systems retained their full data. The full-locale probe was 66,712,104
bytes and the filtered en-US/ja-JP/de-DE probe was 52,818,480 bytes, a whole-probe
reduction of 13,893,624 bytes. Both variants passed the same 18/10/33 Firefox
checks, the 77-system digit contracts and retained-payload byte comparisons.

Fresh-process A-B-B-A sampling used three blocks, six measured samples per
variant and one warmup per variant. Runtime-initialization medians were
161.354 / 159.507 ms in the initialization-only condition and 158.754 / 163.392 ms
with first Intl formatting enabled. The sample ranges overlapped, so no clear
startup improvement was established. The first three-locale Intl formats,
including JS evaluation, had medians of 3.293 / 1.850 ms; this is a separate
measurement from runtime initialization.

The filtered provider lost NumberFormat and DurationFormat supported-locales
coverage and existing localized number, currency, unit and duration output for
ar-EG, ko-KR, fr-FR, ru-RU, zh-CN and th-TH, replacing them with generic fallback.
The production `intl_bundled` default therefore retains the full 739-locale typed
extension. This comparison is not a filter of every Intl component to three
locales and does not promise full ECMA-402 compatibility.

Provider-comparison evidence is
`provider-comparison-20261009T2343/completion.json` (SHA256
`ea4aecb05e2492b8081bd5a259d6366dd9371843cc9d792441faeb210aa2a71b`) under the
owned-cache evidence directory. Final-source nested/JIT/Test262, final root
validation and CI/merge remain pending at this documentation snapshot. See
[Issue #1295](https://github.com/ieee0824/omoikane/issues/1295) for the latest actual
results; the historical results above are not silently promoted to current
source validation.

The current schema adds three markers for native DateTimeFormat ranges,
RelativeTimeFormat and DisplayNames: the existing fourteen markers followed by
`OmoikaneCalendarIntervalPatternsV1`, `OmoikaneRelativeTimePatternsV1` and
`OmoikaneDisplayNamesV1`. The generator and export path select the pinned full
739-locale source, including all 17 calendar attributes for date intervals;
independent verification checks those source records.

The 2026-10-10 follow-up JSON validation passed all 41 generator tests with no
skips and reproduced all 15 datasets plus provenance and license in two
byte-identical runs. Independent verification checked 18,604 hashed input
files, all 12,563 calendar interval payloads, 393,932 interval skeletons,
911,403 raw interval patterns, 106,360 relative-time source records and 652,702
display-name source records. Both regenerated directories passed the
fifteen-dataset pin/hash validator.

Currency display names and symbols remain in the existing per-code
`OmoikaneCurrencyTextV1` marker instead of being copied into every locale's
DisplayNames map. That marker serializes raw CLDR standard and narrow symbols;
NumberFormat applies its existing narrow-to-standard-to-code fallback, while
DisplayNames uses the display name for `long` and the raw symbols for `short`
and `narrow`. A locked release exporter run produced two byte-identical
17-marker postcards of **25,302,285 bytes**, SHA256
`fa01fc293d3911e5ec1a19211961252197a724d0fd4d67d6644a63f66de52ec5`.
Typed comparison with the preceding blob kept every marker's identifier set;
all 15 unrelated markers were payload-identical, while the currency-text and
DisplayNames payloads changed by design.
Generation and export evidence is retained under
`native-existing-intl-generated-dn-dedup-final-20261010T042600Z` and
`native-existing-intl-dn-compact-final-20261010T050000Z` in the owned-cache
evidence directory.

The subsequent exporter uses the shared 45 sanctioned-unit mapping to omit
CLDR simple units that the ECMAScript API cannot request. This changes the
unit-pattern marker from 410,811 to 101,982 identifiers while retaining all
739 locales, supported compound units and all 77 numeric numbering systems.
The currently installed seventeen-marker blob is **19,117,677 bytes**, SHA256
`1bb37c3ffa1d9a8a72c7c7031fd0ece81ec1b6a777edcfa39d035414d6db16e0`.
Two exports from the current source were byte-identical to that installed blob;
proof is retained in the owned-cache evidence directory under
`p1-1331-text-sink-20261010/provider-regeneration-current/completion.json`.
The 25,302,285-byte result above records the preceding exporter snapshot.

These are generation, serialized-data and focused Intl results. Japanese
interval selection, DisplayNames script/currency widths, NumberFormat currency
fallback and all provider extension tests passed against the installed blob.
Broader final-source, nested/JIT/Test262 and root validation and new
whole-executable size/startup measurements are separate gates. The postcard
size is not an executable-size measurement. See Issue #1295 for subsequent
results; the historical fourteen-marker measurements above do not measure the
three additional markers.


### Current publication candidate (2026-10-10)

The candidate now includes main merge `461a4b4c` (PR #1332) without changing
any validated source bytes. On this source, `CI=1 cargo test --locked
--no-fail-fast -- --include-ignored` completed with **113 groups, 3,933 passed,
zero failed and zero ignored**, using required WPT revision
`dc97e7bed3096ac9e0e591ab5fa22e7fb8844ead`. `cargo build --locked
--message-format=json` also completed successfully. The earlier wrapper's
post-command bookkeeping failure is preserved separately and is not reused as
this successful run.

Nested contracts passed 1,224 native engine unit tests, 1,304 with baseline JIT,
and 1,001 in the default/no-Intl configuration; each run also passed ten
integration tests and 120 doctests, with three doctests ignored. The string
crate passed 38 unit tests and two doctests. These are local candidate results;
publication CI and merge still need completion.

The rebuilt tester ran all **6,552 cases in the selected 28 directories** at the
unchanged Test262 pin: **6,231 passed, 321 failed, zero ignored and zero panics**.
Temporal passed 4,605/4,605, Segmenter 79/79, RelativeTimeFormat 80/80 and
DisplayNames 57/57. Remaining failures are DateTimeFormat (82), post-ES2025
Iterator proposals (231), NumberFormat Unicode-17 `tols` data (1),
DurationFormat (5), and immutable-ArrayBuffer fixture prerequisites (2).
Four DurationFormat failures expect an older Temporal integration contract:
current ECMA-402 ToDurationRecord rejects strings and reads duration properties.
Fixtures and ignores were not changed to hide these results.

The unchanged Firefox payload matched all 18 availability checks, ten semantic
checks and 33 locale outputs for `en-US`, `ja-JP` and `de-DE`. The extended
payload passed 60 semantic assertions and twelve descending-range assertions,
but exact string comparison retained seven differences: four English interval
thin-space literals and three compound dialect language names. The pinned CLDR
47 interval patterns and language labels explain the native values; locale
data fields are implementation-defined. This extended comparison is **not**
reported as whole-output equality, and its raw failing comparison is retained.

Same-profile standalone **dev** probes measured binary size
57,579,080 -> 93,921,976 bytes (+36,342,896). Fresh-process initialization ABBA
(three blocks, one warmup per variant, six retained samples each) had medians
173,671.5 -> 170,565 microseconds with overlapping sample ranges. This does not
establish a startup improvement. Both inputs include PR #1332; the difference
covers native features, builtins, data and bootstrap, rather than provider-only
causality. These are standalone probe ELFs, not release or GUI executable sizes.

The parts-to-text sink comparison previously recorded in Issue #1331 measures
that isolated implementation stage. A subsequent final String conversion stage
now writes non-ASCII UTF-8 directly into an exact-capacity UTF-16 JsString
builder, eliminating the intermediate Vec and full UTF-16 copy. For mixed BMP
and astral input of 98,304 / 196,608 UTF-16 units, same-profile allocator probes
showed calls 3 -> 1, requested bytes 688,158 -> 196,632 / 1,376,286 -> 393,240,
and peak additional bytes 524,316 -> 196,632 / 1,048,604 -> 393,240. Input,
warmup and equality checks were outside measurement. These are System allocator
requests, not RSS or browser-wide timing improvements.

Evidence is preserved under `/workspace/.artifacts/p1-owned-cache/` in
`p1-1295-publish-root-20261010`, `p1-1295-test262-current-20261010`,
`p1-1295-publish-product-20261010`, `p1-1331-text-sink-20261010` and
`p1-jsstring-direct-20261010`. Source freezes, real Cargo artifacts, binaries,
reference payloads, exact mismatches and raw samples are retained.

### CI follow-up validation

Inline cache installation now releases the receiver borrow before allocating
weak shape handles, while retaining an owned shape root. A real abandoned Map
iterator reproduces the former finalizer borrow panic before the correction;
the corrected cache contract passes. The JIT full library run passed 3,098 tests.
The Unicode script-extension interval correction was then verified with 520
regress tests and doctests, with one existing doctest ignored, and the original
Grantha Test262 fixture passed in both normal and strict mode.

After both runtime corrections, the required-WPT root command again passed
113 groups and 3,933 tests, with zero failures or ignored tests. The final dev
library build and committed Rust formatting checks also passed. The rebuilt
standalone probe matched the retained Firefox observation in all 18 availability
checks, ten semantic checks and 33 outputs across the three principal locales.

This final standalone dev probe measured **93,908,264 bytes**, compared with
57,579,080 bytes for the same-profile baseline, a 36,329,184-byte increase. Three
fresh-process ABBA blocks retained six initialization samples per variant:
medians were 397,386 -> 374,713.5 microseconds, with ranges
363,593-448,805 -> 273,078-461,807 microseconds. Other Test262 runs were active
during this measurement. The overlapping ranges do not establish a startup
improvement, and this is neither provider-only causality nor release/GUI size.
The earlier probe measurements above remain historical observations.

The CI Test262 pin now uses `2e0a56762801e275a9fdf96dc49d90ba0cddcf63`, matching
the current Temporal YearMonth contract. Both engines share only the tester's
feature-edition metadata adapter; the retained engine, dependency lock and ignore
configuration keep their original bytes. The runner rejects missing or edited
tracked fixtures and records the complete test/harness inventory. The local
checkout contains 53,974 such files; this is an input count, not a passed-case
count. Complete-suite comparison and publication CI remain pending here.

Follow-up evidence is retained in `pr1333-ci-fixes-20261010` and
`p1-1295-publish-product-final-20261010` beneath the evidence root above.

### Resizable TypedArray follow-up

The complete pinned suite exposed five existing TypedArray panic cases in both
the retained engine and the maintained engine. `filter` and `with` now propagate
BigInt conversion errors when source resizing or detachment leaves an element
undefined. `slice` avoids accessing the source byte offset when the copy count
becomes zero after fresh bounds validation; the species-created result keeps
its originally requested length. Three regression tests failed before these
corrections and passed afterward. The five unchanged Test262 fixtures passed
in both normal and strict mode, for ten successful cases.

The immutable reference engine's panics are recorded as panics, rather than
rewriting its engine or converting those results to failures or ignores. A
successful comparison requires every reference panic to become a maintained
engine PASS. The maintained engine must remain panic-free, and removed cases,
regressions, newly failing cases and platform disagreements remain failures.
Nineteen harness tests check these conditions. After this correction, the
required-WPT full root run passed 113 groups and 3,933 tests, with zero failures
or ignored tests. Both the package build and dev library JSON build succeeded.
The rebuilt probe again matched all 18 availability checks, ten semantic
checks and 33 principal-locale outputs against the retained Firefox observation.
The extended comparison passed its 60 semantic and 12 descending-range checks;
all seven previously recorded exact string differences remain unchanged.

The latest standalone dev probe is **93,908,032 bytes**, a 36,328,952-byte
increase over the same-profile 57,579,080-byte baseline. Three fresh-process
ABBA blocks retained six samples per variant: initialization medians were
574,311 -> 592,234 microseconds, with ranges 545,317-744,547 -> 435,326-643,777.
Test262 and a feature-contract build were active during the measurement. These
overlapping ranges do not establish a startup improvement or regression.
The earlier measurements remain historical observations. Latest evidence is
in `p1-1295-publish-product-typedarray-20261010` beneath the evidence root above.
Complete-suite comparison and publication CI for this correction remain pending.
