//! Owned calendar and locale data for native date/time pattern formatting.
use super::{
    calendar_patterns::CalendarComponentPattern,
    calendar_period::CalendarPeriodTime,
    candidates::SelectedFormat,
    hour_cycle::DateTimeHourCycle,
    matcher::{MatchComponents, ZoneName},
    options::{Components, DateTimeFields},
    parts::DateTimePart,
    pattern::ComponentWidths,
    range_data::RangeData,
    range_fields::RangeFields,
    range_parts::{RangePart, RangeSource},
    range_selection::RangePlan,
};
use icu_calendar::{AnyCalendar, AnyCalendarKind, AsCalendar, Iso, Ref, cal};
use icu_datetime::{
    DateTimeFormatter, DateTimeFormatterPreferences,
    fieldsets::enums::{CompositeDateTimeFieldSet, CompositeFieldSet},
    input::DateTime,
    pattern::FixedCalendarDateTimeNames,
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
                names: FixedCalendarDateTimeNames<cal::$calendar, CompositeFieldSet>,
                lunar_months: Option<super::lunisolar_months::LunarMonthNames>,
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
                        let lunar_months = super::lunisolar_months::LunarMonthNames::load(
                            provider, prefs, &calendar_identifier(selected)
                        )?;
                        Ok(Self::$calendar {
                            calendar,
                            lunar_months,
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
                zone: super::zone_input::FullZone,
            ) -> Result<String, String> {
                match self {
                    $( Self::$calendar { calendar, names, .. } => {
                        let converted = icu_datetime::input::ZonedDateTime {
                            date: datetime.date.to_calendar(Ref(&*calendar)),
                            time: datetime.time,
                            zone,
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
                zone: super::zone_input::FullZone,
            ) -> Result<Vec<DateTimePart>, String> {
                match self {
                    $( Self::$calendar { calendar, names, lunar_months } => {
                        let converted = icu_datetime::input::ZonedDateTime {
                            date: datetime.date.to_calendar(Ref(&*calendar)),
                            time: datetime.time,
                            zone,
                        };
                        let formatter = names.load_for_pattern(&provider.as_deserializing(), pattern)
                            .map_err(|error| format!("pattern data: {error:?}"))?;
                        let mut parts = super::parts::collect(&formatter.format(&converted))?;
                        if let (Some(year), Some(numeric)) =
                            (converted.date.year().cyclic(), related_year_formatter)
                        {
                            super::parts::localize_related_year(&mut parts, year.related_iso, numeric);
                        }
                        if let (Some(months), Some(decimal)) = (lunar_months, related_year_formatter) {
                            months.replace(&mut parts, pattern, converted.date.month(), decimal)?;
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
    flexible_periods: Option<super::day_periods::FlexiblePeriods>,
    zone_input: Option<super::zone_input::ZoneInput>,
    fractional_separator: Option<super::fractional::FractionalSeparator>,
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
    /// Selects native provider data using already observed component options.
    pub(super) fn create<P: BufferProvider + ?Sized>(
        provider: &P,
        preferences: DateTimeFormatterPreferences,
        components: &Components,
        iso_calendar: bool,
        cycle: DateTimeHourCycle,
    ) -> Result<Self, String> {
        let mut backend = if components.date_style.is_some() || components.time_style.is_some() {
            Self::new_style(provider, preferences, components, iso_calendar, cycle)
        } else if components.format_matcher == super::matcher::FormatMatcher::Basic
            || components.day_period.is_some()
            || (components.hour.is_none()
                && (components.minute.is_some()
                    || components.second.is_some()
                    || components.fractional_digits.is_some()))
        {
            Self::new_basic(
                provider,
                preferences,
                MatchComponents::requested(components),
                iso_calendar,
                cycle,
            )
        } else {
            let fields = components
                .fields()
                .map_err(|error| format!("date/time fields: {error:?}"))?;
            let zone = components
                .zone_name
                .as_ref()
                .and_then(|name| ZoneName::from_str(&name.to_std_string_escaped()));
            Self::new(provider, preferences, fields, zone, iso_calendar, cycle)
        }?;
        if components.fractional_digits.is_some() {
            backend.fractional_separator = Some(super::fractional::FractionalSeparator::load(
                provider,
                preferences,
                &backend.numbering_system,
            )?);
        }
        Ok(backend)
    }

    pub(super) fn new_style<P: BufferProvider + ?Sized>(
        provider: &P,
        preferences: DateTimeFormatterPreferences,
        options: &Components,
        iso_calendar: bool,
        cycle: DateTimeHourCycle,
    ) -> Result<Self, String> {
        let inspection = NumberingInspection {
            inner: provider,
            numbering_system: Default::default(),
        };
        let calendar = AnyCalendar::try_new_with_buffer_provider(
            &inspection,
            AnyCalendarKind::new((&preferences).into()),
        )
        .map_err(|error| format!("style calendar data: {error:?}"))?;
        let calendar_name = calendar_identifier(&calendar);
        let selected =
            super::style_formats::select(&inspection, preferences, &calendar_name, options, cycle)?;
        let names = CalendarNames::from_calendar(&inspection, preferences, &calendar)?;
        let length = options
            .fields()
            .map_err(|error| format!("style fields: {error:?}"))?
            .length;
        Self::finish(
            inspection,
            preferences,
            PatternSelection::Basic(selected),
            names,
            calendar_name,
            iso_calendar,
            cycle,
            length,
            None,
        )
    }

    pub(super) fn set_inline_zone<P: BufferProvider + ?Sized>(
        &mut self,
        provider: &P,
        identifier: &str,
    ) -> Result<(), String> {
        if matches!(&self.selection, PatternSelection::Basic(selected) if selected.fields.zone_name.is_some())
            && self.zone_writer.is_none()
        {
            self.zone_input = Some(super::zone_input::ZoneInput::load(provider, identifier)?);
        }
        Ok(())
    }

    fn zone_at(
        &self,
        datetime: &DateTime<Iso>,
        offset: i32,
    ) -> Result<super::zone_input::FullZone, String> {
        self.zone_input.as_ref().map_or_else(
            || Ok(super::zone_input::utc_at(datetime)),
            |zone| zone.at(datetime, offset),
        )
    }

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
            flexible_periods: None,
            zone_input: None,
            fractional_separator: None,
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
        if let PatternSelection::Basic(selected) = &self.selection {
            return selected.range_pattern().as_ref().clone().into();
        }
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
        if widths.day_period.is_some() || self.fractional_separator.is_some() {
            return self
                .format_parts(provider, datetime, widths, offset_seconds)
                .map(super::parts::join);
        }
        let semantic = self.semantic_pattern_for(datetime, widths);
        let pattern = match &self.selection {
            PatternSelection::Basic(selected) => selected.pattern(),
            PatternSelection::Semantic { .. } => {
                semantic.as_ref().expect("semantic pattern selected")
            }
        };
        let needs_parts = self.related_year_formatter.is_some()
            || (self.hour_cycle == DateTimeHourCycle::H24 && datetime.time.hour.number() == 0);
        let zone = self.zone_at(datetime, offset_seconds)?;
        let mut value = if needs_parts {
            let parts = Self::parts_for_pattern(
                &mut self.names,
                provider,
                datetime,
                pattern,
                self.related_year_formatter.as_ref(),
                self.midnight_hour.as_deref(),
                zone,
            )?;
            super::parts::join(parts)
        } else {
            self.names.format(provider, pattern, datetime, zone)?
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
        self.prepare_flexible_periods(provider, widths)?;
        let zone = self.zone_at(datetime, offset_seconds)?;
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
            zone,
        )?;
        if let Some(separator) = &self.fractional_separator {
            separator.replace(&mut parts)?;
        }
        self.replace_flexible_periods(&mut parts, datetime, widths)?;
        self.append_zone(&mut parts, offset_seconds)?;
        Ok(parts)
    }

    fn prepare_flexible_periods<P: BufferProvider + ?Sized>(
        &mut self,
        provider: &P,
        widths: ComponentWidths,
    ) -> Result<(), String> {
        if widths.day_period.is_some() && self.flexible_periods.is_none() {
            self.flexible_periods = Some(super::day_periods::FlexiblePeriods::load(
                provider,
                self.range_preferences,
            )?);
        }
        Ok(())
    }

    fn replace_flexible_periods(
        &self,
        parts: &mut [DateTimePart],
        datetime: &DateTime<Iso>,
        widths: ComponentWidths,
    ) -> Result<(), String> {
        if let Some(periods) = &self.flexible_periods {
            periods.replace(parts, &datetime.time, widths)?;
        }
        Ok(())
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
        self.prepare_flexible_periods(provider, widths)?;
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
        let flexible_period_differs = widths.day_period.and_then(|_| {
            self.flexible_periods.as_ref().map(|periods| {
                periods.period(&start.time, widths) != periods.period(&end.time, widths)
            })
        });
        let difference = first.difference_with_period(&last, &fields, flexible_period_differs);
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
        let zones = (
            self.zone_at(start, start_offset_seconds)?,
            self.zone_at(end, end_offset_seconds)?,
        );
        let mut parts = self.render_range_plan(provider, start, end, widths, &plan, zones)?;
        self.append_range_zone(&mut parts, start_offset_seconds, end_offset_seconds)?;
        Ok(parts)
    }

    fn render_range_plan<P: BufferProvider + ?Sized>(
        &mut self,
        provider: &P,
        start: &DateTime<Iso>,
        end: &DateTime<Iso>,
        widths: ComponentWidths,
        plan: &RangePlan,
        zones: (super::zone_input::FullZone, super::zone_input::FullZone),
    ) -> Result<Vec<RangePart>, String> {
        let flexible_periods = &self.flexible_periods;
        let fractional_separator = &self.fractional_separator;
        super::range_parts::render(plan, &mut |pattern, is_end| {
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
            let input = if is_end { end } else { start };
            let mut parts = Self::parts_for_pattern(
                &mut entry.names,
                provider,
                input,
                &entry.writer_pattern,
                self.related_year_formatter.as_ref(),
                self.midnight_hour.as_deref(),
                if is_end { zones.1 } else { zones.0 },
            )?;
            if let Some(separator) = fractional_separator {
                separator.replace(&mut parts)?;
            }
            if let Some(periods) = flexible_periods {
                periods.replace(&mut parts, &input.time, widths)?;
            }
            Ok(parts)
        })
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
        zone: super::zone_input::FullZone,
    ) -> Result<Vec<DateTimePart>, String> {
        let mut parts =
            names.format_parts(provider, pattern, datetime, related_year_formatter, zone)?;
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
        mut widths: ComponentWidths,
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
        // Hebrew yM/yMd patterns use month names even for numeric requests;
        // a standalone M pattern can still be numeric. Preserve that CLDR choice.
        if self.calendar_name == "hebrew"
            && matches!(
                widths.month,
                Some(
                    icu_datetime::provider::fields::FieldLength::One
                        | icu_datetime::provider::fields::FieldLength::Two
                )
            )
            && !matches!(
                icu_datetime::provider::fields::components::Bag::from(&pattern).month,
                Some(
                    icu_datetime::provider::fields::components::Month::Numeric
                        | icu_datetime::provider::fields::components::Month::TwoDigit
                )
            )
        {
            widths.month = None;
        }
        Some(widths.apply(pattern))
    }
}

fn calendar_identifier(calendar: &AnyCalendar) -> String {
    super::negotiation::native_calendar_algorithm(calendar)
        .map(|value| icu_locale::extensions::unicode::Value::from(value).to_string())
        .unwrap_or_else(|| "gregory".to_owned())
}
