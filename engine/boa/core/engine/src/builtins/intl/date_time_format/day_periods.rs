//! Owned flexible-period data; selection depends only on explicit wall time.
use super::pattern::ComponentWidths;
use boa_intl_data::{DayPeriods, OmoikaneDayPeriodsV1};
use icu_datetime::{DateTimeFormatterPreferences, input::Time, provider::fields::FieldLength};
use icu_provider::prelude::*;

#[derive(Debug, Clone)]
pub(super) struct FlexiblePeriods(DataPayload<OmoikaneDayPeriodsV1>);

impl FlexiblePeriods {
    pub(super) fn replace(
        &self,
        parts: &mut [super::parts::DateTimePart],
        time: &Time,
        widths: ComponentWidths,
    ) -> Result<(), String> {
        let Some(width) = widths.day_period else {
            return Ok(());
        };
        let name = self.name(time, width, widths)?;
        for part in parts {
            if part.kind == super::parts::PartKind::DayPeriod {
                part.value.clear();
                part.value.push_str(name);
            }
        }
        Ok(())
    }

    pub(super) fn load<P: BufferProvider + ?Sized>(
        provider: &P,
        preferences: DateTimeFormatterPreferences,
    ) -> Result<Self, String> {
        let locale = OmoikaneDayPeriodsV1::INFO.make_locale(preferences.locale_preferences);
        DataProvider::<OmoikaneDayPeriodsV1>::load(
            &provider.as_deserializing(),
            DataRequest {
                id: DataIdentifierBorrowed::for_locale(&locale),
                ..Default::default()
            },
        )
        .map(|response| Self(response.payload))
        .map_err(|error| format!("flexible day-period data: {error:?}"))
    }

    pub(super) fn period(&self, time: &Time, widths: ComponentWidths) -> &str {
        // Singular periods follow displayed clock fields: omitted minutes and
        // seconds cannot distinguish a singular point from the surrounding hour.
        let nanos = u64::from(time.hour.number()) * 3_600_000_000_000
            + if widths.minute.is_some() {
                u64::from(time.minute.number()) * 60_000_000_000
            } else {
                0
            }
            + if widths.second.is_some() {
                u64::from(time.second.number()) * 1_000_000_000
            } else {
                0
            };
        select(self.0.get(), nanos).unwrap_or(if time.hour.number() < 12 { "am" } else { "pm" })
    }

    pub(super) fn name(
        &self,
        time: &Time,
        length: FieldLength,
        widths: ComponentWidths,
    ) -> Result<&str, String> {
        let data = self.0.get();
        let period = self.period(time, widths);
        let width = match length {
            FieldLength::Four => "wide",
            FieldLength::Five => "narrow",
            _ => "abbreviated",
        };
        data.name(width, period)
            .or_else(|| data.name("wide", period))
            .ok_or_else(|| format!("missing flexible day-period name: {width}/{period}"))
    }
}

fn bound(data: &DayPeriods<'_>, period: &str, key: &str) -> Option<u64> {
    let value = data.rules.get_2d(period, key)?;
    let (hour, minute) = value.split_once(':')?;
    Some((hour.parse::<u64>().ok()? * 60 + minute.parse::<u64>().ok()?) * 60_000_000_000)
}

fn select<'a>(data: &'a DayPeriods<'_>, nanos: u64) -> Option<&'a str> {
    // TR35 allows callers without 00:00/24:00 context to omit ambiguous midnight.
    // Keep the source rule intact and select its surrounding variable period.
    for entry in data.rules.iter0() {
        let period = entry.key0();
        if period != "midnight" && bound(data, period, "_at") == Some(nanos) {
            return Some(period);
        }
    }
    for entry in data.rules.iter0() {
        let period = entry.key0();
        let (start, inclusive) = if let Some(start) = bound(data, period, "_from") {
            (start, true)
        } else if let Some(start) = bound(data, period, "_after") {
            (start, false)
        } else {
            continue;
        };
        let (end, end_inclusive) = if let Some(end) = bound(data, period, "_before") {
            (end, false)
        } else if let Some(end) = bound(data, period, "_to") {
            (end, true)
        } else {
            continue;
        };
        let after_start = nanos > start || inclusive && nanos == start;
        let before_end = nanos < end || end_inclusive && nanos == end;
        if if start < end {
            after_start && before_end
        } else {
            after_start || before_end
        } {
            return Some(period);
        }
    }
    None
}
