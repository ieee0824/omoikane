//! Combines native calendar-period and time patterns using locale CLDR glue.
use icu_calendar::Iso;
use icu_datetime::{
    DateTimeFormatter, DateTimeFormatterPreferences,
    fieldsets::enums::CompositeDateTimeFieldSet,
    input::DateTime,
    options::Length,
    pattern::DateTimePattern,
    provider::{
        neo::{
            DatetimePatternsGlueV1,
            marker_attrs::{GlueType, PatternLength, pattern_marker_attr_for_glue},
        },
        pattern::{GenericPatternItem, runtime::Pattern},
    },
};
use icu_provider::prelude::*;

/// Owns a separately selected clock and zero-copy locale combination data.
#[derive(Debug, Clone)]
pub(super) struct CalendarPeriodTime {
    formatter: DateTimeFormatter<CompositeDateTimeFieldSet>,
    glue: DataPayload<DatetimePatternsGlueV1>,
}

impl CalendarPeriodTime {
    pub(super) fn new<P: BufferProvider + ?Sized>(
        provider: &P,
        prefs: DateTimeFormatterPreferences,
        fields: CompositeDateTimeFieldSet,
        length: Length,
    ) -> Result<Self, String> {
        let formatter = DateTimeFormatter::try_new_with_buffer_provider(provider, prefs, fields)
            .map_err(|error| format!("calendar-period clock: {error:?}"))?;
        let length = match length {
            Length::Long => PatternLength::Long,
            Length::Medium => PatternLength::Medium,
            Length::Short => PatternLength::Short,
            _ => {
                return Err(format!(
                    "unsupported date/time combination length: {length:?}"
                ));
            }
        };
        Ok(Self {
            formatter,
            glue: load_glue(provider, prefs, length)?,
        })
    }

    pub(super) fn combine(&self, date: DateTimePattern, input: &DateTime<Iso>) -> DateTimePattern {
        let date: Pattern<'static> = date.into();
        let time: Pattern<'static> = self.formatter.format(input).pattern().into();
        self.glue
            .get()
            .pattern
            .clone()
            .combined(date, time)
            .expect("date/time glue placeholders were validated at construction")
            .into()
    }
}

/// Loads only the selected combination length, after metadata matching.
pub(super) fn load_glue<P: BufferProvider + ?Sized>(
    provider: &P,
    prefs: DateTimeFormatterPreferences,
    length: PatternLength,
) -> Result<DataPayload<DatetimePatternsGlueV1>, String> {
    let locale = DatetimePatternsGlueV1::INFO.make_locale(prefs.locale_preferences);
    let attributes = pattern_marker_attr_for_glue(length, GlueType::DateTime);
    let response = DataProvider::<DatetimePatternsGlueV1>::load(
        &provider.as_deserializing(),
        DataRequest {
            id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
            ..Default::default()
        },
    )
    .map_err(|error| format!("calendar-period combination data: {error:?}"))?;
    if response
        .payload
        .get()
        .pattern
        .items
        .iter()
        .any(|item| matches!(item, GenericPatternItem::Placeholder(index) if index > 1))
    {
        return Err("date/time glue has an unsupported placeholder".to_owned());
    }
    for index in [0, 1] {
        if response
            .payload
            .get()
            .pattern
            .items
            .iter()
            .filter(|item| *item == GenericPatternItem::Placeholder(index))
            .count()
            != 1
        {
            return Err("date/time glue must contain each component exactly once".to_owned());
        }
    }
    Ok(response.payload)
}
