use crate::{PluralPatterns, StringFields};
use zerovec::ZeroMap;

/// Currency pattern and spacing fields in serialized order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CurrencyPatternField {
    /// Standard positive and optional negative subpatterns.
    Standard,
    /// Accounting positive and optional negative subpatterns.
    Accounting,
    /// Standard variant for alphabetic symbols adjacent to numbers.
    StandardAlphaNextToNumber,
    /// Accounting variant for alphabetic symbols adjacent to numbers.
    AccountingAlphaNextToNumber,
    /// Standard pattern without a repeated currency marker.
    StandardNoCurrency,
    /// Accounting pattern without a repeated currency marker.
    AccountingNoCurrency,
    /// Pattern for appending the ISO code.
    AppendIso,
    /// CLDR beforeCurrency.currencyMatch UnicodeSet rule.
    BeforeCurrencyMatch,
    /// CLDR beforeCurrency.surroundingMatch UnicodeSet rule.
    BeforeSurroundingMatch,
    /// Literal inserted by the beforeCurrency spacing rule.
    BeforeInsertBetween,
    /// CLDR afterCurrency.currencyMatch UnicodeSet rule.
    AfterCurrencyMatch,
    /// CLDR afterCurrency.surroundingMatch UnicodeSet rule.
    AfterSurroundingMatch,
    /// Literal inserted by the afterCurrency spacing rule.
    AfterInsertBetween,
}

impl CurrencyPatternField {
    /// Stable order for source conversion and custom providers.
    pub const ALL: [Self; 13] = [
        Self::Standard,
        Self::Accounting,
        Self::StandardAlphaNextToNumber,
        Self::AccountingAlphaNextToNumber,
        Self::StandardNoCurrency,
        Self::AccountingNoCurrency,
        Self::AppendIso,
        Self::BeforeCurrencyMatch,
        Self::BeforeSurroundingMatch,
        Self::BeforeInsertBetween,
        Self::AfterCurrencyMatch,
        Self::AfterSurroundingMatch,
        Self::AfterInsertBetween,
    ];
}

/// Currency patterns for one locale and numbering system.
///
/// Raw CLDR patterns preserve quotes, bidi controls, positive/negative placement,
/// and alpha variants. Intl algorithms can parse these during initialization.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct CurrencyPatterns<'data> {
    /// Affix patterns and currency spacing rules.
    #[serde(borrow)]
    pub fields: StringFields<'data>,
    /// Patterns used to join a number and a localized currency name.
    #[serde(borrow)]
    pub unit_patterns: PluralPatterns<'data>,
}

impl CurrencyPatterns<'_> {
    /// Returns an exact CLDR pattern or spacing field.
    pub fn get(&self, field: CurrencyPatternField) -> Option<&str> {
        self.fields.get(field as usize)
    }
}

/// Currency display text and currency-specific formatting overrides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CurrencyTextField {
    /// Localized standard symbol when CLDR supplies one.
    Symbol,
    /// Narrow symbol when CLDR supplies one.
    NarrowSymbol,
    /// Formal symbol variant when CLDR supplies it.
    FormalSymbol,
    /// Alternate symbol variant when CLDR supplies it.
    VariantSymbol,
    /// Uncounted display name.
    DisplayName,
    /// Currency-specific decimal separator override.
    DecimalOverride,
    /// Currency-specific grouping separator override.
    GroupOverride,
    /// Currency-specific positive/negative pattern override.
    PatternOverride,
}

/// Localized symbols and names for one currency.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct CurrencyText<'data> {
    /// Symbols, names, and optional overrides indexed by [`CurrencyTextField`].
    #[serde(borrow)]
    pub fields: StringFields<'data>,
    /// Localized counted names indexed by plural category.
    #[serde(borrow)]
    pub plural_names: PluralPatterns<'data>,
}

impl CurrencyText<'_> {
    /// Returns a display string or currency-specific override.
    pub fn get(&self, field: CurrencyTextField) -> Option<&str> {
        self.fields.get(field as usize)
    }
}

/// Normal and cash precision metadata. Intl NumberFormat uses normal digits.
#[derive(Clone, Debug, PartialEq, yoke::Yokeable, zerofrom::ZeroFrom, serde::Deserialize)]
// ZeroMap's associated storage types make its Rust lifetime invariant. Each
// field already implements Yokeable; derive proves the projection field by
// field, as ICU4X does for payloads containing ZeroMap, rather than coercing
// the enclosing struct's lifetime.
#[yoke(prove_covariance_manually)]
#[cfg_attr(feature = "datagen", derive(serde::Serialize, databake::Bake))]
#[cfg_attr(feature = "datagen", databake(path = boa_intl_data))]
pub struct CurrencyDigits<'data> {
    /// Default normal precision for unknown well-formed currency codes.
    pub default_digits: u8,
    /// Normal fraction digits, including the CLDR `DEFAULT` record.
    #[serde(borrow)]
    pub normal_digits: ZeroMap<'data, str, u8>,
    /// Normal rounding metadata; separate from Intl roundingIncrement options.
    #[serde(borrow)]
    pub normal_rounding: ZeroMap<'data, str, u16>,
    /// Explicit cash precision overrides; never used automatically by NumberFormat.
    #[serde(borrow)]
    pub cash_digits: ZeroMap<'data, str, u8>,
    /// Explicit cash rounding increments.
    #[serde(borrow)]
    pub cash_rounding: ZeroMap<'data, str, u16>,
}

impl CurrencyDigits<'_> {
    /// Returns normal fraction digits, falling back to the CLDR default.
    pub fn digits(&self, currency: &str) -> u8 {
        self.normal_digits
            .get_copied(currency)
            .unwrap_or(self.default_digits)
    }
}

icu_provider::data_struct!(CurrencyPatterns<'_>, #[cfg(feature = "datagen")]);
icu_provider::data_struct!(CurrencyText<'_>, #[cfg(feature = "datagen")]);
icu_provider::data_struct!(CurrencyDigits<'_>, #[cfg(feature = "datagen")]);
icu_provider::data_marker!(
    /// Currency patterns with numbering-system attributes; empty attributes select locale default.
    OmoikaneCurrencyPatternsV1, CurrencyPatterns<'static>,
);
icu_provider::data_marker!(
    /// CLDR root currency patterns. Requests use locale `und` and numbering-system attributes.
    OmoikaneGlobalCurrencyPatternsV1, CurrencyPatterns<'static>,
);
icu_provider::data_marker!(
    /// Currency symbols/names with uppercase three-letter ISO-code attributes.
    OmoikaneCurrencyTextV1, CurrencyText<'static>,
);
icu_provider::data_marker!(
    /// Singleton normal and cash currency precision data.
    OmoikaneCurrencyDigitsV1, CurrencyDigits<'static>, is_singleton = true,
);
