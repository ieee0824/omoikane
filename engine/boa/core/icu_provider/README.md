# boa_icu_provider

`boa_icu_provider` defines the [ICU4X](https://github.com/unicode-org/icu4x) data provider
used in the Boa engine to enable internationalization functionality.

The calendar blob contains five singleton calculation datasets: Chinese, Dangi,
simulated Hijri (Mecca), modern Japanese eras, and extended Japanese eras. They
are separate from the locale-dependent date/time name and pattern blob.
`CalendarWeekV1` is not advertised by this provider.

Regenerate only this blob from the baked dataset selected by `Cargo.lock`, from
the `engine/boa` directory:

```sh
cargo run --locked -p boa_icu_provider --example export_calendar_data -- /tmp/icu_calendar.postcard
```

The exporter requires a new output path and does not remove existing data files.
Compare and record the generated file's hash before replacing
`core/icu_provider/data/icu_calendar.postcard`. The exporter-only `compiled_data`
and blob export features are dev dependencies, not requirements of the runtime
buffer provider.

The separate `omoikane_intl.postcard` extension is defined by 17 shared typed
markers from `boa_intl_data`. The first fourteen retain the number, currency,
unit, duration, calendar-date, compact-number and number-range schemas. The
additional three supply DateTimeFormat interval patterns, RelativeTimeFormat
patterns and DisplayNames labels, in that order. The extension selects all 739
CLDR 47 full JSON locales and 77 numeric numbering systems, with
currency symbols/names/patterns/precision, nonfinite and sign symbols, decimal/
percent/scientific patterns, unit/per patterns, digital duration source data,
and exact available-format patterns for 17 CLDR calendars. DateTimeFormat uses
these calendar patterns through the existing ICU native names formatter.
Compact-number data retains short/long decimal and short currency patterns by
source threshold and plural category, including independent alphabetic-currency
variants and number-omitting source patterns.
Number-range data keeps miscellaneous range/approximation patterns and ordered
cardinal plural rules. Its recorded root numbering-system aliases select each
locale's own Latin patterns when needed, retaining localized labels. The shared
marker list advertises this extension through the existing buffer provider.
The other locale-dependent ICU component markers retain their modern coverage;
coverage varies by marker and does not imply complete ECMA-402 compatibility.
Its strings and maps deserialize as borrowed zero-copy payloads. Runtime services
load the markers they need rather than parsing the complete JSON dataset.

Follow-up JSON regeneration on 2026-10-10 reproduced all fifteen datasets and
independently checked the pinned full source. The date-interval data includes
all 739 × 17 calendar records; relative-time and display-name data are compared
to their raw source records. Currency long names and short/narrow symbols reuse
the existing per-code currency marker, avoiding a second locale-wide copy in
the DisplayNames marker. The currency marker stores raw CLDR symbols so absence
is preserved; NumberFormat retains its narrow-to-standard-to-code fallback.
A locked exporter run produced two byte-identical seventeen-marker postcards
of **25,302,285 bytes**, SHA256
`fa01fc293d3911e5ec1a19211961252197a724d0fd4d67d6644a63f66de52ec5`.
Typed comparison retained every marker identifier set and found changes only
in the intended currency-text and DisplayNames payloads; the other 15 markers
were payload-identical.
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

Focused Japanese interval, DisplayNames, NumberFormat currency fallback and
provider extension tests passed. These checks do not certify whole-executable
size/startup or every final validation gate. See
[Issue #1295](https://github.com/ieee0824/omoikane/issues/1295) for subsequent
actual results. Earlier measured fourteen-marker provider comparisons are
historical; the production default retains full locale coverage because the
three-locale filter regressed existing localized NumberFormat and DurationFormat
output outside its selected locales.

The shared schemas also work with nonbundled custom data providers. See
[the marker crate](../intl_data/README.md) for request attributes and
[the pinned generation instructions](scripts/intl-cldr/README.md) for source
hashes, Unicode licensing, reproducible export and payload validation.

## Standard decimal digits

[`data/icu_decimal_digits.postcard`](data/icu_decimal_digits.postcard) supplies
only ICU's standard `DecimalDigitsV1` marker, with all **77 numeric mappings**
from CLDR 47 / Unicode 16.0.0. It precedes the existing decimal component provider
for this marker; decimal symbols and every other component marker retain their
existing route. This supplements the original 27 digit mappings without an
Ahom-specific runtime branch, a new external dependency or a new CLDR schema.

The blob is **4,258 bytes**, SHA256
`93122e8d1950e3676096ad84cf30595b8e41246393b54aa25e9da6b2af1c8c19`.
Its [source TSV](data/icu_decimal_digits.source.tsv),
[validation/provenance](data/icu_decimal_digits.provenance.json) and full
[Unicode License V3](data/icu_decimal_digits.LICENSE.txt) are kept beside it.
The license is `Unicode-3.0`, independent of the Rust code's MIT/Unlicense terms.
The validation records exact agreement with all 77 source mappings and
byte-identical serialized payloads for all 27 original mappings. The original
`icu_decimal.postcard` was unchanged by the digits addition, which also left the
then-current fourteen-marker `omoikane_intl.postcard` untouched. The later
seventeen-marker extension has the separate serialized-data evidence above;
its final native runtime checks were pending at that export-validation snapshot.

The bundled provider contract formats all ten digits for each system: **770
actual outputs**, including supplementary-plane characters and noncontiguous
`hanidec` digits. The historical digits validation passed all eight provider
contracts on that source snapshot; public NumberFormat and DurationFormat
contracts also check all 77 systems and typed
parts. Unicode 17 `tols` is outside the pinned source and remains unsupported.
See [the exporter and source checks](scripts/intl-cldr/README.md#standard-decimal-digits)
for reproduction.
