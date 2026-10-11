//! Select exact CLDR styles independently of the input year.
use super::{
    candidates::SelectedFormat,
    hour_cycle::DateTimeHourCycle,
    match_fields::{FieldError, parse_profile},
    options::Components,
};
use boa_intl_data::OmoikaneDateTimeStylePatternsV1;
use icu_datetime::{
    DateTimeFormatter, DateTimeFormatterPreferences,
    fieldsets::enums::CompositeDateTimeFieldSet,
    input::{DateTime, Time},
    provider::{
        fields::FieldSymbol,
        pattern::{
            PatternItem,
            runtime::{GenericPattern, Pattern},
        },
    },
};
use icu_provider::prelude::*;

pub(super) fn select<P: BufferProvider + ?Sized>(
    provider: &P,
    preferences: DateTimeFormatterPreferences,
    calendar: &str,
    options: &Components,
    cycle: DateTimeHourCycle,
) -> Result<SelectedFormat, String> {
    let locale = OmoikaneDateTimeStylePatternsV1::INFO.make_locale(preferences.locale_preferences);
    let attributes = DataMarkerAttributes::from_str_or_panic(calendar);
    let data = DataProvider::<OmoikaneDateTimeStylePatternsV1>::load(
        &provider.as_deserializing(),
        DataRequest {
            id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
            ..Default::default()
        },
    )
    .map_err(|error| format!("date/time style patterns: {error:?}"))?
    .payload;
    let date = options
        .date_style
        .as_ref()
        .map(|style| {
            let raw = data
                .get()
                .get("date", &style.to_std_string_escaped())
                .ok_or_else(|| "missing date style".to_owned())?;
            parse(raw)
        })
        .transpose()?;
    let time = options
        .time_style
        .as_ref()
        .map(|style| {
            let raw = data
                .get()
                .get("time", &style.to_std_string_escaped())
                .ok_or_else(|| "missing time style".to_owned())?;
            clock(provider, preferences, raw, style, cycle)
        })
        .transpose()?;
    let pattern = match (date, time) {
        (Some(date), Some(time)) => {
            let style = options
                .date_style
                .as_ref()
                .expect("date style selected")
                .to_std_string_escaped();
            let raw = data
                .get()
                .get("atTime", &style)
                .or_else(|| data.get().get("glue", &style))
                .ok_or_else(|| "missing style combination".to_owned())?;
            let glue: GenericPattern<'static> = raw
                .parse()
                .map_err(|error| format!("style combination: {error:?}"))?;
            glue.combined(date, time)
                .map_err(|error| format!("style combination: {error:?}"))?
        }
        (Some(pattern), None) | (None, Some(pattern)) => pattern,
        _ => return Err("date/time style was not requested".to_owned()),
    };
    SelectedFormat::from_style(pattern, cycle)
}

fn parse(raw: &str) -> Result<Pattern<'static>, String> {
    parse_profile(raw)
        .map_err(|error| match error {
            FieldError::Invalid(error) => error,
            FieldError::NonEcmaRecord => "unsupported style fields".to_owned(),
        })?
        .pattern
        .ok_or_else(|| "unrepresentable style pattern".to_owned())
}

fn clock<P: BufferProvider + ?Sized>(
    provider: &P,
    preferences: DateTimeFormatterPreferences,
    raw: &str,
    style: &crate::JsString,
    cycle: DateTimeHourCycle,
) -> Result<Pattern<'static>, String> {
    let profile = parse_profile(raw).map_err(|error| format!("time style: {error:?}"))?;
    let source = profile.pattern.ok_or("unrepresentable time style")?;
    let twelve_hour = matches!(cycle, DateTimeHourCycle::H11 | DateTimeHourCycle::H12);
    if profile.twelve_hour == Some(twelve_hour) {
        return Ok(source);
    }
    // The native provider owns the locale's alternate hour-family ordering.
    // Replace only the clock sequence, preserving source zone position/literals.
    let options = Components {
        time_style: Some(style.clone()),
        ..Components::default()
    };
    let fields = options
        .fields()
        .map_err(|error| format!("alternate style clock: {error:?}"))?;
    let formatter: DateTimeFormatter<CompositeDateTimeFieldSet> =
        DateTimeFormatter::try_new_with_buffer_provider(provider, preferences, fields.primary)
            .map_err(|error| format!("alternate style clock data: {error:?}"))?;
    let input = DateTime {
        date: icu_calendar::Date::try_new_iso(1970, 1, 1).expect("valid reference date"),
        time: Time::start_of_day(),
    };
    let native: Pattern<'static> = formatter.format(&input).pattern().into();
    let items: Vec<_> = source.items.iter().collect();
    let is_clock = |item: &PatternItem| matches!(item, PatternItem::Field(field) if matches!(field.symbol, FieldSymbol::Hour(_) | FieldSymbol::Minute | FieldSymbol::Second(_) | FieldSymbol::DecimalSecond(_) | FieldSymbol::DayPeriod(_)));
    let first = items
        .iter()
        .position(is_clock)
        .ok_or("time style has no clock fields")?;
    let last = items.iter().rposition(is_clock).expect("clock field found");
    Ok(items[..first]
        .iter()
        .copied()
        .chain(native.items.iter())
        .chain(items[last + 1..].iter().copied())
        .collect())
}
