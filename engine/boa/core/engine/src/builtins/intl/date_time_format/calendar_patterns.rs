//! Selects owned exact calendar patterns from typed, zero-copy CLDR data.
use boa_intl_data::{CalendarDatePatterns, OmoikaneCalendarDatePatternsV1};
use icu_datetime::{
    DateTimeFormatterPreferences, pattern::DateTimePattern, provider::pattern::runtime::Pattern,
};
use icu_provider::prelude::*;

/// Retains one locale/calendar payload and an owned parsed selected pattern.
/// No references to another runtime object's state escape a lookup.
#[derive(Debug, Clone)]
pub(super) struct CalendarComponentPattern {
    _data: DataPayload<OmoikaneCalendarDatePatternsV1>,
    pattern: Pattern<'static>,
}

impl CalendarComponentPattern {
    pub(super) fn load<P: BufferProvider + ?Sized>(
        provider: &P,
        prefs: DateTimeFormatterPreferences,
        calendar: &str,
        skeleton: &str,
    ) -> Result<Option<Self>, String> {
        let data = load_calendar_patterns(provider, prefs, calendar)?;
        let Some(raw) = exact_pattern(data.get(), skeleton) else {
            // ICU semantic pattern matching remains responsible for requests
            // for which CLDR does not provide an exact component skeleton.
            return Ok(None);
        };
        let pattern = DateTimePattern::try_from_pattern_str(raw)
            .map_err(|error| format!("calendar available-format pattern: {error:?}"))?
            .into();
        Ok(Some(Self {
            _data: data,
            pattern,
        }))
    }

    pub(super) fn pattern(&self) -> DateTimePattern {
        self.pattern.clone().into()
    }
}

/// Loads one locale/calendar's available formats; borrowing its zero-copy map
/// is limited to constructor-local parsing and selection.
pub(super) fn load_calendar_patterns<P: BufferProvider + ?Sized>(
    provider: &P,
    prefs: DateTimeFormatterPreferences,
    calendar: &str,
) -> Result<DataPayload<OmoikaneCalendarDatePatternsV1>, String> {
    let locale = OmoikaneCalendarDatePatternsV1::INFO.make_locale(prefs.locale_preferences);
    let attributes = DataMarkerAttributes::try_from_str(calendar)
        .map_err(|error| format!("calendar pattern attributes: {error:?}"))?;
    DataProvider::<OmoikaneCalendarDatePatternsV1>::load(
        &provider.as_deserializing(),
        DataRequest {
            id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
            ..Default::default()
        },
    )
    .map(|response| response.payload)
    .map_err(|error| format!("calendar available-format data: {error:?}"))
}

/// CLDR lunar-calendar skeletons commonly spell the numeric year as `yyyy`.
/// This is the same requested numeric year as `y`; `yy` remains a distinct
/// two-digit request. All other component widths and fields match exactly.
fn exact_pattern<'a>(data: &'a CalendarDatePatterns<'_>, skeleton: &str) -> Option<&'a str> {
    if let Some(pattern) = data.get(skeleton) {
        return Some(pattern);
    }
    let position = skeleton.find('y')?;
    if skeleton.as_bytes().get(position + 1) == Some(&b'y') {
        return None;
    }
    let mut cldr_skeleton = skeleton.to_owned();
    cldr_skeleton.insert_str(position, "yyy");
    data.get(&cldr_skeleton)
}

#[cfg(test)]
mod tests {
    use super::exact_pattern;
    use boa_intl_data::CalendarDatePatterns;

    #[test]
    fn numeric_year_alias_keeps_exact_month_width_and_calendar_fields() {
        let data: CalendarDatePatterns<'_> = serde_json::from_str(
            r#"{"available_formats":{"yyyyMMMMd":"rU年MMMMd","yyyyMMMd":"r年MMMd"}}"#,
        )
        .unwrap();
        assert_eq!(exact_pattern(&data, "yMMMd"), Some("r年MMMd"));
        assert_eq!(exact_pattern(&data, "yMMMMd"), Some("rU年MMMMd"));
        assert_eq!(exact_pattern(&data, "yyMMMMd"), None);
        assert_eq!(exact_pattern(&data, "yMMMMdd"), None);
        assert_eq!(exact_pattern(&data, "GyMMMMd"), None);
    }
}
