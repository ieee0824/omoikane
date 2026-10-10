# Native Intl CLDR extension data

The Python extractor reads pinned upstream CLDR sources and writes canonical
JSON for the generator. Runtime Intl does not read these JSON files. The
independent `boa_intl_data` example turns them into typed ICU4X 2.0 postcard
payloads consumed through `boa_icu_provider::buffer()`.

The current schema has **17 typed markers and 15 JSON datasets**. The original
fourteen markers and twelve datasets are retained, followed by date-time
interval, relative-time and display-name data. The existing ICU component
providers keep their own coverage; this extension uses the pinned full
739-locale source and does not promise complete ECMA-402 compatibility.

Sources and license:

| Source | Pin | SHA256 |
| --- | --- | --- |
| [Official CLDR JSON full ZIP](https://github.com/unicode-org/cldr-json/releases/download/47.0.0/cldr-47.0.0-json-full.zip) | tag `47.0.0`, commit `16f6b8578ba5fe98959034706f337674f816fc3f` | `bbb9a9aac2dfc534bd18288678a5984023d11d22f712f3c33425f3214bd1def6` |
| [CLDR root XML](https://raw.githubusercontent.com/unicode-org/cldr/2ef784e3a4168bc2a43cd1b5b9839b6636f5899c/common/main/root.xml) | `release-47`, tag object `adbaf76bf62572cbfe2044564c40ed44beb00f8a`, commit `2ef784e3a4168bc2a43cd1b5b9839b6636f5899c` | `1ac705812e9e186b8d904813a4989d160729cc4d217849ad3736257da10f69b0` |

The root XML supplies all 77 numeric numbering systems, including aliases to
Latin symbol/pattern data. The separate no-numberSystem default alias is not a
numeric system. `arabext` has root time separator `٫`; `arab` and `latn` use `:`.
The Unicode license extracted unchanged from `cldr-core/LICENSE` is retained as
`data/omoikane_intl.LICENSE.txt` and in each generated directory.

From the repository root, choose new paths for every generated artifact:

```sh
python3 engine/boa/core/icu_provider/scripts/intl-cldr/generate.py \
  --source-zip /tmp/cldr-47.0.0-json-full.zip \
  --source-root-xml /tmp/cldr-root-47.xml \
  --output-dir /tmp/cldr-intl-json
python3 engine/boa/core/icu_provider/scripts/intl-cldr/verify.py \
  --source-zip /tmp/cldr-47.0.0-json-full.zip \
  --source-root-xml /tmp/cldr-root-47.xml \
  --output-dir /tmp/cldr-intl-json
```

Missing source paths are downloaded from the fixed URLs and checked against
their pinned SHA256 before extraction. Existing output directories are rejected.
`--verify-tag` additionally checks that the upstream JSON tag still resolves to
the recorded commit. Final-schema JSON provenance records source hashes, all
18,604 input-file hashes, eight extractor source hashes, fifteen dataset hashes,
counts and sizes.
`verify.py` compares every source locale/currency/unit record independently,
including XML aliases and ordering. Generate a second directory and pass
`--compare-dir SECOND_DIR` to verify byte-identical generation.

Run the unit tests from this script directory:

```sh
python3 -m unittest discover -s . -p 'test_*.py' -v
```

Export from `engine/boa`, using the repository's worktree-specific Cargo target
and capacity guard. The new workspace crate must already be present in both
the repository and nested Cargo lockfiles before using `--locked`:

```sh
cargo run --locked -p boa_intl_data --features datagen \
  --example export_cldr_data -- \
  /tmp/cldr-intl-json /tmp/omoikane_intl.postcard \
  /tmp/omoikane_intl.provenance.json
```

This example depends on the marker crate and existing ICU blob exporter. It does
not depend on `boa_icu_provider`, so it can bootstrap a missing bundle without
compiling the provider's `include_bytes!`. JSON and Python are generator inputs,
not runtime dependencies. Input dataset pins/hashes are checked again before
export. Output paths must be new. The report records input provenance, marker
counts, binary bytes and binary SHA256. Export to a second new path, compare the
two binaries, then copy the reviewed binary and report into
`core/icu_provider/data/omoikane_intl.{postcard,provenance.json}`.

Validate the shared schemas with `cargo test --locked -p boa_intl_data`; validate
actual bundled payloads and borrowed storage with
`cargo test --locked -p boa_icu_provider --test intl_extension`. A marker crate
check with `--no-default-features` verifies that its runtime schemas do not
require the blob/exporter. Complete the repository's required tests/builds
before committing the integrated runtime changes.

Coverage is all 739 CLDR JSON locales, 155,792 localized currency records, 73
normal/cash precision records, 881 locale/numbering-system symbol and number/
currency-pattern records, 101,982 runtime-reachable unit records across three
widths, 2,217 compound-width records and all 739 × 77 digital-duration
combinations. Empty numbering-system attributes additionally select each
locale's declared default. The JSON extractor retains and independently checks
all 410,811 source unit records. The postcard exporter selects the shared 45
ECMA-402 sanctioned simple-unit IDs plus the canonical
`speed-kilometer-per-hour` record for every locale and width. Arbitrary
sanctioned compounds use two retained simple-unit records; per-unit and
compound-per patterns are preserved. The extractor checks 704 duration patterns
that omit a number without requiring `{0}` in those patterns.

Calendar extraction also preserves all 575,920 available-format patterns across
739 locales × 17 calendars. Its additional input paths are
`cldr-dates-full/main/*/ca-gregorian.json` and
`cldr-cal-*-full/main/*/ca-*.json` from the same pinned JSON ZIP. The
`calendar_date_patterns.json` dataset maps locale and Unicode calendar ID to
the source calendar ID and its complete raw `dateTimeFormats.availableFormats`
map, including time patterns, source variants, quoting and literals. All 12,563
calendar source files are hashed in provenance; the independent verifier
compares every skeleton and pattern. The original nine number/currency/unit
datasets and eleven marker schemas were unchanged by the original calendar-date
addition; these are historical intermediate counts.

ICU4X 2.0's semantic date skeleton can select a month width from the locale's
dateFormat before component-width adjustment. This is insufficient when another
availableFormat width also changes calendar fields: CLDR `zh/chinese`
`yyyyMMMd` is `r年MMMd`, while `yyyyMMMMd` is `rU年MMMMd`. Native DateTimeFormat
therefore uses exact explicit date-component skeletons from this typed marker,
then passes the localized pattern to ICU `FixedCalendarDateTimeNames` for
calendar conversion, cyclic-year names and typed parts. Numeric-year `y` can
match the equivalent lunar-calendar `yyyy` skeleton; other widths remain exact.
When no exact skeleton exists, the existing ICU semantic pattern is used. Style
requests keep their existing native pattern selection. Clock patterns remain
selected by ICU and are combined through the locale's native CLDR glue; a raw
date pattern cannot silently drop clock fields.

Compact extraction adds `compact_patterns.json` from the existing hashed
`cldr-numbers-full/main/*/numbers.json` inputs. Decimal patterns are read from
`decimalFormats-numberSystem-<nu>/{short,long}/decimalFormat`; currency patterns
are read from `currencyFormats-numberSystem-<nu>/short/standard`. The dataset
records these source paths and separates normal and `alt-alphaNextToNumber`
variants without changing any raw pattern. The independent verifier rebuilds
every source key and compares all 59,376 source rows.

There are 2,606 payloads: 881 short decimal, 881 long decimal and 844 short
currency. The 54,442 normal and 4,934 alpha patterns include 240 raw `0`
sentinels, 52 number-omitting patterns, 47 explicit-count-one patterns and
13 alpha plural pairs absent from their normal table. Thresholds reach 10^19
and are validated as exact positive u64 powers of ten. Normal thresholds have
`other`; alpha variants retain their own category coverage without requiring
the same category in the normal table. All 739 locales contain Latin compact
currency data; 37 of the 881 explicit currency-system records lack this compact
family. The exporter does not invent numbering-system aliases or global
English labels. Native consumers own fallback and formatting policy.

Compact data was the thirteenth marker, added independently of the then-existing
twelve schemas. That historical thirteen-marker generation used the same pinned
ZIP/XML/license and 14,786 hashed input files. Generate new directories and
export to new postcard paths; retain each reviewed previous blob and provenance
before installation.

Number-range extraction adds `number_range_patterns.json` and the independent
fourteenth marker `OmoikaneNumberRangePatternsV1`. It preserves the 881 raw
`miscPatterns-numberSystem-<nu>` range/approximately records in the same numbers
inputs, including all 14 distinct spacing, separator and approximation pairs.
The additional ZIP input is `cldr-core/supplemental/pluralRanges.json`
(24,710 bytes, SHA256
`a2c4e48d32d84a9664c9da11d8855937284721fd7ca5fbd78a8237fcde6cbe29`),
containing 441 explicit ordered cardinal pairs for 91 source languages.
Supplemental rules inherit by successively removing region/script subtags;
main-data nondefault-script parents such as `sr-Latn -> und` do not discard the
language's plural rules. Unknown languages and `und` retain an empty rule table.

The pinned root has miscellaneous patterns for 77 numeric systems. Every
non-Latin system aliases the Latin patterns in the **same locale**. The
extractor preserves the full XML elements, alias paths and resolved root
patterns; the exporter applies those aliases to each locale's own Latin data
when an explicit system record is absent. This supplies 739 × (77 + default)
= 57,642 marker IDs while retaining Japanese `約 {0}` and `～` rather than
substituting the root's `~` and en dash. `und` is already one of the 739 JSON
locales and its Latin patterns match the root; it is not added a second time.
Empty marker attributes select the locale's declared default numbering system.

Typed payloads hold two zero-copy optional strings, 36 u8 result slots and a
presence bitset. `range()`/`approximately()` expose raw patterns, and
`plural_range(start, end)` returns only the exact explicit cardinal rule.
Missing pairs remain distinguishable from an explicit `other`; explicit
numeric-zero/one overrides are not cardinal categories. Custom nonbundled
providers use the same schema without depending on the bundled blob.
The [pinned CLDR 47 specification](https://raw.githubusercontent.com/unicode-org/cldr/2ef784e3a4168bc2a43cd1b5b9839b6636f5899c/docs/ldml/tr35-numbers.md#Plural_Ranges)
defines missing-pair default as the end category, and leaves negative or
nonascending number ranges unspecified. Runtime consumers own that policy,
semantic affix collapse and approximation. The raw miscellaneous approximation
pattern is preserved independently from `NumberSymbol::ApproximatelySign`;
its presence does not require a runtime to adopt legacy pattern whitespace.

The independent verifier compares every raw miscellaneous record, all source
plural pairs, script/region inheritance and every preserved root XML element.
Schema contracts cover ordered asymmetry, missing rules and invalid custom
payloads. Bundled provider contracts check locale/default/explicit-system
aliases, language inheritance and actual zero-copy string/vector storage.
The original number-range addition kept the then-existing eleven datasets and
thirteen marker contracts unchanged; the current complete schema has fifteen
datasets and seventeen markers.

## Date-time intervals, relative time and display names

`calendar_interval_patterns.json` adds marker fifteen,
`OmoikaneCalendarIntervalPatternsV1`. It preserves raw
`dateTimeFormats.intervalFormats` fallback and skeleton/field patterns from all
739 × 17 calendar records, including source variants, quotes and literals. The
independent verifier compares 12,563 interval records, 393,932 skeletons and
911,403 raw patterns directly to the pinned source.

`relative_time_patterns.json` and `display_names.json` add markers sixteen and
seventeen, `OmoikaneRelativeTimePatternsV1` and `OmoikaneDisplayNamesV1`. They
preserve localized source patterns, labels and missing translations; the
independent verifier rebuilds their records without using the extractor.
Currency labels are not duplicated in marker seventeen: DisplayNames reads
long names and raw standard/narrow symbols from the existing per-code currency
marker. The recorded full-source comparisons cover 106,360 relative-time
records and 652,702 display-name records.

The 2026-10-10 JSON validation passed 41 generator tests with zero skips.
Two generations produced seventeen byte-identical files: fifteen datasets,
provenance and the unchanged Unicode license. Independent source verification
checked all 18,604 hashed source inputs. Both new directories passed the
fifteen-dataset pin/hash validator. Use
`--legacy-dir PREVIOUS_VERIFIED_JSON_DIR` with `verify.py` to retain that check.

This JSON proof is recorded under
`native-existing-intl-generated-dn-dedup-final-20261010T042600Z` in the
owned-cache evidence directory. A subsequent locked release export produced
two byte-identical seventeen-marker postcards of **25,302,285 bytes**, SHA256
`fa01fc293d3911e5ec1a19211961252197a724d0fd4d67d6644a63f66de52ec5`.
Typed old/new comparison retained every identifier set, changed only the
currency-text and DisplayNames payloads, and kept all other 15 marker payloads
identical.
The exporter serializes only raw standard and narrow currency symbols;
NumberFormat supplies its established fallback without synthesized copies, and
DisplayNames selects long, short and narrow values from that same typed payload.
The separate export and test proof is retained under
`native-existing-intl-dn-compact-final-20261010T050000Z`. These are
source-generation, serialized-data and focused Intl checks. The postcard byte
count is not an executable-size or startup result. Subsequent actual results
are recorded in [Issue #1295](https://github.com/ieee0824/omoikane/issues/1295).

The subsequent reachable-unit and DurationDigital field reduction keeps all
17 markers and every locale. Two locked exports were byte-identical at
**19,117,677 bytes**, SHA256
`1bb37c3ffa1d9a8a72c7c7031fd0ece81ec1b6a777edcfa39d035414d6db16e0`.
This removes 308,829 unreachable unit payloads and five unused strings from
each of 57,642 digital-duration payloads, reducing the postcard by 6,184,608
bytes (24.44%).

Canonical JSON and exploratory zlib sizes in extraction reports are not runtime
size measurements. Use the actual postcard report and executable size after
export/build to assess bundle growth. The full coverage is deliberately larger
than the pre-existing modern-locale ICU datasets; native consumers still rely
on the capabilities of their other ICU markers.

## Standard decimal digits

The separate digits bundle uses the standard ICU `DecimalDigitsV1` marker.
Its source is `cldr-core/supplemental/numberingSystems.json` in the same pinned
CLDR JSON ZIP: member SHA256
`7fbe7f10209e9770538cef70dfe97e295d06edebe4161908332eea02418f2642`.
The source metadata declares CLDR 47 and Unicode 16.0.0. All 77 `numeric`
records are preserved as ten Unicode scalars each in
[`data/icu_decimal_digits.source.tsv`](../../data/icu_decimal_digits.source.tsv),
SHA256 `1ce679988243be38588277107e5bef0cdfde1240286efee50282525648807d05`.
Algorithmic systems and Unicode 17 `tols` are not added to this source.
The unchanged Unicode License V3 notice is retained in
[`data/icu_decimal_digits.LICENSE.txt`](../../data/icu_decimal_digits.LICENSE.txt).

From `engine/boa`, use the existing worktree-owned target and capacity guard,
and choose a **new output path**:

```sh
cargo run --locked --release -p boa_icu_provider --example export_decimal_digits -- \
  core/icu_provider/data/icu_decimal_digits.source.tsv \
  core/icu_provider/data/icu_decimal.postcard /tmp/NEW-decimal-digits.postcard
cargo test --locked -p boa_icu_provider --test intl_extension \
  all_pinned_numeric_digits_are_formatted_without_latin_fallback
```

The [exporter](../../examples/export_decimal_digits.rs) validates the TSV and
all existing component digit values, then writes a new standard-marker blob;
it must report `exported=77 preserved_existing=27`. Its existing dev dependencies
are sufficient; no manifest, feature or lockfile change is required. It does not
rewrite the TSV, component blob or separate Intl extension bundle.

Before replacing data, compare all 77 decoded mappings to the pinned source and
all 27 old serialized payloads byte-for-byte. The retained
[provenance](../../data/icu_decimal_digits.provenance.json) records the source ZIP,
member, TSV, license, exporter and validator hashes plus the actual 4,258-byte
blob hash and both completed equality checks. The exporter checks old values;
the independent post-export validation supplies the serialized-byte proof.
The behavioral provider test verifies 770 real ICU outputs. These data bytes
and generation checks are not executable-size or startup measurements.
