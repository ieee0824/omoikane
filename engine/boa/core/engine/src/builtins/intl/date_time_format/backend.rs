//! Owned calendar and locale data for native date/time pattern formatting.
use super::{
    calendar_patterns::CalendarComponentPattern,
    calendar_period::CalendarPeriodTime,
    candidates::SelectedFormat,
    hour_cycle::DateTimeHourCycle,
    matcher::{MatchComponents, ZoneName},
    options::DateTimeFields,
    parts::DateTimePart,
    pattern::ComponentWidths,
    range_data::RangeData,
    range_fields::RangeFields,
    range_parts::{RangePart, RangeSource},
    range_selection::RangePlan,
};
use icu_calendar::{AnyCalendar, AnyCalendarKind, AsCalendar, Calendar, Iso, Ref, cal};
use icu_datetime::{
    DateTimeFormatter, DateTimeFormatterPreferences, fieldsets::enums::CompositeDateTimeFieldSet,
    input::DateTime, pattern::FixedCalendarDateTimeNames,
};
use icu_provider::prelude::*;
use std::sync::Arc;
use writeable::TryWriteable;

#[derive(Debug, Clone)]
enum ZoneWriter {
    Short(icu_datetime::NoCalendarFormatter<icu_datetime::fieldsets::zone::LocalizedOffsetShort>),
    Long(icu_datetime::NoCalendarFormatter<icu_datetime::fieldsets::zone::LocalizedOffsetLong>),
}

impl ZoneWriter {
    fn new<P: BufferProvider + ?Sized>(
        provider: &P,
        prefs: DateTimeFormatterPreferences,
        name: ZoneName,
    ) -> Result<Self, String> {
        use icu_datetime::fieldsets::zone::{LocalizedOffsetLong, LocalizedOffsetShort};
        match name {
            ZoneName::Long | ZoneName::LongOffset | ZoneName::LongGeneric => {
                icu_datetime::NoCalendarFormatter::try_new_with_buffer_provider(
                    provider,
                    prefs,
                    LocalizedOffsetLong,
                )
                .map(Self::Long)
                .map_err(|error| format!("long time-zone data: {error:?}"))
            }
            ZoneName::Short | ZoneName::ShortOffset | ZoneName::ShortGeneric => {
                icu_datetime::NoCalendarFormatter::try_new_with_buffer_provider(
                    provider,
                    prefs,
                    LocalizedOffsetShort,
                )
                .map(Self::Short)
                .map_err(|error| format!("short time-zone data: {error:?}"))
            }
        }
    }

    fn offset(offset_seconds: i32) -> Result<icu_datetime::input::UtcOffset, String> {
        icu_datetime::input::UtcOffset::try_from_seconds(offset_seconds)
            .map_err(|error| format!("time-zone offset: {error:?}"))
    }

    fn write_to(
        &self,
        offset_seconds: i32,
        output: &mut impl std::fmt::Write,
    ) -> Result<(), String> {
        let offset = Self::offset(offset_seconds)?;
        match self {
            Self::Short(formatter) => {
                writeable::Writeable::write_to(&formatter.format(&offset), output)
            }
            Self::Long(formatter) => {
                writeable::Writeable::write_to(&formatter.format(&offset), output)
            }
        }
        .map_err(|_| "writing time-zone output failed".to_owned())
    }

    fn format_parts(&self, offset_seconds: i32) -> Result<Vec<DateTimePart>, String> {
        let offset = Self::offset(offset_seconds)?;
        match self {
            Self::Short(formatter) => super::parts::collect(
                &writeable::adapters::WriteableAsTryWriteableInfallible(formatter.format(&offset)),
            ),
            Self::Long(formatter) => super::parts::collect(
                &writeable::adapters::WriteableAsTryWriteableInfallible(formatter.format(&offset)),
            ),
        }
    }
}

// ICU 2.0 exposes custom-pattern formatting for fixed calendars. Keep the
// runtime choice in owned enum variants; borrows are local to each format call.
macro_rules! calendar_names {
    ($($calendar:ident),+ $(,)?) => {
        #[derive(Debug, Clone)]
        enum CalendarNames {
            $( $calendar {
                calendar: cal::$calendar,
                names: FixedCalendarDateTimeNames<cal::$calendar, CompositeDateTimeFieldSet>,
            }, )+
        }

        impl CalendarNames {
            fn range_fields(&self, datetime: &DateTime<Iso>) -> RangeFields {
                match self {
                    $( Self::$calendar { calendar, .. } => {
                        let date = datetime.date.to_calendar(Ref(calendar));
                        RangeFields::from_date(&date, &datetime.time)
                    }, )+
                }
            }

            fn from_calendar<P: BufferProvider + ?Sized>(
                provider: &P,
                prefs: DateTimeFormatterPreferences,
                selected: &AnyCalendar,
            ) -> Result<Self, String> {
                match selected {
                    $( AnyCalendar::$calendar(calendar) => {
                        let calendar = calendar.clone();
                        Ok(Self::$calendar {
                            calendar,
                            // Load textual names only after applying requested widths.
                            // Preloading semantic-skeleton widths would prevent loading
                            // another width into ICU's single-width name containers.
                            names: FixedCalendarDateTimeNames::try_new_with_buffer_provider(provider, prefs)
                                .map_err(|error| format!("numeric data: {error:?}"))?,
                        })
                    }, )+
                    _ => Err("calendar is not supported by ICU date patterns".into()),
                }
            }

            fn format<P: BufferProvider + ?Sized>(
                &mut self,
                provider: &P,
                pattern: &icu_datetime::pattern::DateTimePattern,
                datetime: &DateTime<Iso>,
            ) -> Result<String, String> {
                match self {
                    $( Self::$calendar { calendar, names } => {
                        let converted = DateTime {
                            date: datetime.date.to_calendar(Ref(&*calendar)),
                            time: datetime.time,
                        };
                        let formatter = names.load_for_pattern(&provider.as_deserializing(), pattern)
                            .map_err(|error| format!("pattern data: {error:?}"))?;
                        formatter.format(&converted).try_write_to_string()
                            .map(|value| value.into_owned())
                            .map_err(|(error, _)| format!("pattern formatting: {error:?}"))
                    }, )+
                }
            }

            fn format_parts<P: BufferProvider + ?Sized>(
                &mut self,
                provider: &P,
                pattern: &icu_datetime::pattern::DateTimePattern,
                datetime: &DateTime<Iso>,
                related_year_formatter: Option<&icu_decimal::DecimalFormatter>,
            ) -> Result<Vec<DateTimePart>, String> {
                match self {
                    $( Self::$calendar { calendar, names } => {
                        let converted = DateTime {
                            date: datetime.date.to_calendar(Ref(&*calendar)),
                            time: datetime.time,
                        };
                        let formatter = names.load_for_pattern(&provider.as_deserializing(), pattern)
                            .map_err(|error| format!("pattern data: {error:?}"))?;
                        let mut parts = super::parts::collect(&formatter.format(&converted))?;
                        if let (Some(year), Some(numeric)) =
                            (converted.date.year().cyclic(), related_year_formatter)
                        {
                            super::parts::localize_related_year(&mut parts, year.related_iso, numeric);
                        }
                        Ok(parts)
                    }, )+
                }
            }
        }
    };
}

calendar_names!(
    Buddhist,
    Chinese,
    Coptic,
    Dangi,
    Ethiopian,
    Gregorian,
    Hebrew,
    Indian,
    HijriTabular,
    HijriSimulated,
    HijriUmmAlQura,
    Japanese,
    JapaneseExtended,
    Persian,
    Roc,
);

/// Owns semantic pattern selection and all names required by the chosen calendar.
#[derive(Debug, Clone)]
pub(super) struct DateTimeBackend {
    selection: PatternSelection,
    names: CalendarNames,
    numbering_system: String,
    calendar_name: String,
    iso_calendar: bool,
    hour_cycle: DateTimeHourCycle,
    midnight_hour: Option<String>,
    related_year_formatter: Option<icu_decimal::DecimalFormatter>,
    range_preferences: DateTimeFormatterPreferences,
    range_length: icu_datetime::provider::neo::marker_attrs::PatternLength,
    range: Option<RangeData>,
    // A numeric-only template is cloned once per newly selected pattern, not
    // per endpoint or call. Cached textual names own provider payload handles.
    range_names_template: CalendarNames,
    range_names: Vec<RangeNames>,
    zone_writer: Option<ZoneWriter>,
}

#[derive(Debug, Clone)]
struct RangeNames {
    pattern: super::range_selection::SharedPattern,
    writer_pattern: Arc<icu_datetime::pattern::DateTimePattern>,
    names: CalendarNames,
}

#[derive(Debug, Clone)]
enum PatternSelection {
    Semantic {
        formatter: DateTimeFormatter<CompositeDateTimeFieldSet>,
        calendar_period_time: Option<CalendarPeriodTime>,
        calendar_component_pattern: Option<CalendarComponentPattern>,
        interval_skeleton: Option<String>,
    },
    Basic(SelectedFormat),
}

/// A constructor-local view that observes the numbering system actually loaded.
/// It is never retained in a formatter or propagated through another struct.
struct NumberingInspection<'a, P: ?Sized> {
    inner: &'a P,
    numbering_system: std::cell::RefCell<Option<String>>,
}

impl<P: BufferProvider + ?Sized> DynamicDataProvider<BufferMarker> for NumberingInspection<'_, P> {
    fn load_data(
        &self,
        marker: DataMarkerInfo,
        request: DataRequest<'_>,
    ) -> Result<DataResponse<BufferMarker>, DataError> {
        let response = self.inner.load_data(marker, request);
        if marker == icu_decimal::provider::DecimalDigitsV1::INFO && response.is_ok() {
            *self.numbering_system.borrow_mut() =
                Some(request.id.marker_attributes.as_str().to_owned());
        }
        response
    }
}

impl DateTimeBackend {
    pub(super) fn new<P: BufferProvider + ?Sized>(
        provider: &P,
        prefs: DateTimeFormatterPreferences,
        fields: DateTimeFields,
        zone_name: Option<ZoneName>,
        iso_calendar: bool,
        hour_cycle: DateTimeHourCycle,
    ) -> Result<Self, String> {
        let inspection = NumberingInspection {
            inner: provider,
            numbering_system: Default::default(),
        };
        let formatter =
            DateTimeFormatter::try_new_with_buffer_provider(&inspection, prefs, fields.primary)
                .map_err(|error| format!("date/time data: {error:?}"))?;
        let calendar = calendar_identifier(formatter.calendar().as_calendar());
        let calendar_component_pattern = fields
            .date_skeleton
            .as_deref()
            .map(|skeleton| CalendarComponentPattern::load(&inspection, prefs, &calendar, skeleton))
            .transpose()?
            .flatten();
        let time = if calendar_component_pattern.is_some() {
            fields.clock
        } else {
            fields.time
        };
        let calendar_period_time = time
            .map(|time| CalendarPeriodTime::new(&inspection, prefs, time, fields.length))
            .transpose()?;
        let names =
            CalendarNames::from_calendar(&inspection, prefs, formatter.calendar().as_calendar())?;
        let selection = PatternSelection::Semantic {
            formatter,
            calendar_period_time,
            calendar_component_pattern,
            interval_skeleton: fields.date_skeleton,
        };
        Self::finish(
            inspection,
            prefs,
            selection,
            names,
            calendar,
            iso_calendar,
            hour_cycle,
            fields.length,
            zone_name,
        )
    }

    pub(super) fn new_basic<P: BufferProvider + ?Sized>(
        provider: &P,
        prefs: DateTimeFormatterPreferences,
        request: MatchComponents,
        iso_calendar: bool,
        hour_cycle: DateTimeHourCycle,
    ) -> Result<Self, String> {
        let inspection = NumberingInspection {
            inner: provider,
            numbering_system: Default::default(),
        };
        let kind = AnyCalendarKind::new((&prefs).into());
        let calendar = AnyCalendar::try_new_with_buffer_provider(&inspection, kind)
            .map_err(|error| format!("calendar data: {error:?}"))?;
        let calendar_name = calendar_identifier(&calendar);
        let selected =
            SelectedFormat::load(&inspection, prefs, &calendar_name, request, hour_cycle)?;
        let range_length = match selected.fields.month {
            Some(super::matcher::Width::Long) => icu_datetime::options::Length::Long,
            Some(super::matcher::Width::Short) => icu_datetime::options::Length::Medium,
            _ => icu_datetime::options::Length::Short,
        };
        let zone_name = selected.fields.zone_name;
        let names = CalendarNames::from_calendar(&inspection, prefs, &calendar)?;
        Self::finish(
            inspection,
            prefs,
            PatternSelection::Basic(selected),
            names,
            calendar_name,
            iso_calendar,
            hour_cycle,
            range_length,
            zone_name,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn finish<P: BufferProvider + ?Sized>(
        inspection: NumberingInspection<'_, P>,
        prefs: DateTimeFormatterPreferences,
        selection: PatternSelection,
        names: CalendarNames,
        calendar_name: String,
        iso_calendar: bool,
        hour_cycle: DateTimeHourCycle,
        range_length: icu_datetime::options::Length,
        zone_name: Option<ZoneName>,
    ) -> Result<Self, String> {
        let zone_writer = zone_name
            .map(|name| ZoneWriter::new(&inspection, prefs, name))
            .transpose()?;
        // ICU writes r (related ISO year) in ASCII. ECMA-402 uses the selected
        // numbering system for this field, without grouping or year truncation.
        let related_year_formatter = if matches!(calendar_name.as_str(), "chinese" | "dangi") {
            let mut options = icu_decimal::options::DecimalFormatterOptions::default();
            options.grouping_strategy = Some(icu_decimal::options::GroupingStrategy::Never);
            Some(
                icu_decimal::DecimalFormatter::try_new_with_buffer_provider(
                    &inspection,
                    (&prefs).into(),
                    options,
                )
                .map_err(|error| format!("related year numbering data: {error:?}"))?,
            )
        } else {
            None
        };
        let midnight_hour = if hour_cycle == DateTimeHourCycle::H24 {
            let preferences: icu_decimal::DecimalFormatterPreferences = (&prefs).into();
            let decimal = icu_decimal::DecimalFormatter::try_new_with_buffer_provider(
                &inspection,
                preferences,
                Default::default(),
            )
            .map_err(|error| format!("hour numbering data: {error:?}"))?;
            Some(
                writeable::Writeable::write_to_string(
                    &decimal.format(&fixed_decimal::Decimal::from(24)),
                )
                .into_owned(),
            )
        } else {
            None
        };
        let numbering_system = inspection
            .numbering_system
            .into_inner()
            .ok_or_else(|| "numbering system was not loaded".to_owned())?;
        Ok(Self {
            selection,
            range_names_template: names.clone(),
            names,
            numbering_system,
            calendar_name,
            iso_calendar,
            hour_cycle,
            midnight_hour,
            related_year_formatter,
            range_preferences: prefs,
            range_length: match range_length {
                icu_datetime::options::Length::Long => {
                    icu_datetime::provider::neo::marker_attrs::PatternLength::Long
                }
                icu_datetime::options::Length::Medium => {
                    icu_datetime::provider::neo::marker_attrs::PatternLength::Medium
                }
                _ => icu_datetime::provider::neo::marker_attrs::PatternLength::Short,
            },
            range: None,
            range_names: Vec::new(),
            zone_writer,
        })
    }

    pub(super) fn numbering_system(&self) -> &str {
        &self.numbering_system
    }

    pub(super) fn calendar_name(&self) -> String {
        if self.iso_calendar {
            // ISO and Gregorian have identical civil fields. ISO input is
            // retained and Gregorian's CLDR names supply the alias data.
            return "iso8601".to_owned();
        }
        self.calendar_name.clone()
    }

    pub(super) fn hour_cycle(&self) -> &'static str {
        match &self.selection {
            PatternSelection::Basic(selected) => selected.hour_cycle.as_str(),
            PatternSelection::Semantic { .. } => self.hour_cycle.as_str(),
        }
    }

    pub(super) fn selected_fields(&self) -> Option<MatchComponents> {
        match &self.selection {
            PatternSelection::Basic(selected) => Some(selected.fields),
            PatternSelection::Semantic { .. } => None,
        }
    }

    /// Inspects component resolution using an explicit ordinary ISO reference date.
    /// Style formats omit component properties, so value-dependent style patterns
    /// are never reported as a fixed component width.
    pub(super) fn resolved_pattern(
        &self,
        widths: ComponentWidths,
    ) -> icu_datetime::pattern::DateTimePattern {
        let input = DateTime {
            date: icu_calendar::Date::try_new_iso(1970, 1, 1).expect("valid ISO reference date"),
            time: icu_datetime::input::Time::start_of_day(),
        };
        self.semantic_pattern_for(&input, widths)
            .expect("Basic formats resolve directly from their selected field metadata")
    }

    /// Formats explicit ISO date/time input, preserving locale and calendar data.
    /// Pattern choice may depend on the input year and era, so it is performed
    /// for each input before applying the explicitly requested component widths.
    pub(super) fn format<P: BufferProvider + ?Sized>(
        &mut self,
        provider: &P,
        datetime: &DateTime<Iso>,
        widths: ComponentWidths,
        offset_seconds: i32,
    ) -> Result<String, String> {
        let semantic = self.semantic_pattern_for(datetime, widths).map(|pattern| {
            if self.related_year_formatter.is_some() {
                related_year_pattern(pattern)
            } else {
                (pattern, false)
            }
        });
        let (pattern, related_year) = match &self.selection {
            PatternSelection::Basic(selected) => (selected.pattern(), selected.related_year),
            PatternSelection::Semantic { .. } => {
                let (pattern, related_year) = semantic.as_ref().expect("semantic pattern selected");
                (pattern, *related_year)
            }
        };
        let needs_parts = (related_year && self.related_year_formatter.is_some())
            || (self.hour_cycle == DateTimeHourCycle::H24 && datetime.time.hour.number() == 0);
        let mut value = if needs_parts {
            let parts = Self::parts_for_pattern(
                &mut self.names,
                provider,
                datetime,
                pattern,
                self.related_year_formatter.as_ref(),
                self.midnight_hour.as_deref(),
            )?;
            super::parts::join(parts)
        } else {
            self.names.format(provider, pattern, datetime)?
        };
        self.append_zone_text(&mut value, offset_seconds)?;
        Ok(value)
    }

    /// Formats the same adjusted pattern as `format`, retaining ICU field spans.
    pub(super) fn format_parts<P: BufferProvider + ?Sized>(
        &mut self,
        provider: &P,
        datetime: &DateTime<Iso>,
        widths: ComponentWidths,
        offset_seconds: i32,
    ) -> Result<Vec<DateTimePart>, String> {
        let semantic = self.semantic_pattern_for(datetime, widths);
        let pattern = match &self.selection {
            PatternSelection::Basic(selected) => selected.pattern(),
            PatternSelection::Semantic { .. } => {
                semantic.as_ref().expect("semantic pattern selected")
            }
        };
        let mut parts = Self::parts_for_pattern(
            &mut self.names,
            provider,
            datetime,
            pattern,
            self.related_year_formatter.as_ref(),
            self.midnight_hour.as_deref(),
        )?;
        self.append_zone(&mut parts, offset_seconds)?;
        Ok(parts)
    }

    /// Formats owned interval parts with only temporary provider/calendar borrows.
    /// Ordinary formatting does not load the interval marker or its raw maps.
    pub(super) fn format_range_parts<P: BufferProvider + ?Sized>(
        &mut self,
        provider: &P,
        start: &DateTime<Iso>,
        end: &DateTime<Iso>,
        widths: ComponentWidths,
        start_offset_seconds: i32,
        end_offset_seconds: i32,
    ) -> Result<Vec<RangePart>, String> {
        let selected = |input| -> super::range_selection::SharedPattern {
            match &self.selection {
                PatternSelection::Basic(selected) => selected.range_pattern(),
                PatternSelection::Semantic { .. } => Arc::new(
                    self.semantic_pattern_for(input, widths)
                        .expect("semantic pattern selected")
                        .into(),
                ),
            }
        };
        let first_pattern = selected(start);
        let last_pattern = selected(end);
        let fields = super::range_pattern::PatternFields::new(&first_pattern)?;
        let first = self.names.range_fields(start);
        let last = self.names.range_fields(end);
        let difference = first.difference(&last, &fields);
        let plan = if difference.is_none() && first_pattern == last_pattern {
            Arc::new(RangePlan::Single(first_pattern))
        } else {
            if self.range.is_none() {
                self.range = Some(RangeData::load(
                    provider,
                    self.range_preferences,
                    &self.calendar_name,
                    self.range_length,
                )?);
            }
            let interval_skeleton = match &self.selection {
                PatternSelection::Basic(selected) => Some(selected.interval_skeleton()),
                PatternSelection::Semantic {
                    interval_skeleton, ..
                } => interval_skeleton.as_deref(),
            };
            self.range.as_mut().expect("range data loaded").plan(
                first_pattern,
                last_pattern,
                &first,
                &last,
                difference.unwrap_or(super::range_fields::Difference::Era),
                interval_skeleton,
            )?
        };
        let mut parts = super::range_parts::render(&plan, &mut |pattern, is_end| {
            let position = self
                .range_names
                .iter()
                .position(|entry| entry.pattern.as_ref() == pattern);
            let position = if let Some(position) = position {
                position
            } else {
                if self.range_names.len() == 16 {
                    self.range_names.remove(0);
                }
                self.range_names.push(RangeNames {
                    pattern: Arc::new(pattern.clone()),
                    writer_pattern: Arc::new(pattern.clone().into()),
                    names: self.range_names_template.clone(),
                });
                self.range_names.len() - 1
            };
            let entry = &mut self.range_names[position];
            Self::parts_for_pattern(
                &mut entry.names,
                provider,
                if is_end { end } else { start },
                &entry.writer_pattern,
                self.related_year_formatter.as_ref(),
                self.midnight_hour.as_deref(),
            )
        })?;
        self.append_range_zone(&mut parts, start_offset_seconds, end_offset_seconds)?;
        Ok(parts)
    }

    fn append_zone(
        &self,
        parts: &mut Vec<DateTimePart>,
        offset_seconds: i32,
    ) -> Result<(), String> {
        let Some(writer) = &self.zone_writer else {
            return Ok(());
        };
        if !parts.is_empty() {
            parts.push(DateTimePart {
                kind: super::parts::PartKind::Literal,
                value: " ".to_owned(),
            });
        }
        parts.extend(writer.format_parts(offset_seconds)?);
        Ok(())
    }

    fn append_zone_text(&self, value: &mut String, offset_seconds: i32) -> Result<(), String> {
        let Some(writer) = &self.zone_writer else {
            return Ok(());
        };
        if !value.is_empty() {
            value.push(' ');
        }
        writer.write_to(offset_seconds, value)
    }

    fn append_range_zone(
        &self,
        parts: &mut Vec<RangePart>,
        start_offset_seconds: i32,
        end_offset_seconds: i32,
    ) -> Result<(), String> {
        let Some(writer) = &self.zone_writer else {
            return Ok(());
        };
        if !parts.is_empty() {
            parts.push(RangePart {
                part: DateTimePart {
                    kind: super::parts::PartKind::Literal,
                    value: " ".to_owned(),
                },
                source: RangeSource::Shared,
            });
        }
        if start_offset_seconds == end_offset_seconds {
            parts.extend(
                writer
                    .format_parts(start_offset_seconds)?
                    .into_iter()
                    .map(|part| RangePart {
                        part,
                        source: RangeSource::Shared,
                    }),
            );
        } else {
            parts.extend(
                writer
                    .format_parts(start_offset_seconds)?
                    .into_iter()
                    .map(|part| RangePart {
                        part,
                        source: RangeSource::Start,
                    }),
            );
            parts.push(RangePart {
                part: DateTimePart {
                    kind: super::parts::PartKind::Literal,
                    value: " – ".to_owned(),
                },
                source: RangeSource::Shared,
            });
            parts.extend(
                writer
                    .format_parts(end_offset_seconds)?
                    .into_iter()
                    .map(|part| RangePart {
                        part,
                        source: RangeSource::End,
                    }),
            );
        }
        Ok(())
    }

    /// Uses one already selected owned pattern for both field corrections.
    fn parts_for_pattern<P: BufferProvider + ?Sized>(
        names: &mut CalendarNames,
        provider: &P,
        datetime: &DateTime<Iso>,
        pattern: &icu_datetime::pattern::DateTimePattern,
        related_year_formatter: Option<&icu_decimal::DecimalFormatter>,
        midnight_hour: Option<&str>,
    ) -> Result<Vec<DateTimePart>, String> {
        let mut parts = names.format_parts(provider, pattern, datetime, related_year_formatter)?;
        if datetime.time.hour.number() == 0 {
            if let Some(hour) = midnight_hour {
                super::parts::replace_hour(&mut parts, hour);
            }
        }
        Ok(parts)
    }

    /// Semantic/style patterns remain input-dependent. Basic formats borrow
    /// their fixed pattern directly and never allocate a pattern for this call.
    fn semantic_pattern_for(
        &self,
        datetime: &DateTime<Iso>,
        widths: ComponentWidths,
    ) -> Option<icu_datetime::pattern::DateTimePattern> {
        let (formatter, calendar_period_time, calendar_component_pattern) = match &self.selection {
            PatternSelection::Basic(_) => return None,
            PatternSelection::Semantic {
                formatter,
                calendar_period_time,
                calendar_component_pattern,
                ..
            } => (formatter, calendar_period_time, calendar_component_pattern),
        };
        let date = calendar_component_pattern.as_ref().map_or_else(
            || formatter.format(datetime).pattern(),
            CalendarComponentPattern::pattern,
        );
        let pattern = if let Some(time) = calendar_period_time {
            time.combine(date, datetime)
        } else {
            date
        };
        Some(widths.apply(pattern))
    }
}

/// Inspects typed fields by moving the owned pattern into and out of ICU's
/// public runtime representation; neither pattern text nor output is parsed.
fn related_year_pattern(
    pattern: icu_datetime::pattern::DateTimePattern,
) -> (icu_datetime::pattern::DateTimePattern, bool) {
    use icu_datetime::provider::{
        fields::{FieldSymbol, Year},
        pattern::{PatternItem, runtime::Pattern},
    };
    let native: Pattern<'static> = pattern.into();
    let related_year = native.items.iter().any(|item| {
        matches!(item, PatternItem::Field(field) if field.symbol == FieldSymbol::Year(Year::RelatedIso))
    });
    (native.into(), related_year)
}

fn calendar_identifier(calendar: &AnyCalendar) -> String {
    calendar
        .calendar_algorithm()
        .map(|value| icu_locale::extensions::unicode::Value::from(value).to_string())
        .unwrap_or_else(|| "gregory".to_owned())
}
