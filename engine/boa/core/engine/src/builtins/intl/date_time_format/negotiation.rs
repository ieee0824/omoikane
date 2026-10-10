//! Provider-backed ECMA-402 locale keywords, separate from public locale text.
use super::hour_cycle::DateTimeHourCycle;
use crate::context::icu::IntlProvider;
use icu_calendar::{AsCalendar, Calendar};
use icu_datetime::{DateTimeFormatter, DateTimeFormatterPreferences, fieldsets::YMD};
use icu_locale::{
    Locale,
    extensions::unicode::{Key, Value, key},
};
use icu_provider::prelude::*;

pub(super) struct NegotiatedLocale {
    pub(super) locale: Locale,
    pub(super) preferences: DateTimeFormatterPreferences,
    pub(super) iso_calendar: bool,
    pub(super) hour_cycle: DateTimeHourCycle,
}

pub(super) fn resolve(
    mut locale: Locale,
    calendar: Option<Value>,
    numbering: Option<Value>,
    hour12: Option<bool>,
    hour_cycle: Option<DateTimeHourCycle>,
    provider: &IntlProvider,
) -> Result<NegotiatedLocale, String> {
    let ca = locale.extensions.unicode.keywords.get(&key!("ca")).cloned();
    let nu = locale.extensions.unicode.keywords.get(&key!("nu")).cloned();
    let hc = locale.extensions.unicode.keywords.get(&key!("hc")).cloned();
    locale.extensions.unicode.clear();
    let mut preferences: DateTimeFormatterPreferences = locale.clone().into();
    // ICU calendar defaults inspect the region; infer it with the provider's
    // likely-subtags data without maximizing the public resolved locale.
    let mut expanded = locale.id.clone();
    provider
        .locale_expander()
        .map_err(|error| error.to_string())?
        .maximize(&mut expanded);
    preferences.locale_preferences.extend((&expanded).into());
    let selected = keyword(&mut locale, key!("ca"), ca, calendar, |value| {
        supports_calendar(value, preferences, provider)
    });
    let iso_calendar = selected
        .as_ref()
        .is_some_and(|value| value.to_string() == "iso8601");
    preferences.calendar_algorithm = selected.as_ref().and_then(calendar_algorithm);
    let selected = keyword(&mut locale, key!("nu"), nu, numbering, |value| {
        supports_numbering(value, provider)
    });
    preferences.numbering_system = selected.and_then(|value| value.try_into().ok());
    let hour_cycle = if let Some(hour12) = hour12 {
        super::hour_cycle::locale_cycle(provider, &locale.id, if hour12 { "h" } else { "h0" })?
    } else {
        let selected = keyword(
            &mut locale,
            key!("hc"),
            hc,
            hour_cycle.map(|cycle| Value::try_from_str(cycle.as_str()).expect("valid hour cycle")),
            |value| DateTimeHourCycle::from_str(&value.to_string()).is_some(),
        );
        selected
            .and_then(|value| DateTimeHourCycle::from_str(&value.to_string()))
            .map(Ok)
            .unwrap_or_else(|| super::hour_cycle::locale_cycle(provider, &locale.id, "j"))?
    };
    preferences.hour_cycle = Some(hour_cycle.icu());
    Ok(NegotiatedLocale {
        locale,
        preferences,
        iso_calendar,
        hour_cycle,
    })
}

fn keyword(
    locale: &mut Locale,
    key: Key,
    requested: Option<Value>,
    option: Option<Value>,
    supported: impl Fn(&Value) -> bool,
) -> Option<Value> {
    let requested = requested.filter(&supported);
    let selected = option.filter(supported).or_else(|| requested.clone());
    // Only a supported requested extension remains in [[Locale]]. Options
    // override preferences without being inserted into the public locale.
    if requested.is_some() && requested == selected {
        locale
            .extensions
            .unicode
            .keywords
            .set(key, requested.clone().unwrap());
    }
    selected
}

fn calendar_algorithm(value: &Value) -> Option<icu_calendar::preferences::CalendarAlgorithm> {
    use icu_calendar::preferences::CalendarAlgorithm;
    match CalendarAlgorithm::try_from(value).ok()? {
        // CLDR ISO8601 date names alias Gregorian. ECMA-402 exposes civil
        // year/month/day/era fields; use their shared Gregorian pattern families
        // with the ISO civil calculation instead of ICU's locale-default fallback.
        CalendarAlgorithm::Iso8601 => Some(CalendarAlgorithm::Gregory),
        calendar => Some(calendar),
    }
}

fn supports_calendar(
    value: &Value,
    mut prefs: DateTimeFormatterPreferences,
    provider: &IntlProvider,
) -> bool {
    let Some(algorithm) = calendar_algorithm(value) else {
        return false;
    };
    prefs.calendar_algorithm = Some(algorithm);
    DateTimeFormatter::try_new_with_buffer_provider(provider.erased_provider(), prefs, YMD::short())
        .is_ok_and(|formatter| {
            formatter.calendar().as_calendar().calendar_algorithm() == Some(algorithm)
        })
}

fn supports_numbering(value: &Value, provider: &IntlProvider) -> bool {
    let Ok(numbering): Result<icu_decimal::preferences::NumberingSystem, _> =
        value.clone().try_into()
    else {
        return false;
    };
    let locale = DataLocale::default();
    let attributes = DataMarkerAttributes::from_str_or_panic(numbering.as_str());
    DryDataProvider::<icu_decimal::provider::DecimalDigitsV1>::dry_load(
        provider,
        DataRequest {
            id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
            ..Default::default()
        },
    )
    .is_ok()
}
