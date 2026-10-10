//! Pure numeric formatting shared by native Intl services.

use std::sync::Arc;

use boa_intl_data::{
    NumberPatternField, NumberSymbol, OmoikaneGlobalNumberPatternsV1,
    OmoikaneGlobalNumberSymbolsV1, OmoikaneNumberPatternsV1, OmoikaneNumberSymbolsV1,
    PluralCategory,
};
use fixed_decimal::{Decimal, Sign, SignDisplay, SignedRoundingMode};
use icu_decimal::{
    DecimalFormatter,
    options::{DecimalFormatterOptions, GroupingStrategy},
    preferences::NumberingSystem,
};
use icu_locale::{Locale, extensions::unicode::Value};
use icu_plurals::{PluralCategory as IcuPluralCategory, PluralRules};
use icu_provider::DataPayload;
use writeable::Writeable;

use super::{
    compact_pattern::CompactPattern,
    currency::CurrencyFormatter,
    data,
    input::MathematicalValue,
    notation::NativeNotation,
    options::*,
    output::{DecimalWriter, NumberOutput, NumberText, push_symbol},
    parts::NumberPart,
    pattern::NumberPattern,
    units::UnitFormatter,
};
use crate::{JsNativeError, JsResult, context::icu::IntlProvider};

/// Immutable digit and sign options, without JavaScript property access.
#[derive(Clone, Debug)]
pub(crate) struct NumericFormatOptions {
    pub(crate) digits: DigitFormatOptions,
    pub(crate) sign_display: SignDisplay,
}

impl NumericFormatOptions {
    /// Constructs fraction precision for an internal exact-decimal caller.
    pub(crate) fn fraction(
        min_integer: u8,
        min_fraction: u8,
        max_fraction: u8,
        rounding_mode: SignedRoundingMode,
        sign_display: SignDisplay,
    ) -> Self {
        Self {
            digits: DigitFormatOptions {
                minimum_integer_digits: min_integer,
                rounding_increment: RoundingIncrement::from_u16(1).expect("valid increment"),
                rounding_mode,
                trailing_zero_display: TrailingZeroDisplay::Auto,
                rounding_type: RoundingType::FractionDigits(Extrema {
                    minimum: min_fraction,
                    maximum: max_fraction,
                }),
                rounding_priority: RoundingPriority::Auto,
            },
            sign_display,
        }
    }
}

#[derive(Debug)]
enum NativeStyle {
    Decimal,
    Percent(Option<UnitFormatter>),
    Currency(CurrencyFormatter),
    Unit(UnitFormatter),
}

impl NativeStyle {
    fn new(
        provider: &IntlProvider,
        locale: &Locale,
        attributes: &str,
        options: &UnitFormatOptions,
        notation: Notation,
    ) -> JsResult<Self> {
        Ok(match options {
            UnitFormatOptions::Decimal => Self::Decimal,
            UnitFormatOptions::Percent => {
                let unit = if let Notation::Compact { display } = notation {
                    let display = if display == CompactDisplay::Long {
                        UnitDisplay::Long
                    } else {
                        UnitDisplay::Short
                    };
                    Some(UnitFormatter::new(
                        provider,
                        locale,
                        &"percent".parse().expect("sanctioned percent unit"),
                        display,
                    )?)
                } else {
                    None
                };
                Self::Percent(unit)
            }
            UnitFormatOptions::Currency {
                currency,
                display,
                sign,
            } => Self::Currency(CurrencyFormatter::new(
                provider, locale, attributes, *currency, *display, *sign,
            )?),
            UnitFormatOptions::Unit { unit, display } => {
                Self::Unit(UnitFormatter::new(provider, locale, unit, *display)?)
            }
        })
    }
}

/// Immutable numeric data shared by formatters that differ only in their unit style.
#[derive(Debug)]
struct NativeNumberCore {
    formatter: DecimalFormatter,
    plurals: Option<PluralRules>,
    symbols: DataPayload<OmoikaneNumberSymbolsV1>,
    pattern: NumberPattern,
    notation: NativeNotation,
}

/// Shared standard-decimal state used by internal compound formatters.
#[derive(Clone, Debug)]
pub(crate) struct SharedNumberFormatCore(Arc<NativeNumberCore>);

/// Owns a unit style and shares immutable numeric data where the caller can.
#[derive(Debug)]
pub(crate) struct NativeNumberFormatter {
    core: Arc<NativeNumberCore>,
    plural_usage: PluralUsage,
    style: NativeStyle,
}

/// Records the two independent places where number formatting needs plural rules.
#[derive(Clone, Copy, Debug)]
struct PluralUsage {
    compact: bool,
    grammar: bool,
}

impl PluralUsage {
    fn new(style: &UnitFormatOptions, notation: Notation) -> Self {
        let compact = matches!(notation, Notation::Compact { .. });
        Self {
            compact,
            grammar: matches!(
                style,
                UnitFormatOptions::Unit { .. }
                    | UnitFormatOptions::Currency {
                        display: CurrencyDisplay::Name,
                        ..
                    }
            ) || compact && matches!(style, UnitFormatOptions::Percent),
        }
    }

    const fn any(self) -> bool {
        self.compact || self.grammar
    }
}

/// One scalar result retains the rounded value's grammatical category.
#[derive(Debug)]
pub(super) struct NumberEndpoint {
    pub(super) parts: Vec<NumberPart>,
    pub(super) category: PluralCategory,
}

impl NativeNumberFormatter {
    /// Loads immutable locale data once. No provider lookup occurs in `format`.
    pub(crate) fn new(
        provider: &IntlProvider,
        locale: &Locale,
        numbering_system: Option<&str>,
        style: &UnitFormatOptions,
        grouping: GroupingStrategy,
    ) -> JsResult<Self> {
        Self::new_with_notation(
            provider,
            locale,
            numbering_system,
            style,
            grouping,
            Notation::Standard,
        )
    }

    /// Initializes NumberFormat notation while internal Duration callers remain standard.
    pub(super) fn new_with_notation(
        provider: &IntlProvider,
        locale: &Locale,
        numbering_system: Option<&str>,
        style: &UnitFormatOptions,
        grouping: GroupingStrategy,
        notation: Notation,
    ) -> JsResult<Self> {
        let plural_usage = PluralUsage::new(style, notation);
        let attributes = numbering_system.unwrap_or_default();
        let symbols = data::locale_or_global::<
            OmoikaneNumberSymbolsV1,
            OmoikaneGlobalNumberSymbolsV1,
        >(provider, locale, attributes)?;
        validate_symbols(symbols.get(), matches!(style, UnitFormatOptions::Percent))?;
        let patterns = data::locale_or_global::<
            OmoikaneNumberPatternsV1,
            OmoikaneGlobalNumberPatternsV1,
        >(provider, locale, attributes)?;
        let field = if matches!(style, UnitFormatOptions::Percent)
            && !matches!(notation, Notation::Compact { .. })
        {
            NumberPatternField::Percent
        } else {
            NumberPatternField::Decimal
        };
        let raw = patterns
            .get()
            .get(field)
            .ok_or_else(|| JsNativeError::typ().with_message("missing CLDR number pattern"))?;
        let pattern = NumberPattern::parse(raw)?;
        let effective = symbols
            .get()
            .get(NumberSymbol::NumberingSystem)
            .expect("validated numbering system");
        let formatter = decimal_formatter(provider, locale, effective, grouping)?;
        let native_style = NativeStyle::new(provider, locale, attributes, style, notation)?;
        let notation = NativeNotation::new(
            provider,
            locale,
            effective,
            notation,
            style,
            patterns.get().get(NumberPatternField::Scientific),
            symbols.get(),
        )?;
        let plurals = if plural_usage.any() {
            Some(
                PluralRules::try_new_cardinal_with_buffer_provider(
                    provider.erased_provider(),
                    locale.into(),
                )
                .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?,
            )
        } else {
            None
        };
        Ok(Self {
            core: Arc::new(NativeNumberCore {
                formatter,
                plurals,
                symbols,
                pattern,
                notation,
            }),
            plural_usage,
            style: native_style,
        })
    }

    /// Loads one immutable standard-decimal core for internal unit formatters.
    ///
    /// This deliberately excludes percent, currency and non-standard notation,
    /// whose numeric patterns and notation state can depend on their style.
    pub(crate) fn new_shared_decimal_core(
        provider: &IntlProvider,
        locale: &Locale,
        numbering_system: Option<&str>,
        grouping: GroupingStrategy,
        needs_plural_rules: bool,
    ) -> JsResult<SharedNumberFormatCore> {
        let attributes = numbering_system.unwrap_or_default();
        let symbols = data::locale_or_global::<
            OmoikaneNumberSymbolsV1,
            OmoikaneGlobalNumberSymbolsV1,
        >(provider, locale, attributes)?;
        validate_symbols(symbols.get(), false)?;
        let patterns = data::locale_or_global::<
            OmoikaneNumberPatternsV1,
            OmoikaneGlobalNumberPatternsV1,
        >(provider, locale, attributes)?;
        let raw = patterns
            .get()
            .get(NumberPatternField::Decimal)
            .ok_or_else(|| JsNativeError::typ().with_message("missing CLDR number pattern"))?;
        let pattern = NumberPattern::parse(raw)?;
        let effective = symbols
            .get()
            .get(NumberSymbol::NumberingSystem)
            .expect("validated numbering system");
        let formatter = decimal_formatter(provider, locale, effective, grouping)?;
        let plurals = if needs_plural_rules {
            Some(
                PluralRules::try_new_cardinal_with_buffer_provider(
                    provider.erased_provider(),
                    locale.into(),
                )
                .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?,
            )
        } else {
            None
        };
        Ok(SharedNumberFormatCore(Arc::new(NativeNumberCore {
            formatter,
            plurals,
            symbols,
            pattern,
            notation: NativeNotation::Standard,
        })))
    }

    /// Adds a decimal or unit style to an existing standard-decimal core.
    pub(crate) fn from_shared_decimal_core(
        provider: &IntlProvider,
        locale: &Locale,
        numbering_system: Option<&str>,
        style: &UnitFormatOptions,
        core: &SharedNumberFormatCore,
    ) -> JsResult<Self> {
        if !matches!(
            style,
            UnitFormatOptions::Decimal | UnitFormatOptions::Unit { .. }
        ) {
            return Err(JsNativeError::typ()
                .with_message("shared decimal core requires decimal or unit style")
                .into());
        }
        let plural_usage = PluralUsage::new(style, Notation::Standard);
        if plural_usage.any() && core.0.plurals.is_none() {
            return Err(JsNativeError::typ()
                .with_message("shared unit core requires plural rules")
                .into());
        }
        let native_style = NativeStyle::new(
            provider,
            locale,
            numbering_system.unwrap_or_default(),
            style,
            Notation::Standard,
        )?;
        Ok(Self {
            core: Arc::clone(&core.0),
            plural_usage,
            style: native_style,
        })
    }

    /// Effective numbering system from the same payload used for formatting.
    pub(crate) fn numbering_system(&self) -> &str {
        self.core
            .symbols
            .get()
            .get(NumberSymbol::NumberingSystem)
            .expect("validated numbering system")
    }

    /// Range approximation uses the same resolved numbering-system symbols.
    pub(super) fn approximately_sign(&self) -> Option<&str> {
        self.core.symbols.get().get(NumberSymbol::ApproximatelySign)
    }

    /// Rounds an exact value and emits ICU numeric parts and typed CLDR affixes.
    pub(crate) fn format(
        &self,
        value: MathematicalValue,
        options: &NumericFormatOptions,
    ) -> Vec<NumberPart> {
        self.format_endpoint(value, options, None).parts
    }

    /// Uses the shared numeric decisions while writing no typed part containers.
    pub(crate) fn format_text(
        &self,
        value: MathematicalValue,
        options: &NumericFormatOptions,
    ) -> String {
        self.format_output::<NumberText>(value, options, None)
            .0
            .into_string()
    }

    /// A range may reselect unit/name grammar without changing numeric rounding
    /// or the compact mantissa's pattern and explicit-value selection.
    pub(super) fn format_endpoint(
        &self,
        value: MathematicalValue,
        options: &NumericFormatOptions,
        category_override: Option<PluralCategory>,
    ) -> NumberEndpoint {
        let (parts, category) = self.format_output(value, options, category_override);
        NumberEndpoint { parts, category }
    }

    fn format_output<O: NumberOutput>(
        &self,
        value: MathematicalValue,
        options: &NumericFormatOptions,
        category_override: Option<PluralCategory>,
    ) -> (O, PluralCategory) {
        let (number, sign, category) = match value {
            MathematicalValue::Finite(value) => {
                return self.format_finite(value, options, category_override);
            }
            MathematicalValue::NaN => {
                self.nonfinite(NumberSymbol::NaN, "nan", false, true, options.sign_display)
            }
            MathematicalValue::Infinity { negative } => self.nonfinite(
                NumberSymbol::Infinity,
                "infinity",
                negative,
                false,
                options.sign_display,
            ),
        };
        (
            self.finish(
                number,
                sign,
                category_override.unwrap_or(category),
                None,
                None,
            ),
            category,
        )
    }

    fn format_finite<O: NumberOutput>(
        &self,
        mut value: Decimal,
        options: &NumericFormatOptions,
        category_override: Option<PluralCategory>,
    ) -> (O, PluralCategory) {
        if matches!(self.style, NativeStyle::Percent(_)) {
            value.multiply_pow10(2);
        }
        let selected = self.core.notation.prepare(&mut value, &options.digits);
        let compact_category = if self.plural_usage.compact {
            plural_category(
                self.core
                    .plurals
                    .as_ref()
                    .expect("compact notation loaded plural rules")
                    .category_for(&value),
            )
        } else {
            PluralCategory::Other
        };
        // Explicit patterns use the rounded mathematical sign before signDisplay.
        let compact = self
            .core
            .notation
            .compact_pattern(selected, &value, compact_category, false);
        let alpha = self
            .core
            .notation
            .compact_pattern(selected, &value, compact_category, true);
        let category = if self.plural_usage.grammar {
            plural_category(
                self.core
                    .plurals
                    .as_ref()
                    .expect("unit grammar loaded plural rules")
                    .category_for(self.core.notation.operands(&value, selected.exponent)),
            )
        } else {
            PluralCategory::Other
        };
        value.apply_sign_display(options.sign_display);
        let sign = value.sign();
        value.set_sign(Sign::None);
        let number = self.decimal_output(&value);
        let number = self
            .core
            .notation
            .render(number, selected.exponent, self.core.symbols.get());
        (
            self.finish(
                number,
                sign,
                category_override.unwrap_or(category),
                compact,
                alpha,
            ),
            category,
        )
    }

    fn decimal_output<O: NumberOutput>(&self, value: &Decimal) -> O {
        let symbols = self.core.symbols.get();
        let separator = |symbol| match &self.style {
            NativeStyle::Currency(currency) => currency.separator(symbol, symbols),
            _ => symbols.get(symbol).expect("validated numeric symbol"),
        };
        let mut writer: DecimalWriter<'_, O> = DecimalWriter::new(
            Some(separator(NumberSymbol::Decimal)),
            Some(separator(NumberSymbol::Group)),
            false,
        );
        let formatted = self.core.formatter.format(value);
        writer
            .output
            .reserve_text(formatted.writeable_length_hint().capacity());
        formatted
            .write_to_parts(&mut writer)
            .expect("writing to String cannot fail");
        writer.output
    }

    #[cfg(test)]
    pub(crate) fn has_plural_rules(&self) -> bool {
        self.core.plurals.is_some()
    }

    #[cfg(test)]
    pub(crate) fn shared_core_id(&self) -> usize {
        Arc::as_ptr(&self.core) as usize
    }

    fn finish<O: NumberOutput>(
        &self,
        number: O,
        sign: Sign,
        category: PluralCategory,
        compact: Option<&CompactPattern>,
        alpha: Option<&CompactPattern>,
    ) -> O {
        let symbols = self.core.symbols.get();
        if let NativeStyle::Currency(currency) = &self.style {
            return if let Some(pattern) = compact {
                currency.format_compact(number, sign, category, symbols, pattern, alpha)
            } else {
                currency.format(number, sign, category, symbols, &self.core.pattern)
            };
        }
        let number = if let Some(pattern) = compact {
            pattern.render(number, sign, symbols, "")
        } else {
            self.core.pattern.render(number, sign, symbols, "")
        };
        match &self.style {
            NativeStyle::Unit(unit) => unit.format(number, category),
            NativeStyle::Percent(Some(unit)) => {
                let mut parts = unit.format(number, category);
                parts.rename("unit", "percentSign");
                parts
            }
            NativeStyle::Decimal | NativeStyle::Percent(None) => number,
            NativeStyle::Currency(_) => unreachable!("currency returned above"),
        }
    }

    fn nonfinite<O: NumberOutput>(
        &self,
        symbol: NumberSymbol,
        kind: &'static str,
        negative: bool,
        zero: bool,
        display: SignDisplay,
    ) -> (O, Sign, PluralCategory) {
        let sign = match display {
            SignDisplay::Never => Sign::None,
            SignDisplay::Always => {
                if negative {
                    Sign::Negative
                } else {
                    Sign::Positive
                }
            }
            SignDisplay::ExceptZero if !zero => {
                if negative {
                    Sign::Negative
                } else {
                    Sign::Positive
                }
            }
            SignDisplay::Auto | SignDisplay::Negative if negative => Sign::Negative,
            _ => Sign::None,
        };
        let mut parts = O::default();
        push_symbol(
            &mut parts,
            kind,
            self.core
                .symbols
                .get()
                .get(symbol)
                .expect("validated nonfinite symbol"),
        );
        (parts, sign, PluralCategory::Other)
    }
}

pub(super) fn decimal_formatter(
    provider: &IntlProvider,
    locale: &Locale,
    numbering_system: &str,
    grouping: GroupingStrategy,
) -> JsResult<DecimalFormatter> {
    let mut preferences: icu_decimal::DecimalFormatterPreferences = locale.into();
    let numbering_system = numbering_system
        .parse::<Value>()
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
    preferences.numbering_system = Some(
        NumberingSystem::try_from(numbering_system)
            .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?,
    );
    let mut options = DecimalFormatterOptions::default();
    options.grouping_strategy = Some(grouping);
    DecimalFormatter::try_new_with_buffer_provider(provider.erased_provider(), preferences, options)
        .map_err(|error| JsNativeError::typ().with_message(error.to_string()).into())
}

fn validate_symbols(symbols: &boa_intl_data::NumberSymbols<'_>, percent: bool) -> JsResult<()> {
    for symbol in [
        NumberSymbol::NumberingSystem,
        NumberSymbol::NaN,
        NumberSymbol::Infinity,
        NumberSymbol::PlusSign,
        NumberSymbol::MinusSign,
        NumberSymbol::Decimal,
        NumberSymbol::Group,
    ] {
        if symbols.get(symbol).is_none() {
            return Err(JsNativeError::typ()
                .with_message("missing CLDR number symbol")
                .into());
        }
    }
    if percent && symbols.get(NumberSymbol::PercentSign).is_none() {
        return Err(JsNativeError::typ()
            .with_message("missing CLDR percent symbol")
            .into());
    }
    Ok(())
}

fn plural_category(category: IcuPluralCategory) -> PluralCategory {
    match category {
        IcuPluralCategory::Zero => PluralCategory::Zero,
        IcuPluralCategory::One => PluralCategory::One,
        IcuPluralCategory::Two => PluralCategory::Two,
        IcuPluralCategory::Few => PluralCategory::Few,
        IcuPluralCategory::Many => PluralCategory::Many,
        IcuPluralCategory::Other => PluralCategory::Other,
    }
}
