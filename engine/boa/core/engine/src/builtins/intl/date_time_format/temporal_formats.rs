//! Lazily cached native patterns selected from already observed options.
use super::{
    backend::DateTimeBackend, hour_cycle::DateTimeHourCycle, input::Kind, options::Components,
};
use crate::{Context, JsNativeError, JsResult, js_string};
use icu_datetime::DateTimeFormatterPreferences;

#[derive(Debug, Clone)]
pub(super) struct TemporalFormats {
    options: Components,
    preferences: DateTimeFormatterPreferences,
    iso_calendar: bool,
    hour_cycle: DateTimeHourCycle,
    formats: [Option<(Components, DateTimeBackend)>; 6],
}

impl TemporalFormats {
    pub(super) fn new(
        options: Components,
        preferences: DateTimeFormatterPreferences,
        iso_calendar: bool,
        hour_cycle: DateTimeHourCycle,
    ) -> Self {
        Self {
            options,
            preferences,
            iso_calendar,
            hour_cycle,
            formats: std::array::from_fn(|_| None),
        }
    }

    /// Borrows one cache entry only during native formatting. Options are never
    /// reread from JavaScript and provider access cannot call author code.
    pub(super) fn get(
        &mut self,
        kind: Kind,
        base: &DateTimeBackend,
        base_components: &Components,
        context: &Context,
    ) -> JsResult<&mut (Components, DateTimeBackend)> {
        let index = kind as usize;
        if self.formats[index].is_none() {
            let components = select_components(kind, &self.options, base, base_components)?;
            let provider = context.intl_provider().erased_provider();
            let backend = if components.date_style.is_some() || components.time_style.is_some() {
                base.clone()
            } else {
                DateTimeBackend::create(
                    provider,
                    self.preferences,
                    &components,
                    self.iso_calendar,
                    self.hour_cycle,
                )
                .map_err(|error| JsNativeError::range().with_message(error))?
            };
            self.formats[index] = Some((components, backend));
        }
        Ok(self.formats[index]
            .as_mut()
            .expect("format was initialized"))
    }
}

fn select_components(
    kind: Kind,
    options: &Components,
    base: &DateTimeBackend,
    base_components: &Components,
) -> JsResult<Components> {
    let has_style = options.date_style.is_some() || options.time_style.is_some();
    if has_style {
        if matches!(kind, Kind::Date | Kind::YearMonth | Kind::MonthDay)
            && options.date_style.is_none()
            || kind == Kind::Time && options.time_style.is_none()
        {
            return Err(no_overlap());
        }
        if kind == Kind::Instant {
            return Ok(options.clone());
        }
        let resolved = if let Some(fields) = base.selected_fields() {
            super::resolution::ResolvedPattern::from_components(fields, base.hour_cycle())
        } else {
            super::resolution::ResolvedPattern::from_pattern(
                base.resolved_pattern(base_components.widths()),
            )
        };
        let mut components = Components {
            format_matcher: options.format_matcher,
            ..Components::default()
        };
        for (name, value) in resolved.fields {
            match name {
                "weekday" => components.weekday = Some(js_string!(value)),
                "era" => components.era = Some(js_string!(value)),
                "year" => components.year = Some(js_string!(value)),
                "month" => components.month = Some(js_string!(value)),
                "day" => components.day = Some(js_string!(value)),
                "dayPeriod" => components.day_period = Some(js_string!(value)),
                "hour" => components.hour = Some(js_string!(value)),
                "minute" => components.minute = Some(js_string!(value)),
                "second" => components.second = Some(js_string!(value)),
                "timeZoneName" => components.zone_name = Some(js_string!(value)),
                _ => {}
            }
        }
        components.fractional_digits = resolved.fractional_digits;
        let filtered = filter(kind, components.clone());
        if same_fields(&components, &filtered) {
            return Ok(options.clone());
        }
        return Ok(filtered);
    }
    let mut components = filter(kind, options.clone());
    let any_present = relevant_fields(options);
    if !relevant_fields(&components) {
        if any_present && kind != Kind::Instant {
            return Err(no_overlap());
        }
        if matches!(
            kind,
            Kind::Date | Kind::YearMonth | Kind::DateTime | Kind::Instant
        ) {
            components.year = Some(js_string!("numeric"));
        }
        if kind != Kind::Time {
            components.month = Some(js_string!("numeric"));
        }
        if !matches!(kind, Kind::Time | Kind::YearMonth) {
            components.day = Some(js_string!("numeric"));
        }
        if matches!(kind, Kind::Time | Kind::DateTime | Kind::Instant) {
            components.hour = Some(js_string!("numeric"));
            components.minute = Some(js_string!("numeric"));
            components.second = Some(js_string!("numeric"));
        }
    }
    Ok(components)
}

fn filter(kind: Kind, mut value: Components) -> Components {
    if !matches!(kind, Kind::DateTime | Kind::Instant) {
        value.zone_name = None;
        if matches!(kind, Kind::Date | Kind::YearMonth | Kind::MonthDay) {
            value.day_period = None;
            value.hour = None;
            value.minute = None;
            value.second = None;
            value.fractional_digits = None;
        }
        if matches!(kind, Kind::YearMonth | Kind::MonthDay | Kind::Time) {
            value.weekday = None;
        }
        if matches!(kind, Kind::MonthDay | Kind::Time) {
            value.era = None;
            value.year = None;
        }
        if kind == Kind::YearMonth {
            value.day = None;
        }
        if kind == Kind::Time {
            value.month = None;
            value.day = None;
        }
    }
    // PlainDateTime also ignores time-zone name options.
    if kind == Kind::DateTime {
        value.zone_name = None;
    }
    value
}

fn relevant_fields(value: &Components) -> bool {
    value.weekday.is_some()
        || value.year.is_some()
        || value.month.is_some()
        || value.day.is_some()
        || value.day_period.is_some()
        || value.hour.is_some()
        || value.minute.is_some()
        || value.second.is_some()
        || value.fractional_digits.is_some()
}

fn same_fields(left: &Components, right: &Components) -> bool {
    left.zone_name == right.zone_name
        && left.weekday == right.weekday
        && left.era == right.era
        && left.year == right.year
        && left.month == right.month
        && left.day == right.day
        && left.day_period == right.day_period
        && left.hour == right.hour
        && left.minute == right.minute
        && left.second == right.second
        && left.fractional_digits == right.fractional_digits
}

fn no_overlap() -> crate::JsError {
    JsNativeError::typ()
        .with_message("formatter options do not overlap the Temporal type")
        .into()
}
