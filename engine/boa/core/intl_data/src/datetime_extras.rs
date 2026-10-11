use crate::StringFields;
use zerovec::ZeroMap2d;

/// Raw CLDR date, time and combination styles for one locale and calendar.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
#[yoke(prove_covariance_manually)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct DateTimeStylePatterns<'data> {
    /// Family (`date`, `time`, `glue`, `atTime`) / source style / raw pattern.
    #[serde(borrow)]
    pub patterns: ZeroMap2d<'data, str, str, str>,
    /// Source `_numbers` overrides, indexed by the same family and style.
    #[serde(borrow)]
    pub numbering_overrides: ZeroMap2d<'data, str, str, str>,
}

impl DateTimeStylePatterns<'_> {
    /// Gets an exact style pattern, preserving quoting and source variants.
    pub fn get(&self, family: &str, style: &str) -> Option<&str> {
        self.patterns.get_2d(family, style)
    }
}

icu_provider::data_struct!(DateTimeStylePatterns<'_>, #[cfg(feature = "datagen")]);
icu_provider::data_marker!(
    /// CLDR date/time styles. Attributes are Unicode calendar identifiers.
    OmoikaneDateTimeStylePatternsV1, DateTimeStylePatterns<'static>,
);

/// Flexible day-period names and source time bounds for one locale.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
#[yoke(prove_covariance_manually)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct DayPeriods<'data> {
    /// CLDR format width / period identifier / localized name.
    #[serde(borrow)]
    pub names: ZeroMap2d<'data, str, str, str>,
    /// Period identifier / source bound (`_at`, `_from`, `_before`, etc.) / time.
    #[serde(borrow)]
    pub rules: ZeroMap2d<'data, str, str, str>,
    /// The locale supplying the inherited supplemental rules, at index zero.
    #[serde(borrow)]
    pub fields: StringFields<'data>,
}

impl DayPeriods<'_> {
    /// Gets a source name without imposing runtime fallback or midnight policy.
    pub fn name(&self, width: &str, period: &str) -> Option<&str> {
        self.names.get_2d(width, period)
    }

    /// Returns the source locale selected for the supplemental rule set.
    pub fn source_rule_locale(&self) -> Option<&str> {
        self.fields.get(0)
    }
}

icu_provider::data_struct!(DayPeriods<'_>, #[cfg(feature = "datagen")]);
icu_provider::data_marker!(
    /// CLDR localized flexible day-period names and rules. Attributes are empty.
    OmoikaneDayPeriodsV1, DayPeriods<'static>,
);
