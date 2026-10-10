//! Four ECMA-402 hour cycles, including the variant not represented by ICU 2.0.
use crate::context::icu::IntlProvider;
use icu_datetime::{
    preferences::HourCycle,
    provider::{
        DatetimePatternsTimeV1,
        fields::{FieldSymbol, Hour},
    },
};
use icu_locale::LanguageIdentifier;
use icu_provider::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DateTimeHourCycle {
    H11,
    H12,
    H23,
    H24,
}

impl DateTimeHourCycle {
    pub(super) fn from_str(value: &str) -> Option<Self> {
        match value {
            "h11" => Some(Self::H11),
            "h12" => Some(Self::H12),
            "h23" => Some(Self::H23),
            "h24" => Some(Self::H24),
            _ => None,
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::H11 => "h11",
            Self::H12 => "h12",
            Self::H23 => "h23",
            Self::H24 => "h24",
        }
    }

    pub(super) const fn icu(self) -> HourCycle {
        match self {
            Self::H11 => HourCycle::H11,
            Self::H12 => HourCycle::H12,
            Self::H23 | Self::H24 => HourCycle::H23,
        }
    }
}

/// Reads the provider's preferred 12/24/default cycle from its raw pattern.
/// This does not naively force H12 onto a locale whose 12-hour pattern uses H11.
pub(super) fn locale_cycle(
    provider: &IntlProvider,
    locale: &LanguageIdentifier,
    attributes: &str,
) -> Result<DateTimeHourCycle, String> {
    use icu_datetime::provider::pattern::PatternItem;
    let locale = DataLocale::from(locale.clone());
    let attributes = DataMarkerAttributes::from_str_or_panic(attributes);
    let data = DataProvider::<DatetimePatternsTimeV1>::load(
        provider,
        DataRequest {
            id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
            ..Default::default()
        },
    )
    .map_err(|error| format!("hour-cycle data: {error:?}"))?;
    let patterns = data.payload.get().to_builder();
    patterns
        .standard
        .short
        .other()
        .items
        .iter()
        .find_map(|item| match item {
            PatternItem::Field(field) => match field.symbol {
                FieldSymbol::Hour(Hour::H11) => Some(DateTimeHourCycle::H11),
                FieldSymbol::Hour(Hour::H12) => Some(DateTimeHourCycle::H12),
                FieldSymbol::Hour(Hour::H23) => Some(DateTimeHourCycle::H23),
                _ => None,
            },
            _ => None,
        })
        .ok_or_else(|| "locale time pattern has no hour field".to_owned())
}
