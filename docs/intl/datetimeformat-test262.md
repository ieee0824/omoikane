# DateTimeFormat: fixed Test262 compatibility verification

Test262 revision: 2e0a56762801e275a9fdf96dc49d90ba0cddcf63. The original fixtures and harness remain unchanged; ignored tests and features are empty.

The baseline reported in #1334 was 163 PASS / 82 FAIL. Latest unfiltered runs use the same compiled binary in both environments: C.UTF-8 yields 244 PASS / 1 FAIL; en_US.UTF-8 yields 245 PASS / 0 FAIL. Both runs have zero ignored tests and zero panics. Full Temporal yields 4605 PASS / 0 FAIL.

The remaining C.UTF-8 fixture uses an empty locale list and expects a dayPeriod field. Its implicit default locale resolves to h23; the same strict and non-strict fixture passes with en-US. DefaultLocale is implementation-defined, and the fixture does not request a twelve-hour clock. This failure is preserved rather than ignored or hidden by changing the runtime default locale.

References: [DefaultLocale](https://tc39.es/ecma402/#sec-defaultlocale), [CreateDateTimeFormat](https://tc39.es/ecma402/#sec-createdatetimeformat). Temporal-enabled identifier preservation follows the [Temporal Intl integration](https://tc39.es/proposal-temporal/#sec-createdatetimeformat). The non-Temporal path retains primary-identifier normalization.

## Individual baseline failures

Each row names an original failure, its classification, and latest actual results. The source SHA and full original error are retained in the [accompanying JSON evidence](datetimeformat-test262-baseline.json).

| Original fixture | Classification | C.UTF-8 | en-US |
| --- | --- | --- | --- |
| DateTimeFormat/prototype/formatRangeToParts/to-datetime-formattable-with-different-arg-kinds.js | implementation: both argument conversions and native Temporal kind validation | PASS | PASS |
| DateTimeFormat/prototype/formatRangeToParts/fractionalSecondDigits.js | implementation: fractional numeric field widths and parts/range consistency | PASS | PASS |
| DateTimeFormat/prototype/formatRangeToParts/temporal-objects-format-with-era.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRangeToParts/temporal-objects-no-time-clip.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRangeToParts/temporal-objects-not-overlapping-options.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRangeToParts/chinese-calendar-dates.js | implementation/data: related year and lunar month codes from source CLDR patterns | PASS | PASS |
| DateTimeFormat/prototype/formatRangeToParts/temporal-plaindatetime-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRangeToParts/temporal-plainmonthday-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRangeToParts/temporal-objects-ignore-timezone.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRangeToParts/temporal-objects-resolved-time-zone.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRangeToParts/temporal-objects-throws-with-different-calendars.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRangeToParts/temporal-plaintime-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRangeToParts/temporal-plainyearmonth-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRangeToParts/temporal-objects-no-time-clip-weekday.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRangeToParts/temporal-plaindate-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/numbering-system.js | implementation: semantic numeric field widths with locale digits | PASS | PASS |
| DateTimeFormat/prototype/format/dayPeriod-long-en.js | implementation/data: CLDR flexible day-period rules and names | PASS | PASS |
| DateTimeFormat/prototype/format/fractionalSecondDigits.js | implementation: fractional numeric field widths and parts/range consistency | PASS | PASS |
| DateTimeFormat/prototype/format/timedatestyle-en.js | implementation/data: exact CLDR date/time style composition | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-objects-format-with-era.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-objects-no-time-clip.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-objects-not-overlapping-options.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-plaindatetime-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-plainmonthday-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-plainmonthday-formatting-datetime-style.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-objects-ignore-timezone.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-plaindate-formatting-datetime-style.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-objects-resolved-time-zone.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-objects-format-weekday-long.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-plaintime-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-objects-no-time-clip-non-latin-numerals.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-plainyearmonth-formatting-datetime-style.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-plaindatetime-formatting-datetime-style.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/dayPeriod-short-en.js | implementation/data: CLDR flexible day-period rules and names | PASS | PASS |
| DateTimeFormat/prototype/format/dayPeriod-narrow-en.js | implementation/data: CLDR flexible day-period rules and names | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-plaintime-formatting-datetime-style.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-plainyearmonth-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-objects-no-time-clip-weekday.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/format/temporal-plaindate-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/dayPeriod-long-en.js | implementation/data: CLDR flexible day-period rules and names | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/fractionalSecondDigits.js | implementation: fractional numeric field widths and parts/range consistency | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/temporal-objects-format-with-era.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/temporal-objects-no-time-clip.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/temporal-objects-not-overlapping-options.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/chinese-calendar-dates.js | implementation/data: related year and lunar month codes from source CLDR patterns | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/era.js | implementation/data: era fields and native Hijri AH/BH names | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/temporal-plaindatetime-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/temporal-plainmonthday-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/temporal-objects-ignore-timezone.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/temporal-objects-resolved-time-zone.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/temporal-plaintime-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/compare-to-temporal-lunisolar.js | implementation/data: Temporal arithmetic year, lunar code versus ordinal, native Hebrew leap names | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/dayPeriod-short-en.js | implementation/data: CLDR flexible day-period rules and names | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/dayPeriod-narrow-en.js | implementation/data: CLDR flexible day-period rules and names | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/temporal-plainyearmonth-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/temporal-objects-no-time-clip-weekday.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/compare-to-temporal.js | implementation/data: calendar date overflow and native Hijri AH/BH names | PASS | PASS |
| DateTimeFormat/prototype/formatToParts/temporal-plaindate-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRange/to-datetime-formattable-with-different-arg-kinds.js | implementation: both argument conversions and native Temporal kind validation | PASS | PASS |
| DateTimeFormat/prototype/formatRange/fractionalSecondDigits.js | implementation: fractional numeric field widths and parts/range consistency | PASS | PASS |
| DateTimeFormat/prototype/formatRange/temporal-objects-format-with-era.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRange/temporal-objects-no-time-clip.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRange/temporal-objects-not-overlapping-options.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRange/temporal-plaindatetime-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRange/temporal-plainmonthday-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRange/temporal-objects-ignore-timezone.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRange/temporal-objects-resolved-time-zone.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRange/temporal-objects-throws-with-different-calendars.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRange/temporal-plaintime-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRange/temporal-plainyearmonth-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRange/temporal-objects-no-time-clip-weekday.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/formatRange/temporal-plaindate-formatting-timezonename.js | implementation: owned native Temporal inputs, relevant-field formatting and calendar/timezone/range validation | PASS | PASS |
| DateTimeFormat/prototype/resolvedOptions/order-dayPeriod.js | environment: default locale clock style (retained C failure; en-US passes) | FAIL | PASS |
| DateTimeFormat/prototype/resolvedOptions/calendar.js | implementation/data: supported calendar enumeration and canonical resolution | PASS | PASS |
| DateTimeFormat/prototype/resolvedOptions/hourCycle-default.js | implementation/data: locale-specific preferred twelve-hour cycle | PASS | PASS |
| DateTimeFormat/canonicalize-utc-timezone.js | implementation: named timezone case, link and UTC resolution | PASS | PASS |
| DateTimeFormat/intl-legacy-constructed-symbol-on-unwrap.js | implementation: shared fallback symbol and observable legacy receiver unwrapping | PASS | PASS |
| DateTimeFormat/canonicalize-timezone.js | implementation: named timezone case, link and UTC resolution | PASS | PASS |
| DateTimeFormat/intl-legacy-constructed-symbol-property.js | implementation: shared fallback symbol and observable legacy receiver unwrapping | PASS | PASS |
| DateTimeFormat/intl-legacy-constructed-symbol.js | implementation: shared fallback symbol and observable legacy receiver unwrapping | PASS | PASS |
| DateTimeFormat/timezone-not-canonicalized.js | implementation: named timezone case, link and UTC resolution | PASS | PASS |
| DateTimeFormat/timezone-case-insensitive.js | implementation: named timezone case, link and UTC resolution | PASS | PASS |

## Other verification

- Root cargo test --locked -- --include-ignored: 3945 PASS, zero failed or ignored tests. CI=1 and WPT_REQUIRED=1; fixed original WPT manifest has 415 PASS / 5 known failures / 0 regressions across 420 cases.
- Root cargo build --locked: passed.
- Latest engine library tests with annex-b,intl_bundled,experimental: 1240 PASS, zero failed or ignored tests.
- Original major-locale Firefox comparison: all 155 leaves match for en-US, ja-JP, and de-DE. This sample does not establish equivalence for every formatter or locale.
- Provider and generator contracts: 16 and 49 PASS; all 89 corresponding source/data files remain identical to that verified revision.
- Native JIT contracts: eight test binaries / 38 tests passed; all 16 native/interpreter comparisons matched.

## Reproduction

Create an unfiltered runner configuration containing:

```toml
commit = "2e0a56762801e275a9fdf96dc49d90ba0cddcf63"
[ignored]
tests = []
features = []
```

Build with `cargo build --locked --manifest-path engine/boa/Cargo.toml -p boa_tester`. Run the same binary with `run --test262-path <checkout> --suite test/intl402/DateTimeFormat --config <unfiltered.toml> --disable-parallelism -vvv --output <fresh-directory>`, once with both LANG and LC_ALL set to C.UTF-8 and once with both set to en_US.UTF-8. Use separate output directories. Run `test/built-ins/Temporal` with the same pin and configuration. Runner exit status alone is insufficient: inspect the emitted result JSON for total, passed, ignored, and panic counts.
