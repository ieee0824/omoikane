//! Shared typed CLDR data for native Intl services.
//!
//! This crate contains schemas and marker identifiers only. It has no bundled
//! locale data, JSON loader, file I/O, or locale fallback policy. Embedders using
//! nonbundled Intl can implement the same markers in their own data provider.
//! Strings are stored in zero-copy vectors and borrowed from provider payloads.
#![no_std]

extern crate alloc;

mod compact;
mod currency;
mod datetime;
mod names;
mod number;
mod number_range;
mod strings;
mod units;

pub use compact::{CompactPatterns, OmoikaneCompactPatternsV1};
pub use currency::{
    CurrencyDigits, CurrencyPatternField, CurrencyPatterns, CurrencyText, CurrencyTextField,
    OmoikaneCurrencyDigitsV1, OmoikaneCurrencyPatternsV1, OmoikaneCurrencyTextV1,
    OmoikaneGlobalCurrencyPatternsV1,
};
pub use datetime::{
    CalendarDatePatterns, CalendarIntervalPatterns, OmoikaneCalendarDatePatternsV1,
    OmoikaneCalendarIntervalPatternsV1,
};
pub use names::{
    DisplayNamesData, OmoikaneDisplayNamesV1, OmoikaneRelativeTimePatternsV1, RelativeTimePatterns,
};
pub use number::{
    NumberPatternField, NumberPatterns, NumberSymbol, NumberSymbols,
    OmoikaneGlobalNumberPatternsV1, OmoikaneGlobalNumberSymbolsV1, OmoikaneNumberPatternsV1,
    OmoikaneNumberSymbolsV1,
};
pub use number_range::{NumberRangePatterns, OmoikaneNumberRangePatternsV1};
pub use strings::{PluralCategory, PluralPatterns, StringFields};
pub use units::{
    CANONICAL_COMPOUND_UNIT_IDENTIFIERS, CompoundUnitPatterns, DurationDigital,
    DurationDigitalField, OmoikaneCompoundUnitPatternsV1, OmoikaneDurationDigitalV1,
    OmoikaneUnitPatternsV1, SANCTIONED_SIMPLE_UNIT_IDENTIFIERS, UnitPatterns, UnitWidth,
    canonical_compound_unit_cldr_id, is_reachable_unit_cldr_id, sanctioned_simple_unit,
};

use icu_provider::{DataMarker, DataMarkerInfo};

/// All marker schemas supplied by the bundled CLDR extension blob.
pub const MARKERS: &[DataMarkerInfo] = &[
    OmoikaneNumberSymbolsV1::INFO,
    OmoikaneGlobalNumberSymbolsV1::INFO,
    OmoikaneNumberPatternsV1::INFO,
    OmoikaneGlobalNumberPatternsV1::INFO,
    OmoikaneCurrencyPatternsV1::INFO,
    OmoikaneGlobalCurrencyPatternsV1::INFO,
    OmoikaneCurrencyTextV1::INFO,
    OmoikaneCurrencyDigitsV1::INFO,
    OmoikaneUnitPatternsV1::INFO,
    OmoikaneCompoundUnitPatternsV1::INFO,
    OmoikaneDurationDigitalV1::INFO,
    OmoikaneCalendarDatePatternsV1::INFO,
    OmoikaneCompactPatternsV1::INFO,
    OmoikaneNumberRangePatternsV1::INFO,
    OmoikaneCalendarIntervalPatternsV1::INFO,
    OmoikaneRelativeTimePatternsV1::INFO,
    OmoikaneDisplayNamesV1::INFO,
];
