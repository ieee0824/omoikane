use crate::{PluralPatterns, StringFields};

/// ECMA-402 sanctioned simple unit identifiers and their CLDR unit taxonomy IDs.
///
/// Entries are sorted by the ECMA identifier so runtime option validation can
/// use binary search. Keep all consumers on this table: arbitrary sanctioned
/// compounds are composed from two simple-unit records.
pub const SANCTIONED_SIMPLE_UNIT_IDENTIFIERS: &[(&str, &str)] = &[
    ("acre", "area-acre"),
    ("bit", "digital-bit"),
    ("byte", "digital-byte"),
    ("celsius", "temperature-celsius"),
    ("centimeter", "length-centimeter"),
    ("day", "duration-day"),
    ("degree", "angle-degree"),
    ("fahrenheit", "temperature-fahrenheit"),
    ("fluid-ounce", "volume-fluid-ounce"),
    ("foot", "length-foot"),
    ("gallon", "volume-gallon"),
    ("gigabit", "digital-gigabit"),
    ("gigabyte", "digital-gigabyte"),
    ("gram", "mass-gram"),
    ("hectare", "area-hectare"),
    ("hour", "duration-hour"),
    ("inch", "length-inch"),
    ("kilobit", "digital-kilobit"),
    ("kilobyte", "digital-kilobyte"),
    ("kilogram", "mass-kilogram"),
    ("kilometer", "length-kilometer"),
    ("liter", "volume-liter"),
    ("megabit", "digital-megabit"),
    ("megabyte", "digital-megabyte"),
    ("meter", "length-meter"),
    ("microsecond", "duration-microsecond"),
    ("mile", "length-mile"),
    ("mile-scandinavian", "length-mile-scandinavian"),
    ("milliliter", "volume-milliliter"),
    ("millimeter", "length-millimeter"),
    ("millisecond", "duration-millisecond"),
    ("minute", "duration-minute"),
    ("month", "duration-month"),
    ("nanosecond", "duration-nanosecond"),
    ("ounce", "mass-ounce"),
    ("percent", "concentr-percent"),
    ("petabyte", "digital-petabyte"),
    ("pound", "mass-pound"),
    ("second", "duration-second"),
    ("stone", "mass-stone"),
    ("terabit", "digital-terabit"),
    ("terabyte", "digital-terabyte"),
    ("week", "duration-week"),
    ("yard", "length-yard"),
    ("year", "duration-year"),
];

/// Sanctioned compound identifiers with a directly translated CLDR record.
///
/// Other sanctioned compounds are composed from two simple-unit records.
pub const CANONICAL_COMPOUND_UNIT_IDENTIFIERS: &[(&str, &str)] =
    &[("kilometer-per-hour", "speed-kilometer-per-hour")];

/// Returns the canonical sanctioned identifier and CLDR taxonomy ID.
pub fn sanctioned_simple_unit(identifier: &str) -> Option<(&'static str, &'static str)> {
    SANCTIONED_SIMPLE_UNIT_IDENTIFIERS
        .binary_search_by(|(candidate, _)| candidate.cmp(&identifier))
        .ok()
        .map(|index| SANCTIONED_SIMPLE_UNIT_IDENTIFIERS[index])
}

/// Returns the directly translated CLDR taxonomy ID for a compound identifier.
pub fn canonical_compound_unit_cldr_id(identifier: &str) -> Option<&'static str> {
    CANONICAL_COMPOUND_UNIT_IDENTIFIERS
        .iter()
        .find_map(|&(candidate, cldr)| (candidate == identifier).then_some(cldr))
}

/// Whether a CLDR unit record can be reached by the Intl runtime.
pub fn is_reachable_unit_cldr_id(identifier: &str) -> bool {
    SANCTIONED_SIMPLE_UNIT_IDENTIFIERS
        .iter()
        .chain(CANONICAL_COMPOUND_UNIT_IDENTIFIERS)
        .any(|&(_, cldr)| cldr == identifier)
}

/// Width of localized unit labels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitWidth {
    /// Full unit names.
    Long,
    /// Abbreviated unit labels.
    Short,
    /// Narrow labels, often without separating whitespace.
    Narrow,
}

impl UnitWidth {
    /// CLDR width and provider attribute prefix.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Long => "long",
            Self::Short => "short",
            Self::Narrow => "narrow",
        }
    }
}

/// Localized number/unit patterns and per-unit overrides.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct UnitPatterns<'data> {
    /// Raw plural patterns; a present pattern may deliberately omit `{0}`.
    #[serde(borrow)]
    pub patterns: PluralPatterns<'data>,
    /// Field 0 is displayName; field 1 is perUnitPattern.
    #[serde(borrow)]
    pub fields: StringFields<'data>,
}

impl UnitPatterns<'_> {
    /// Uncounted localized display name.
    pub fn display_name(&self) -> Option<&str> {
        self.fields.get(0)
    }
    /// Denominator-specific pattern, including its placement and whitespace.
    pub fn per_unit_pattern(&self) -> Option<&str> {
        self.fields.get(1)
    }
}

/// Locale/width patterns for composing compound units.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct CompoundUnitPatterns<'data> {
    /// Field 0 is the per pattern, field 1 is the multiplication pattern.
    #[serde(borrow)]
    pub fields: StringFields<'data>,
}

impl CompoundUnitPatterns<'_> {
    /// Raw CLDR pattern with `{0}` numerator and `{1}` denominator.
    pub fn per_pattern(&self) -> Option<&str> {
        self.fields.get(0)
    }
    /// Raw CLDR multiplication pattern.
    pub fn times_pattern(&self) -> Option<&str> {
        self.fields.get(1)
    }
}

/// Digital duration fields in their serialized order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DurationDigitalField {
    /// Locale/numbering-system separator, with global root data as fallback.
    TimeSeparator,
    /// Effective numbering-system identifier.
    NumberingSystem,
}

/// Digital duration patterns for one locale and numbering system.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct DurationDigital<'data> {
    /// Resolved separator and numbering-system identifier.
    #[serde(borrow)]
    pub fields: StringFields<'data>,
}

impl DurationDigital<'_> {
    /// Returns the separator or numbering-system identifier.
    pub fn get(&self, field: DurationDigitalField) -> Option<&str> {
        self.fields.get(field as usize)
    }
}

icu_provider::data_struct!(UnitPatterns<'_>, #[cfg(feature = "datagen")]);
icu_provider::data_struct!(CompoundUnitPatterns<'_>, #[cfg(feature = "datagen")]);
icu_provider::data_struct!(DurationDigital<'_>, #[cfg(feature = "datagen")]);
icu_provider::data_marker!(
    /// Unit labels. Attributes combine width and CLDR unit ID, e.g. `long-duration-second`.
    OmoikaneUnitPatternsV1, UnitPatterns<'static>,
);
icu_provider::data_marker!(
    /// Compound-unit patterns with width attributes `long`, `short`, or `narrow`.
    OmoikaneCompoundUnitPatternsV1, CompoundUnitPatterns<'static>,
);
icu_provider::data_marker!(
    /// Digital-duration patterns with numbering-system attributes; empty selects locale default.
    OmoikaneDurationDigitalV1, DurationDigital<'static>,
);
