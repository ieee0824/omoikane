//! Native calendar fields and displayed-precision comparisons for intervals.
use super::range_pattern::{FieldId, PatternFields};
use icu_calendar::{AsCalendar, Date};
use icu_datetime::input::Time;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct RangeFields {
    era: Option<tinystr::TinyStr16>,
    year: i32,
    cyclic_year: Option<u8>,
    month: icu_calendar::types::MonthCode,
    month_ordinal: u8,
    day: u8,
    weekday: u8,
    hour: u8,
    minute: u8,
    second: u8,
    nanos: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Difference {
    Era,
    Year,
    Month,
    Day,
    Period,
    Hour,
    Minute,
    Second,
    Fraction,
}

impl Difference {
    pub(super) fn key(self, twelve_hour: bool) -> &'static str {
        match self {
            Self::Era => "G",
            Self::Year => "y",
            Self::Month => "M",
            Self::Day => "d",
            Self::Period => "a",
            Self::Hour if twelve_hour => "h",
            Self::Hour => "H",
            Self::Minute => "m",
            Self::Second => "s",
            Self::Fraction => "S",
        }
    }
}

impl RangeFields {
    pub(super) fn from_date<A: AsCalendar>(date: &Date<A>, time: &Time) -> Self {
        let year = date.year();
        let month = date.month();
        Self {
            era: year.era().map(|year| year.era),
            year: year.era_year_or_related_iso(),
            cyclic_year: year.cyclic().map(|year| year.year),
            month: month.formatting_code,
            month_ordinal: month.ordinal,
            day: date.day_of_month().0,
            weekday: date.day_of_week() as u8,
            hour: time.hour.number(),
            minute: time.minute.number(),
            second: time.second.number(),
            nanos: time.subsecond.number(),
        }
    }

    pub(super) fn same_date(&self, other: &Self) -> bool {
        self.era == other.era
            && self.year == other.year
            && self.cyclic_year == other.cyclic_year
            && self.month == other.month
            && self.month_ordinal == other.month_ordinal
            && self.day == other.day
    }

    pub(super) fn difference(&self, other: &Self, selected: &PatternFields) -> Option<Difference> {
        use Difference::*;
        let fraction = selected.fraction;
        let differences = [
            (Era, self.era != other.era),
            (
                Year,
                self.year != other.year || self.cyclic_year != other.cyclic_year,
            ),
            (
                Month,
                self.month != other.month || self.month_ordinal != other.month_ordinal,
            ),
            (Day, self.day != other.day || self.weekday != other.weekday),
            (
                Period,
                selected.has(FieldId::DayPeriod) && self.period(selected) != other.period(selected),
            ),
            (Hour, self.hour != other.hour),
            (Minute, self.minute != other.minute),
            (Second, self.second != other.second),
            (
                Fraction,
                fraction > 0
                    && self.nanos / 10u32.pow(9 - u32::from(fraction))
                        != other.nanos / 10u32.pow(9 - u32::from(fraction)),
            ),
        ];
        let precision = precision(selected);
        differences.into_iter().find_map(|(kind, differs)| {
            (differs && precision.is_some_and(|precision| kind <= precision)).then_some(kind)
        })
    }

    fn period(&self, selected: &PatternFields) -> u8 {
        use icu_datetime::provider::fields::{DayPeriod, FieldSymbol};
        let noon_midnight = selected.fields[FieldId::DayPeriod as usize]
            .is_some_and(|field| field.symbol == FieldSymbol::DayPeriod(DayPeriod::NoonMidnight));
        let precision = precision(selected);
        let exact = (!precision.is_some_and(|p| p >= Difference::Minute) || self.minute == 0)
            && (!precision.is_some_and(|p| p >= Difference::Second) || self.second == 0)
            && (!precision.is_some_and(|p| p >= Difference::Fraction) || self.nanos == 0);
        if noon_midnight && exact && self.hour == 0 {
            2
        } else if noon_midnight && exact && self.hour == 12 {
            3
        } else if self.hour < 12 {
            0
        } else {
            1
        }
    }
}

fn precision(fields: &PatternFields) -> Option<Difference> {
    use Difference::*;
    [
        (FieldId::Era, Era),
        (FieldId::Year, Year),
        (FieldId::RelatedYear, Year),
        (FieldId::YearName, Year),
        (FieldId::Month, Month),
        (FieldId::Day, Day),
        (FieldId::Weekday, Day),
        (FieldId::DayPeriod, Period),
        (FieldId::Hour, Hour),
        (FieldId::Minute, Minute),
        (FieldId::Second, Second),
        (FieldId::Fraction, Fraction),
    ]
    .into_iter()
    .filter_map(|(id, rank)| fields.has(id).then_some(rank))
    .max()
}

#[cfg(test)]
mod tests {
    use super::*;
    use icu_calendar::Iso;
    use icu_datetime::{
        input::DateTime, pattern::DateTimePattern, provider::pattern::runtime::Pattern,
    };

    #[test]
    fn hidden_time_and_truncated_fraction_collapse_but_hidden_larger_date_does_not() {
        let date = Date::<Iso>::try_new_iso(2025, 1, 1).unwrap();
        let a = DateTime {
            date,
            time: Time::start_of_day(),
        };
        let mut x = RangeFields::from_date(&a.date, &a.time);
        let mut y = x;
        y.hour = 1;
        let date: Pattern<'static> = DateTimePattern::try_from_pattern_str("yMd").unwrap().into();
        assert_eq!(x.difference(&y, &PatternFields::new(&date).unwrap()), None);
        let fraction: Pattern<'static> = DateTimePattern::try_from_pattern_str("ss.S")
            .unwrap()
            .into();
        x.nanos = 234_000_000;
        y = x;
        y.nanos = 239_000_000;
        assert_eq!(
            x.difference(&y, &PatternFields::new(&fraction).unwrap()),
            None
        );
        y.nanos = 567_000_000;
        assert_eq!(
            x.difference(&y, &PatternFields::new(&fraction).unwrap()),
            Some(Difference::Fraction)
        );
        y = x;
        y.day += 1;
        assert_eq!(
            x.difference(&y, &PatternFields::new(&fraction).unwrap()),
            Some(Difference::Day)
        );
    }
}
