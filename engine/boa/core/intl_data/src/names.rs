use zerovec::ZeroMap;

/// Raw CLDR relative date fields for one locale, including every style.
///
/// Keys have `unit/style/auto/value` or `unit/style/tense/category` form.
/// Missing style records are resolved by the consumer, never fabricated.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
// ZeroMap's associated storage types obscure covariance. Use its Yokeable
// lifetime projection, matching the existing calendar and compact schemas.
#[yoke(prove_covariance_manually)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct RelativeTimePatterns<'data> {
    /// Zero-copy exact patterns and relative labels keyed by their source fields.
    #[serde(borrow)]
    pub records: ZeroMap<'data, str, str>,
}

icu_provider::data_struct!(RelativeTimePatterns<'_>, #[cfg(feature = "datagen")]);
icu_provider::data_marker!(
    /// Relative time fields for the complete bundled CLDR locale closure.
    OmoikaneRelativeTimePatternsV1, RelativeTimePatterns<'static>,
);

/// Raw CLDR locale display names and composition patterns for one locale.
///
/// Keys use `type/style/code`; composition patterns use `pattern/name`.
/// A missing label remains missing so consumers can implement `fallback`.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
// The only borrowed field projects through ZeroMap's existing Yokeable impl.
#[yoke(prove_covariance_manually)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct DisplayNamesData<'data> {
    /// Zero-copy exact labels; no runtime JSON or owned string map is required.
    #[serde(borrow)]
    pub records: ZeroMap<'data, str, str>,
}

icu_provider::data_struct!(DisplayNamesData<'_>, #[cfg(feature = "datagen")]);
icu_provider::data_marker!(
    /// Localized names for languages, regions, scripts, currencies and date fields.
    OmoikaneDisplayNamesV1, DisplayNamesData<'static>,
);
