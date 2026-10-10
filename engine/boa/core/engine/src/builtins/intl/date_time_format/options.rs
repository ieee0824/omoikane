//! Reads date/time component options in ECMA-402 order.
use super::{matcher::FormatMatcher, pattern::ComponentWidths};
use crate::builtins::intl::options::get_number_option;
use crate::builtins::options::get_option;
use crate::{Context, JsNativeError, JsObject, JsResult, JsString, js_string};
use icu_datetime::{
    fieldsets::{
        builder::{DateFields, FieldSetBuilder},
        enums::CompositeDateTimeFieldSet,
    },
    options::{Length, SubsecondDigits, TimePrecision, YearStyle},
    provider::fields::FieldLength,
};

const TEXT: &[&str] = &["narrow", "short", "long"];
const NUMERIC: &[&str] = &["numeric", "2-digit"];

/// ICU semantic date fields and, when needed, a separate time selection.
/// Calendar periods (Y/M/YM) cannot be directly combined by ICU's builder.
pub(super) struct DateTimeFields {
    pub(super) primary: CompositeDateTimeFieldSet,
    pub(super) time: Option<CompositeDateTimeFieldSet>,
    pub(super) length: Length,
    /// Explicit date-only component skeleton, absent for style requests.
    pub(super) date_skeleton: Option<String>,
    /// A clock selection used when an exact CLDR date pattern is combined.
    pub(super) clock: Option<CompositeDateTimeFieldSet>,
}

#[derive(Debug, Default, Clone)]
pub(super) struct Components {
    pub(super) weekday: Option<JsString>,
    pub(super) era: Option<JsString>,
    pub(super) year: Option<JsString>,
    pub(super) month: Option<JsString>,
    pub(super) day: Option<JsString>,
    pub(super) day_period: Option<JsString>,
    pub(super) hour: Option<JsString>,
    pub(super) minute: Option<JsString>,
    pub(super) second: Option<JsString>,
    pub(super) fractional_digits: Option<u8>,
    pub(super) zone_name: Option<JsString>,
    pub(super) date_style: Option<JsString>,
    pub(super) time_style: Option<JsString>,
    pub(super) format_matcher: FormatMatcher,
}

pub(super) fn string_option(
    options: &JsObject,
    name: &str,
    allowed: &[&str],
    context: &mut Context,
) -> JsResult<Option<JsString>> {
    let value = get_option::<JsString>(options, js_string!(name), context)?;
    if let Some(value) = &value {
        if !allowed.contains(&value.to_std_string_escaped().as_str()) {
            return Err(JsNativeError::range()
                .with_message(format!("invalid {name} option"))
                .into());
        }
    }
    Ok(value)
}

impl Components {
    pub(super) fn read(
        options: &JsObject,
        required: super::DateTimeReqs,
        defaults: super::DateTimeReqs,
        context: &mut Context,
    ) -> JsResult<Self> {
        let mut result = Self {
            weekday: string_option(options, "weekday", TEXT, context)?,
            era: string_option(options, "era", TEXT, context)?,
            year: string_option(options, "year", NUMERIC, context)?,
            month: string_option(
                options,
                "month",
                &["numeric", "2-digit", "narrow", "short", "long"],
                context,
            )?,
            day: string_option(options, "day", NUMERIC, context)?,
            day_period: string_option(options, "dayPeriod", TEXT, context)?,
            hour: string_option(options, "hour", NUMERIC, context)?,
            minute: string_option(options, "minute", NUMERIC, context)?,
            second: string_option(options, "second", NUMERIC, context)?,
            fractional_digits: get_number_option(
                options,
                js_string!("fractionalSecondDigits"),
                1u8,
                3u8,
                context,
            )?,
            zone_name: string_option(
                options,
                "timeZoneName",
                &[
                    "short",
                    "long",
                    "shortOffset",
                    "longOffset",
                    "shortGeneric",
                    "longGeneric",
                ],
                context,
            )?,
            ..Self::default()
        };
        result.format_matcher =
            match string_option(options, "formatMatcher", &["basic", "best fit"], context)? {
                Some(value) if value == js_string!("basic") => FormatMatcher::Basic,
                _ => FormatMatcher::BestFit,
            };
        result.date_style = string_option(
            options,
            "dateStyle",
            &["full", "long", "medium", "short"],
            context,
        )?;
        result.time_style = string_option(
            options,
            "timeStyle",
            &["full", "long", "medium", "short"],
            context,
        )?;
        if (result.date_style.is_some() || result.time_style.is_some()) && result.has_components() {
            return Err(JsNativeError::typ()
                .with_message("styles cannot be combined with date/time components")
                .into());
        }
        result.apply_defaults(required, defaults)?;
        Ok(result)
    }

    fn apply_defaults(
        &mut self,
        required: super::DateTimeReqs,
        defaults: super::DateTimeReqs,
    ) -> JsResult<()> {
        use super::DateTimeReqs::{AnyAll, Date, Time};
        if required == Date && self.time_style.is_some() {
            return Err(JsNativeError::typ()
                .with_message("timeStyle is not allowed for a date-only format")
                .into());
        }
        if required == Time && self.date_style.is_some() {
            return Err(JsNativeError::typ()
                .with_message("dateStyle is not allowed for a time-only format")
                .into());
        }
        let date = self.weekday.is_some()
            || self.year.is_some()
            || self.month.is_some()
            || self.day.is_some();
        let time = self.day_period.is_some()
            || self.hour.is_some()
            || self.minute.is_some()
            || self.second.is_some()
            || self.fractional_digits.is_some();
        let needs_defaults = match required {
            Date => !date,
            Time => !time,
            AnyAll => !date && !time,
        };
        if needs_defaults && self.date_style.is_none() && self.time_style.is_none() {
            if matches!(defaults, Date | AnyAll) {
                self.year = Some(js_string!("numeric"));
                self.month = Some(js_string!("numeric"));
                self.day = Some(js_string!("numeric"));
            }
            if matches!(defaults, Time | AnyAll) {
                self.hour = Some(js_string!("numeric"));
                self.minute = Some(js_string!("numeric"));
                self.second = Some(js_string!("numeric"));
            }
        }
        Ok(())
    }

    fn has_components(&self) -> bool {
        self.weekday.is_some()
            || self.era.is_some()
            || self.year.is_some()
            || self.month.is_some()
            || self.day.is_some()
            || self.day_period.is_some()
            || self.hour.is_some()
            || self.minute.is_some()
            || self.second.is_some()
            || self.fractional_digits.is_some()
            || self.zone_name.is_some()
    }

    pub(super) fn widths(&self) -> ComponentWidths {
        fn width(value: &Option<JsString>) -> Option<FieldLength> {
            let value = value.as_ref()?;
            Some(if value.as_str() == "numeric" {
                FieldLength::One
            } else if value.as_str() == "2-digit" {
                FieldLength::Two
            } else if value.as_str() == "short" {
                FieldLength::Three
            } else if value.as_str() == "long" {
                FieldLength::Four
            } else if value.as_str() == "narrow" {
                FieldLength::Five
            } else {
                unreachable!("validated component option")
            })
        }
        ComponentWidths {
            era: width(&self.era),
            year: width(&self.year),
            month: width(&self.month),
            day: width(&self.day),
            weekday: width(&self.weekday),
            day_period: width(&self.day_period),
            hour: width(&self.hour),
            minute: width(&self.minute),
            second: width(&self.second),
        }
    }

    pub(super) fn fields(&self) -> JsResult<DateTimeFields> {
        let mut builder = FieldSetBuilder::new();
        let date_style = self
            .date_style
            .as_ref()
            .map(JsString::to_std_string_escaped);
        builder.date_fields = if let Some(style) = &date_style {
            Some(if style == "full" {
                DateFields::YMDE
            } else {
                DateFields::YMD
            })
        } else {
            match (
                self.year.is_some(),
                self.month.is_some(),
                self.day.is_some(),
                self.weekday.is_some(),
            ) {
                (false, false, false, false) => None,
                (false, false, false, true) => Some(DateFields::E),
                (true, false, false, false) => Some(DateFields::Y),
                (false, true, false, false) => Some(DateFields::M),
                (true, true, false, false) => Some(DateFields::YM),
                (false, false, true, false) => Some(DateFields::D),
                (false, false, true, true) => Some(DateFields::DE),
                (false, true, _, false) => Some(DateFields::MD),
                (false, true, _, true) => Some(DateFields::MDE),
                (_, _, _, true) => Some(DateFields::YMDE),
                _ => Some(DateFields::YMD),
            }
        };
        let length = date_style
            .as_deref()
            .or_else(|| self.month.as_ref().map(|_| "component"));
        builder.length = Some(match length {
            Some("full" | "long") => Length::Long,
            Some("medium") => Length::Medium,
            Some("component") => match self
                .month
                .as_ref()
                .unwrap()
                .to_std_string_escaped()
                .as_str()
            {
                "long" => Length::Long,
                "short" => Length::Medium,
                _ => Length::Short,
            },
            _ => Length::Short,
        });
        if builder.date_fields.is_some_and(|fields| {
            matches!(
                fields,
                DateFields::Y | DateFields::YM | DateFields::YMD | DateFields::YMDE
            )
        }) {
            builder.year_style = Some(if self.era.is_some() {
                YearStyle::WithEra
            } else if date_style.is_some() {
                YearStyle::Auto
            } else {
                YearStyle::Full
            });
        }
        if self.time_style.is_some()
            || self.hour.is_some()
            || self.minute.is_some()
            || self.second.is_some()
            || self.day_period.is_some()
            || self.fractional_digits.is_some()
        {
            builder.time_precision = Some(if let Some(digits) = self.fractional_digits {
                TimePrecision::Subsecond(match digits {
                    1 => SubsecondDigits::S1,
                    2 => SubsecondDigits::S2,
                    3 => SubsecondDigits::S3,
                    _ => unreachable!("validated fractional second digits"),
                })
            } else if self
                .time_style
                .as_ref()
                .is_some_and(|style| style == &js_string!("short"))
                || (self.time_style.is_none()
                    && self.second.is_none()
                    && self.fractional_digits.is_none())
            {
                TimePrecision::Minute
            } else {
                TimePrecision::Second
            });
        }
        let date_skeleton = (self.date_style.is_none() && self.time_style.is_none())
            .then(|| self.widths().date_skeleton())
            .flatten();
        finish_fields(builder, date_skeleton)
    }
}

fn finish_fields(
    mut builder: FieldSetBuilder,
    date_skeleton: Option<String>,
) -> JsResult<DateTimeFields> {
    let field_error =
        |error| JsNativeError::typ().with_message(format!("date/time field selection: {error:?}"));
    let length = builder.length.unwrap_or(Length::Short);
    let clock = if builder.date_fields.is_some() && builder.time_precision.is_some() {
        let mut clock = builder.clone();
        clock.date_fields = None;
        clock.year_style = None;
        Some(clock.build_composite_datetime().map_err(field_error)?)
    } else {
        None
    };
    let time = if builder.time_precision.is_some()
        && matches!(
            builder.date_fields,
            Some(DateFields::Y | DateFields::M | DateFields::YM)
        ) {
        let mut time = FieldSetBuilder::new();
        time.length = Some(length);
        time.time_precision = builder.time_precision.take();
        Some(time.build_composite_datetime().map_err(field_error)?)
    } else {
        None
    };
    Ok(DateTimeFields {
        primary: builder.build_composite_datetime().map_err(field_error)?,
        time,
        length,
        date_skeleton,
        clock,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_widths_map_js_strings_without_std_string_conversion() {
        let components = Components {
            era: Some(js_string!("narrow")),
            year: Some(js_string!("numeric")),
            month: Some(js_string!("long")),
            day: Some(js_string!("2-digit")),
            weekday: Some(js_string!("short")),
            ..Default::default()
        };

        let widths = components.widths();
        assert_eq!(widths.era, Some(FieldLength::Five));
        assert_eq!(widths.year, Some(FieldLength::One));
        assert_eq!(widths.month, Some(FieldLength::Four));
        assert_eq!(widths.day, Some(FieldLength::Two));
        assert_eq!(widths.weekday, Some(FieldLength::Three));
        assert_eq!(widths.hour, None);
    }
}
