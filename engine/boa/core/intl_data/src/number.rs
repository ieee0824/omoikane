use crate::StringFields;

/// Localized number symbols in their serialized field order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NumberSymbol {
    /// Effective numbering-system identifier for this payload.
    NumberingSystem,
    /// Localized not-a-number text.
    NaN,
    /// Localized infinity text.
    Infinity,
    /// Positive sign, including directionality controls.
    PlusSign,
    /// Negative sign, including directionality controls.
    MinusSign,
    /// Decimal separator.
    Decimal,
    /// Grouping separator.
    Group,
    /// Currency decimal separator, with the normal separator as default.
    CurrencyDecimal,
    /// Currency grouping separator, with the normal separator as default.
    CurrencyGroup,
    /// Percent symbol.
    PercentSign,
    /// Per-mille symbol.
    PerMille,
    /// Exponent separator.
    Exponential,
    /// Approximate-value symbol.
    ApproximatelySign,
    /// Separator used in digital duration formatting.
    TimeSeparator,
    /// Multiplication symbol for superscript exponents.
    SuperscriptingExponent,
    /// Numeric list separator.
    List,
}

impl NumberSymbol {
    /// Stable field order for generator and provider implementations.
    pub const ALL: [Self; 16] = [
        Self::NumberingSystem,
        Self::NaN,
        Self::Infinity,
        Self::PlusSign,
        Self::MinusSign,
        Self::Decimal,
        Self::Group,
        Self::CurrencyDecimal,
        Self::CurrencyGroup,
        Self::PercentSign,
        Self::PerMille,
        Self::Exponential,
        Self::ApproximatelySign,
        Self::TimeSeparator,
        Self::SuperscriptingExponent,
        Self::List,
    ];

    /// Source CLDR symbol name; the numbering-system identifier is metadata.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NumberingSystem => "numberingSystem",
            Self::NaN => "nan",
            Self::Infinity => "infinity",
            Self::PlusSign => "plusSign",
            Self::MinusSign => "minusSign",
            Self::Decimal => "decimal",
            Self::Group => "group",
            Self::CurrencyDecimal => "currencyDecimal",
            Self::CurrencyGroup => "currencyGroup",
            Self::PercentSign => "percentSign",
            Self::PerMille => "perMille",
            Self::Exponential => "exponential",
            Self::ApproximatelySign => "approximatelySign",
            Self::TimeSeparator => "timeSeparator",
            Self::SuperscriptingExponent => "superscriptingExponent",
            Self::List => "list",
        }
    }
}

/// Zero-copy number symbols for one locale and numbering system.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct NumberSymbols<'data> {
    /// Localized symbols indexed by [`NumberSymbol`].
    #[serde(borrow)]
    pub fields: StringFields<'data>,
}

impl NumberSymbols<'_> {
    /// Returns a symbol without adding directionality or changing whitespace.
    pub fn get(&self, symbol: NumberSymbol) -> Option<&str> {
        self.fields.get(symbol as usize)
    }
}

icu_provider::data_struct!(NumberSymbols<'_>, #[cfg(feature = "datagen")]);
icu_provider::data_marker!(
    /// Locale-specific number symbols. Attributes are numbering systems;
    /// empty attributes select the locale's default numbering system.
    OmoikaneNumberSymbolsV1, NumberSymbols<'static>,
);
icu_provider::data_marker!(
    /// Global CLDR root symbols for all numeric numbering systems.
    /// Requests use locale `und` and numbering-system attributes.
    OmoikaneGlobalNumberSymbolsV1, NumberSymbols<'static>,
);

/// Raw CLDR number pattern fields in serialized order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum NumberPatternField {
    /// Standard decimal pattern.
    Decimal,
    /// Percent pattern, preserving locale-specific affix placement.
    Percent,
    /// Scientific pattern.
    Scientific,
}

impl NumberPatternField {
    /// Stable field order for custom providers and generators.
    pub const ALL: [Self; 3] = [Self::Decimal, Self::Percent, Self::Scientific];

    /// Corresponding source CLDR format family.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Decimal => "decimal",
            Self::Percent => "percent",
            Self::Scientific => "scientific",
        }
    }
}

/// Zero-copy number affix patterns for one locale and numbering system.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct NumberPatterns<'data> {
    /// Raw standard decimal, percent and scientific patterns.
    #[serde(borrow)]
    pub fields: StringFields<'data>,
}

impl NumberPatterns<'_> {
    /// Returns an exact CLDR pattern, including quotes and sign literals.
    pub fn get(&self, field: NumberPatternField) -> Option<&str> {
        self.fields.get(field as usize)
    }
}

icu_provider::data_struct!(NumberPatterns<'_>, #[cfg(feature = "datagen")]);
icu_provider::data_marker!(
    /// Number patterns with numbering-system attributes; empty attributes select locale default.
    OmoikaneNumberPatternsV1, NumberPatterns<'static>,
);
icu_provider::data_marker!(
    /// Global CLDR root number patterns with locale `und` and numbering-system attributes.
    OmoikaneGlobalNumberPatternsV1, NumberPatterns<'static>,
);
