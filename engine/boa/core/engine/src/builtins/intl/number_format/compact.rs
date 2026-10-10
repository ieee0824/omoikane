//! Immutable compact thresholds and plural patterns from typed CLDR data.

use boa_intl_data::{CompactPatterns, OmoikaneCompactPatternsV1, PluralCategory};
use fixed_decimal::{Decimal, Sign};
use icu_locale::Locale;
use icu_provider::DataErrorKind;

use super::{
    compact_pattern::CompactPattern,
    data,
    notation::scale,
    options::{CompactDisplay, CurrencyDisplay, DigitFormatOptions, UnitFormatOptions},
};
use crate::{JsNativeError, JsResult, context::icu::IntlProvider};

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct CompactSelection {
    pub(super) exponent: i32,
    entry: Option<usize>,
}

#[derive(Debug)]
struct Entry {
    magnitude: i32,
    divisor: i32,
    normal: [Option<CompactPattern>; 8],
    alpha: [Option<CompactPattern>; 8],
}

#[derive(Debug)]
pub(super) struct CompactFormatter {
    entries: Vec<Entry>,
}

impl CompactFormatter {
    pub(super) fn new(
        provider: &IntlProvider,
        locale: &Locale,
        nu: &str,
        display: CompactDisplay,
        style: &UnitFormatOptions,
    ) -> JsResult<Self> {
        let currency = matches!(style,UnitFormatOptions::Currency { display,.. } if *display != CurrencyDisplay::Name);
        let family = if currency {
            "currency-short"
        } else if display == CompactDisplay::Long {
            "decimal-long"
        } else {
            "decimal-short"
        };
        let payload = match data::try_load::<OmoikaneCompactPatternsV1>(
            provider,
            locale,
            &format!("{family}-{nu}"),
        ) {
            Ok(payload) => payload,
            Err(error) if error.kind == DataErrorKind::IdentifierNotFound && nu != "latn" => {
                data::load::<OmoikaneCompactPatternsV1>(
                    provider,
                    locale,
                    &format!("{family}-latn"),
                )?
            }
            Err(error) => return Err(data::data_error(error)),
        };
        Self::from_data(payload.get())
    }

    pub(super) fn from_data(data: &CompactPatterns<'_>) -> JsResult<Self> {
        let entries = data
            .magnitudes()
            .map(|threshold| Entry::new(data, threshold))
            .collect::<JsResult<Vec<_>>>()?;
        if entries.is_empty() {
            return Err(invalid("missing compact thresholds"));
        }
        Ok(Self { entries })
    }

    pub(super) fn prepare(
        &self,
        value: &mut Decimal,
        digits: &DigitFormatOptions,
    ) -> CompactSelection {
        if value.is_zero() {
            digits.format_fixed_decimal(value);
            return CompactSelection::default();
        }
        let magnitude = i32::from(value.nonzero_magnitude_start());
        let initial = self.selection(magnitude);
        let mut trial = value.clone();
        scale(&mut trial, initial.exponent);
        digits.format_fixed_decimal(&mut trial);
        let selected = if trial.is_zero()
            || i32::from(trial.nonzero_magnitude_start()) == magnitude - initial.exponent
        {
            initial
        } else {
            self.selection(magnitude + 1)
        };
        if selected.exponent == initial.exponent {
            *value = trial;
        } else {
            scale(value, selected.exponent);
            digits.format_fixed_decimal(value);
        }
        selected
    }

    fn selection(&self, magnitude: i32) -> CompactSelection {
        let entry = self
            .entries
            .partition_point(|entry| entry.magnitude <= magnitude)
            .checked_sub(1);
        CompactSelection {
            exponent: entry.map_or(0, |index| self.entries[index].divisor),
            entry,
        }
    }

    pub(super) fn pattern(
        &self,
        selection: CompactSelection,
        value: &Decimal,
        category: PluralCategory,
        alpha: bool,
    ) -> Option<&CompactPattern> {
        let entry = &self.entries[selection.entry?];
        let patterns = if alpha { &entry.alpha } else { &entry.normal };
        let explicit = if value.sign() != Sign::Negative {
            if value.is_zero() {
                Some(PluralCategory::ExplicitZero)
            } else if value.nonzero_magnitude_start() == 0
                && value.nonzero_magnitude_end() == 0
                && value.digit_at(0) == 1
            {
                Some(PluralCategory::ExplicitOne)
            } else {
                None
            }
        } else {
            None
        };
        let pattern = explicit
            .and_then(|category| patterns[category as usize].as_ref())
            .or(patterns[category as usize].as_ref())
            .or(patterns[PluralCategory::Other as usize].as_ref())?;
        (!pattern.sentinel).then_some(pattern)
    }
}

impl Entry {
    fn new(data: &CompactPatterns<'_>, threshold: u64) -> JsResult<Self> {
        if threshold == 0 || 10_u64.checked_pow(threshold.ilog10()) != Some(threshold) {
            return Err(invalid("invalid compact threshold"));
        }
        let normal = parse_patterns(|category| data.get(threshold, category))?;
        let alpha = parse_patterns(|category| data.get_alpha_next_to_number(threshold, category))?;
        let other = normal[PluralCategory::Other as usize]
            .as_ref()
            .ok_or_else(|| invalid("missing compact other pattern"))?;
        let magnitude = threshold.ilog10() as i32;
        let divisor = if other.sentinel {
            0
        } else {
            magnitude
                - i32::from(
                    other
                        .zeros
                        .ok_or_else(|| invalid("missing compact divisor"))?,
                )
                + 1
        };
        if divisor < 0 {
            return Err(invalid("invalid compact divisor"));
        }
        if normal
            .iter()
            .chain(&alpha)
            .flatten()
            .any(|pattern| pattern.zeros.is_some() && pattern.zeros != other.zeros)
        {
            return Err(invalid("inconsistent compact divisor"));
        }
        Ok(Self {
            magnitude,
            divisor,
            normal,
            alpha,
        })
    }
}

fn parse_patterns<'a>(
    mut get: impl FnMut(PluralCategory) -> Option<&'a str>,
) -> JsResult<[Option<CompactPattern>; 8]> {
    let mut patterns = std::array::from_fn(|_| None);
    for category in PluralCategory::ALL {
        patterns[category as usize] = get(category).map(CompactPattern::parse).transpose()?;
    }
    Ok(patterns)
}

fn invalid(message: &'static str) -> crate::JsError {
    JsNativeError::typ().with_message(message).into()
}
