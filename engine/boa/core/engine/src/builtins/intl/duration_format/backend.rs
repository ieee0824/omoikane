//! Pure duration partitioning over owned native number/list formatter data.

use super::{
    options::{DurationOptions, DurationUnit, UnitDisplay, UnitStyle},
    parts::{DurationOutput, DurationPart, format_text_list, list_parts},
    record::DurationRecord,
};
use crate::{
    JsNativeError, JsResult,
    builtins::intl::number_format::{
        MathematicalValue, NativeNumberFormatter, NumericFormatOptions, SharedNumberFormatCore,
        UnitDisplay as NumberUnitDisplay, UnitFormatOptions,
    },
    context::icu::IntlProvider,
};
use boa_intl_data::{DurationDigitalField, OmoikaneDurationDigitalV1};
use fixed_decimal::{Decimal, Sign, SignDisplay, SignedRoundingMode, UnsignedRoundingMode};
use icu_decimal::options::GroupingStrategy;
use icu_list::{
    ListFormatter, ListFormatterPreferences,
    options::{ListFormatterOptions, ListLength},
};
use icu_locale::Locale;
use icu_provider::DataPayload;

/// All formatter state is owned; provider/context borrows are constructor-local.
#[derive(Debug)]
pub(super) struct DurationBackend {
    numbers: [Option<NativeNumberFormatter>; 10],
    list: ListFormatter,
    digital: DataPayload<OmoikaneDurationDigitalV1>,
}

impl DurationBackend {
    /// Loads only the numeric/unit widths used by this formatter, plus its list.
    pub(super) fn new(
        provider: &IntlProvider,
        locale: &Locale,
        nu: &str,
        options: &DurationOptions,
        digital: DataPayload<OmoikaneDurationDigitalV1>,
    ) -> JsResult<Self> {
        let mut numbers = std::array::from_fn(|_| None);
        let mut text_core: Option<SharedNumberFormatCore> = None;
        let mut numeric_core: Option<SharedNumberFormatCore> = None;
        for unit in DurationUnit::ALL {
            let style = options.unit(unit).style;
            if style == UnitStyle::Fractional {
                continue;
            }
            let native_style = match style {
                UnitStyle::Long | UnitStyle::Short | UnitStyle::Narrow => UnitFormatOptions::Unit {
                    unit: unit.singular().parse().map_err(|_| {
                        JsNativeError::typ().with_message("native duration unit is not supported")
                    })?,
                    display: match style {
                        UnitStyle::Long => NumberUnitDisplay::Long,
                        UnitStyle::Narrow => NumberUnitDisplay::Narrow,
                        _ => NumberUnitDisplay::Short,
                    },
                },
                _ => UnitFormatOptions::Decimal,
            };
            let shared_core = if style.is_text() {
                if text_core.is_none() {
                    text_core = Some(NativeNumberFormatter::new_shared_decimal_core(
                        provider,
                        locale,
                        Some(nu),
                        GroupingStrategy::Auto,
                        true,
                    )?);
                }
                text_core.as_ref().expect("initialized text number core")
            } else {
                if numeric_core.is_none() {
                    numeric_core = Some(NativeNumberFormatter::new_shared_decimal_core(
                        provider,
                        locale,
                        Some(nu),
                        GroupingStrategy::Never,
                        false,
                    )?);
                }
                numeric_core
                    .as_ref()
                    .expect("initialized numeric number core")
            };
            numbers[unit.index()] = Some(NativeNumberFormatter::from_shared_decimal_core(
                provider,
                locale,
                Some(nu),
                &native_style,
                shared_core,
            )?);
        }
        let length = match options.style.list_style() {
            "long" => ListLength::Wide,
            "narrow" => ListLength::Narrow,
            _ => ListLength::Short,
        };
        let list = ListFormatter::try_new_unit_with_buffer_provider(
            provider.erased_provider(),
            ListFormatterPreferences::from(locale),
            ListFormatterOptions::default().with_length(length),
        )
        .map_err(|error| {
            JsNativeError::typ().with_message(format!("duration list data: {error}"))
        })?;
        // Missing custom data is a provider error; do not substitute a locale.
        if digital
            .get()
            .get(DurationDigitalField::TimeSeparator)
            .is_none()
        {
            return Err(JsNativeError::typ()
                .with_message("duration separator data is missing")
                .into());
        }
        Ok(Self {
            numbers,
            list,
            digital,
        })
    }

    #[cfg(test)]
    pub(super) fn number_core_ids(&self) -> [Option<usize>; 10] {
        std::array::from_fn(|index| {
            self.numbers[index]
                .as_ref()
                .map(NativeNumberFormatter::shared_core_id)
        })
    }

    #[cfg(test)]
    pub(super) fn number_has_plural_rules(&self, unit: DurationUnit) -> Option<bool> {
        self.numbers[unit.index()]
            .as_ref()
            .map(NativeNumberFormatter::has_plural_rules)
    }

    /// Partitions validated fields without author code or shared-state mutation.
    pub(super) fn partition(
        &self,
        record: &DurationRecord,
        options: &DurationOptions,
    ) -> JsResult<Vec<DurationPart>> {
        list_parts(&self.list, self.groups(record, options)?)
    }

    /// Formats validated fields directly into their final list string.
    pub(super) fn format(
        &self,
        record: &DurationRecord,
        options: &DurationOptions,
    ) -> JsResult<String> {
        let groups = self.groups::<String>(record, options)?;
        Ok(format_text_list(&self.list, groups))
    }

    fn groups<O: DurationOutput>(
        &self,
        record: &DurationRecord,
        options: &DurationOptions,
    ) -> JsResult<Vec<O>> {
        let mut groups = Vec::new();
        let mut first_sign = true;
        for unit in DurationUnit::ALL {
            let settings = options.unit(unit);
            if settings.style.is_numeric() {
                let numeric: O = self.numeric_units(record, options, unit, first_sign)?;
                if !numeric.is_empty() {
                    groups.push(numeric);
                }
                break;
            }
            let fractional = next_fractional(options, unit);
            let value = if fractional {
                record.fractional_value(unit)?
            } else {
                record.decimal_value(unit)
            };
            if settings.display == UnitDisplay::Always || !value.is_zero() {
                groups
                    .push(self.number_parts(record, options, unit, value, fractional, first_sign));
                first_sign = false;
            }
            if fractional {
                break;
            }
        }
        Ok(groups)
    }

    fn numeric_units<O: DurationOutput>(
        &self,
        record: &DurationRecord,
        options: &DurationOptions,
        first: DurationUnit,
        mut first_sign: bool,
    ) -> JsResult<O> {
        use DurationUnit::{Hours, Minutes, Seconds};
        let mut seconds = Some(record.fractional_value(Seconds)?);
        let hours_shown = first == Hours
            && (record.value(Hours) != 0 || options.unit(Hours).display == UnitDisplay::Always);
        let seconds_shown = !seconds.as_ref().expect("seconds are available").is_zero()
            || options.unit(Seconds).display == UnitDisplay::Always;
        let minutes_shown = matches!(first, Hours | Minutes)
            && ((hours_shown && seconds_shown)
                || record.value(Minutes) != 0
                || options.unit(Minutes).display == UnitDisplay::Always);
        let mut result = O::default();
        for (unit, shown, separator) in [
            (Hours, hours_shown, false),
            (Minutes, minutes_shown, hours_shown),
            (Seconds, seconds_shown, minutes_shown),
        ] {
            if !shown {
                continue;
            }
            if separator {
                result.literal(
                    self.digital
                        .get()
                        .get(DurationDigitalField::TimeSeparator)
                        .expect("validated duration separator"),
                );
            }
            let value = if unit == Seconds {
                seconds.take().expect("seconds are consumed once")
            } else {
                record.decimal_value(unit)
            };
            result.append(self.number_parts(
                record,
                options,
                unit,
                value,
                unit == Seconds,
                first_sign,
            ));
            first_sign = false;
        }
        Ok(result)
    }

    fn number_parts<O: DurationOutput>(
        &self,
        record: &DurationRecord,
        options: &DurationOptions,
        unit: DurationUnit,
        mut value: Decimal,
        fractional: bool,
        first_sign: bool,
    ) -> O {
        if first_sign && value.is_zero() && record.sign() < 0 {
            value.set_sign(Sign::Negative);
        }
        let (min_fraction, max_fraction) = if fractional {
            options
                .fractional_digits
                .map_or((0, 9), |digits| (digits, digits))
        } else {
            (0, 0)
        };
        let digits = NumericFormatOptions::fraction(
            if options.unit(unit).style == UnitStyle::TwoDigit {
                2
            } else {
                1
            },
            min_fraction,
            max_fraction,
            SignedRoundingMode::Unsigned(UnsignedRoundingMode::Trunc),
            if first_sign {
                SignDisplay::Auto
            } else {
                SignDisplay::Never
            },
        );
        O::number(
            self.numbers[unit.index()]
                .as_ref()
                .expect("nonfractional unit formatter"),
            MathematicalValue::Finite(value),
            &digits,
            unit,
        )
    }
}

fn next_fractional(options: &DurationOptions, unit: DurationUnit) -> bool {
    let next = match unit {
        DurationUnit::Seconds => DurationUnit::Milliseconds,
        DurationUnit::Milliseconds => DurationUnit::Microseconds,
        DurationUnit::Microseconds => DurationUnit::Nanoseconds,
        _ => return false,
    };
    options.unit(next).style == UnitStyle::Fractional
}
