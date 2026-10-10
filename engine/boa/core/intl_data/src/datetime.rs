use crate::StringFields;
use zerovec::{ZeroMap, ZeroMap2d};

/// CLDR available-format patterns for one locale and calendar.
///
/// Skeleton keys, pattern fields, quotes and literals are preserved unchanged.
/// This schema owns its provider payload; its maps borrow only the payload's
/// backing buffer, rather than another formatter or mutable runtime state.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
// ZeroMap's associated storage types obscure covariance. Prove the lifetime
// projection using each field's existing Yokeable implementation, as in ICU4X.
#[yoke(prove_covariance_manually)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct CalendarDatePatterns<'data> {
    /// Exact CLDR skeleton to localized raw pattern, including source variants.
    #[serde(borrow)]
    pub available_formats: ZeroMap<'data, str, str>,
}

impl CalendarDatePatterns<'_> {
    /// Returns an exact source skeleton without changing calendar fields.
    pub fn get(&self, skeleton: &str) -> Option<&str> {
        self.available_formats.get(skeleton)
    }
}

icu_provider::data_struct!(CalendarDatePatterns<'_>, #[cfg(feature = "datagen")]);
icu_provider::data_marker!(
    /// Calendar-specific CLDR availableFormats. Attributes are Unicode calendar
    /// identifiers, for example `gregory`, `chinese`, `ethioaa` or `islamic-civil`.
    /// Locale fallback is provided by the embedder, not the payload schema.
    OmoikaneCalendarDatePatternsV1, CalendarDatePatterns<'static>,
);

/// Complete raw CLDR interval patterns for one locale and calendar.
///
/// Source skeletons, greatest-difference keys (including variants), quoting and
/// fallback placeholder order are retained. The consumer selects and compiles
/// patterns; this payload does not embed formatting or locale-fallback policy.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
#[yoke(prove_covariance_manually)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct CalendarIntervalPatterns<'data> {
    /// Raw skeleton / greatest-difference key / pattern map.
    #[serde(borrow)]
    pub patterns: ZeroMap2d<'data, str, str, str>,
    /// The exact locale fallback pattern at index zero.
    #[serde(borrow)]
    pub fields: StringFields<'data>,
}

impl CalendarIntervalPatterns<'_> {
    /// Gets an exact source pattern without applying runtime matching policy.
    pub fn get(&self, skeleton: &str, difference: &str) -> Option<&str> {
        self.patterns.get_2d(skeleton, difference)
    }

    /// Returns the source fallback, preserving either placeholder order.
    pub fn fallback(&self) -> Option<&str> {
        self.fields.get(0)
    }

    /// Iterates the exact stored source skeletons in lexical order.
    pub fn skeletons(&self) -> impl Iterator<Item = &str> + '_ {
        self.patterns.iter0().map(|cursor| cursor.key0())
    }
}

icu_provider::data_struct!(CalendarIntervalPatterns<'_>, #[cfg(feature = "datagen")]);
icu_provider::data_marker!(
    /// Raw calendar interval data. Attributes use the same Unicode calendar
    /// identifiers as OmoikaneCalendarDatePatternsV1. All source variants remain
    /// available; locale fallback is provided by the embedder.
    OmoikaneCalendarIntervalPatternsV1, CalendarIntervalPatterns<'static>,
);
