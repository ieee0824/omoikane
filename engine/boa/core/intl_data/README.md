# boa_intl_data

Native Intl services share these ICU4X 2.0 typed CLDR schemas. The crate contains
marker IDs and zero-copy payload types. Its default build has no bundled data,
blob dependency, JSON loader, Python requirement, I/O, or locale fallback policy.
An embedder using nonbundled Intl can implement these markers in its own typed
provider, or deserialize the same marker payloads from a custom buffer provider.

Keep a `DataPayload<M>` as the owned state of a formatter and borrow `get()` only
while formatting. `VarZeroVec<str>` and `ZeroMap` borrow the provider buffer;
`StringFields::present` distinguishes missing fields from present empty strings.
Raw patterns preserve quoting, signs, bidi controls, whitespace and plural
patterns that intentionally omit `{0}`.

| Marker | Request attributes | Payload |
| --- | --- | --- |
| `OmoikaneNumberSymbolsV1` | numbering system, or empty for locale default | Nonfinite, sign and numeric symbols |
| `OmoikaneGlobalNumberSymbolsV1` | numbering system; locale `und` | CLDR root numeric symbols |
| `OmoikaneNumberPatternsV1` | numbering system, or empty for locale default | Raw decimal/percent/scientific patterns |
| `OmoikaneGlobalNumberPatternsV1` | numbering system; locale `und` | CLDR root number patterns |
| `OmoikaneCurrencyPatternsV1` | numbering system, or empty for locale default | Standard/accounting affixes, spacing, name patterns |
| `OmoikaneGlobalCurrencyPatternsV1` | numbering system; locale `und` | CLDR root currency patterns |
| `OmoikaneCurrencyTextV1` | uppercase three-letter currency code | Symbols, narrow symbols, names and overrides |
| `OmoikaneCurrencyDigitsV1` | empty; singleton `und` | Normal/cash precision maps |
| `OmoikaneUnitPatternsV1` | width and CLDR unit ID, e.g. `long-length-meter` | Plural unit patterns and per-unit overrides |
| `OmoikaneCompoundUnitPatternsV1` | `long`, `short` or `narrow` | Compound per/times patterns |
| `OmoikaneDurationDigitalV1` | numbering system, or empty for locale default | Resolved time separator and effective numbering system |
| `OmoikaneCalendarDatePatternsV1` | Unicode calendar ID, e.g. `gregory`, `chinese`, `ethioaa` | Exact CLDR availableFormats skeleton/pattern map |
| `OmoikaneCompactPatternsV1` | `decimal-short-<nu>`, `decimal-long-<nu>`, `currency-short-<nu>` | Raw compact patterns by threshold and plural category |
| `OmoikaneNumberRangePatternsV1` | numbering system, or empty for locale default | Raw range/approximately patterns and explicit ordered cardinal plural rules |

`DurationDigitalField::TimeSeparator` preserves the locale's explicit numeric
separator when present and otherwise uses the CLDR root separator for that
numbering system. `DurationDigitalField::NumberingSystem` records the effective
system for an empty default attribute. Raw hm/hms/ms patterns and the separate
global/locale separator sources are generator inputs only because no runtime
consumer reads them.

`CurrencyDigits::digits` uses normal precision and falls back to the CLDR default
for unknown valid codes. Cash rounding is separate. Name/plural/sign policy,
currency UnicodeSet spacing rules belong to the engine. The ECMA-402 sanctioned
unit mapping is shared by the runtime and exporter so the bundle contains only
records that NumberFormat or DurationFormat can request.

The bundled unit marker contains the 45 sanctioned simple units and the
canonical `speed-kilometer-per-hour` record for all 739 locales and three
widths: 101,982 payloads. Other sanctioned compounds are assembled from two
simple-unit payloads and the retained locale/width compound-per pattern.

`CalendarDatePatterns` preserves every raw available-format pattern for all 739
locales and 17 CLDR calendars. The Unicode identifiers `gregory` and `ethioaa`
map to CLDR source calendars `gregorian` and `ethiopic-amete-alem`; the extractor
records the source identifiers. An ISO civil-date formatter can use Gregorian
patterns while keeping its ISO calendar identity. Calendar selection, component
skeleton matching and hour-cycle policy belong to the runtime. For explicit
DateTimeFormat components, custom providers must supply this marker alongside
the existing ICU calendar/name/pattern markers. Missing marker data is an error;
an absent exact skeleton leaves ICU semantic matching in control.

`CompactPatterns` has independent normal and alpha-next-to-number
`ZeroMap2d<u64,u8,str>` tables. Thresholds are positive CLDR powers of ten, up to
10^19; plural keys are the stable `PluralCategory` indices. Its `get` and
`get_alpha_next_to_number` return exact values; `magnitudes` returns ascending
normal thresholds. Alpha variants can add categories absent from the normal
table. All raw `0` sentinels, explicit count `1`, number-omitting patterns,
positive/negative subpatterns, quotes and directionality controls are retained.
Plural fallback, divisor selection, rounding and parts construction belong to
the NumberFormat consumer. Only actual CLDR family/numbering-system payloads
are exported: all 739 locales have Latin compact families, while 37 explicitly
listed non-Latin currency systems have no compact currency records. Consumers
can request the same locale's Latin pattern data while keeping selected digits;
the schema does not synthesize another locale's labels.

`NumberRangePatterns` exposes `range()` and `approximately()` as raw optional
strings. `plural_range(start, end)` returns only explicit ordered cardinal rules:
missing pairs remain distinct from explicit `other` and explicit numeric zero/
one are rejected. The result table uses 36 zero-copy u8 slots and a presence
bitset. The bundled marker supplies all 739 locales and 77 numeric systems,
plus empty default attributes. For a system missing from the locale's source,
the recorded root alias selects that locale's Latin miscellaneous patterns,
preserving localized approximation text and separators. Supplemental rules
keep language inheritance even when a main-data script parent is `und`.
`und` patterns are root data with no invented plural rules.
Affix collapse and approximation are runtime policy: raw miscellaneous
`approximately` and `NumberSymbol::ApproximatelySign` are independent fields.
The [pinned CLDR 47 plural-range specification](https://raw.githubusercontent.com/unicode-org/cldr/2ef784e3a4168bc2a43cd1b5b9839b6636f5899c/docs/ldml/tr35-numbers.md#Plural_Ranges)
defines an absent pair's default as its end category and leaves negative or
nonascending numeric ranges unspecified. The schema does not apply that policy.

The `datagen` feature and `export_cldr_data` example are generator-only. They use
existing locked `databake` 0.2.0 and `zerovec` 0.11.4, plus ICU4X's existing blob
exporter. Python's standard-library SHA256 verifies generator inputs and records
the binary hash, avoiding another runtime or exporter dependency.

See [the generator instructions](../icu_provider/scripts/intl-cldr/README.md)
for pinned sources, validation, the independent exporter bootstrap and bundle
regeneration. The schemas are separate from `boa_icu_provider` so nonbundled
Intl consumers do not acquire the bundled blob dependency.
