//! Exact exponent selection and localized exponent parts.

use boa_intl_data::{NumberSymbol, NumberSymbols};
use fixed_decimal::Decimal;
use icu_decimal::{DecimalFormatter, options::GroupingStrategy};
use icu_locale::Locale;
use icu_plurals::{PluralOperands, RawPluralOperands};
use writeable::Writeable;

use super::{
    backend::decimal_formatter,
    compact::{CompactFormatter, CompactSelection},
    compact_pattern::CompactPattern,
    options::{DigitFormatOptions, Notation, UnitFormatOptions},
    output::{DecimalWriter, NumberOutput, push_symbol},
    pattern::NumberPattern,
};
use crate::{JsNativeError, JsResult, context::icu::IntlProvider};

#[derive(Debug)]
pub(super) enum NativeNotation {
    Standard,
    Scientific(ScientificFormatter),
    Compact(CompactFormatter),
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct PreparedNotation {
    pub(super) exponent: i32,
    pub(super) compact: CompactSelection,
}

impl NativeNotation {
    pub(super) fn new(
        provider: &IntlProvider,
        locale: &Locale,
        numbering_system: &str,
        notation: Notation,
        style: &UnitFormatOptions,
        scientific_pattern: Option<&str>,
        symbols: &NumberSymbols<'_>,
    ) -> JsResult<Self> {
        match notation {
            Notation::Standard => Ok(Self::Standard),
            Notation::Scientific | Notation::Engineering => {
                let raw = scientific_pattern.ok_or_else(|| {
                    JsNativeError::typ().with_message("missing CLDR scientific pattern")
                })?;
                // Legacy LDML patterns can omit E or contain bracket affixes.
                // Intl uses its notation subpattern and localized symbols,
                // rather than those legacy DecimalFormat affixes.
                NumberPattern::parse(raw)?;
                if symbols.get(NumberSymbol::Exponential).is_none() {
                    return Err(JsNativeError::typ()
                        .with_message("missing CLDR exponent separator")
                        .into());
                }
                Ok(Self::Scientific(ScientificFormatter {
                    engineering: notation == Notation::Engineering,
                    exponent: decimal_formatter(
                        provider,
                        locale,
                        numbering_system,
                        GroupingStrategy::Never,
                    )?,
                }))
            }
            Notation::Compact { display } => {
                CompactFormatter::new(provider, locale, numbering_system, display, style)
                    .map(Self::Compact)
            }
        }
    }

    pub(super) fn prepare(
        &self,
        value: &mut Decimal,
        digits: &DigitFormatOptions,
    ) -> PreparedNotation {
        match self {
            Self::Standard => {
                digits.format_fixed_decimal(value);
                PreparedNotation::default()
            }
            Self::Scientific(scientific) => {
                let exponent = select_exponent(value, digits, |magnitude| {
                    if scientific.engineering {
                        magnitude.div_euclid(3) * 3
                    } else {
                        magnitude
                    }
                });
                PreparedNotation {
                    exponent,
                    compact: CompactSelection::default(),
                }
            }
            Self::Compact(compact) => {
                let compact = compact.prepare(value, digits);
                PreparedNotation {
                    exponent: compact.exponent,
                    compact,
                }
            }
        }
    }

    pub(super) fn render<O: NumberOutput>(
        &self,
        number: O,
        exponent: i32,
        symbols: &NumberSymbols<'_>,
    ) -> O {
        match self {
            Self::Standard | Self::Compact(_) => number,
            Self::Scientific(scientific) => scientific.render(number, exponent, symbols),
        }
    }

    pub(super) fn operands(&self, value: &Decimal, exponent: i32) -> PluralOperands {
        match self {
            Self::Standard => value.into(),
            Self::Scientific(_) | Self::Compact(_) => expanded_operands(value, exponent),
        }
    }

    pub(super) fn compact_pattern(
        &self,
        selected: PreparedNotation,
        value: &Decimal,
        category: boa_intl_data::PluralCategory,
        alpha: bool,
    ) -> Option<&CompactPattern> {
        match self {
            Self::Compact(compact) => compact.pattern(selected.compact, value, category, alpha),
            _ => None,
        }
    }
}

/// Computes the rounded mathematical value's ICU operands without materializing
/// exponent-sized padding. As with ICU's Decimal conversion, retain 18 digits
/// on either side of the decimal separator and preserve visible precision.
fn expanded_operands(value: &Decimal, exponent: i32) -> PluralOperands {
    let range = value.magnitude_range();
    let high = (i32::from(*range.end()) + exponent).min(17);
    let low = (i32::from(*range.start()) + exponent).clamp(-18, 0);
    let digit = |magnitude: i32| {
        i16::try_from(magnitude - exponent)
            .map_or(0, |magnitude| u64::from(value.digit_at(magnitude)))
    };
    let mut operands = RawPluralOperands {
        i: 0,
        v: (-low) as usize,
        w: 0,
        f: 0,
        t: 0,
        c: 0,
    };
    for magnitude in (0..=high).rev() {
        operands.i = operands.i * 10 + digit(magnitude);
    }
    for magnitude in (low..=-1).rev() {
        let digit = digit(magnitude);
        operands.f = operands.f * 10 + digit;
        if digit != 0 {
            operands.t = operands.f;
            operands.w = (-magnitude) as usize;
        }
    }
    operands.into()
}

#[derive(Debug)]
pub(super) struct ScientificFormatter {
    engineering: bool,
    exponent: DecimalFormatter,
}

impl ScientificFormatter {
    fn render<O: NumberOutput>(
        &self,
        mut number: O,
        exponent: i32,
        symbols: &NumberSymbols<'_>,
    ) -> O {
        push_symbol(
            &mut number,
            "exponentSeparator",
            symbols
                .get(NumberSymbol::Exponential)
                .expect("validated exponent separator"),
        );
        if exponent < 0 {
            push_symbol(
                &mut number,
                "exponentMinusSign",
                symbols
                    .get(NumberSymbol::MinusSign)
                    .expect("validated minus sign"),
            );
        }
        let mut parts = DecimalWriter::<O>::new(None, None, true);
        let exponent = Decimal::from(exponent.unsigned_abs());
        let formatted = self.exponent.format(&exponent);
        parts
            .output
            .reserve_text(formatted.writeable_length_hint().capacity());
        formatted
            .write_to_parts(&mut parts)
            .expect("writing to String cannot fail");
        number.append(parts.output);
        number
    }
}

/// Selects a carry using the actual sign and retains the rounded trial whenever
/// its exponent wins. Only a carry into another exponent rerounds the original.
pub(super) fn select_exponent(
    value: &mut Decimal,
    digits: &DigitFormatOptions,
    exponent_for_magnitude: impl Fn(i32) -> i32,
) -> i32 {
    if value.is_zero() {
        digits.format_fixed_decimal(value);
        return 0;
    }
    let magnitude = i32::from(value.nonzero_magnitude_start());
    let exponent = exponent_for_magnitude(magnitude);
    let mut trial = value.clone();
    scale(&mut trial, exponent);
    digits.format_fixed_decimal(&mut trial);
    let selected =
        if trial.is_zero() || i32::from(trial.nonzero_magnitude_start()) == magnitude - exponent {
            exponent
        } else {
            exponent_for_magnitude(magnitude + 1)
        };
    if selected == exponent {
        *value = trial;
    } else {
        scale(value, selected);
        digits.format_fixed_decimal(value);
    }
    selected
}

/// Shifts exactly, including exponents outside one i16 delta in engineering notation.
pub(super) fn scale(value: &mut Decimal, exponent: i32) {
    let mut delta = -exponent;
    value.trim_start();
    value.trim_end();
    while delta != 0 {
        let step = delta.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        value.multiply_pow10(step);
        value.trim_start();
        value.trim_end();
        delta -= i32::from(step);
    }
}
