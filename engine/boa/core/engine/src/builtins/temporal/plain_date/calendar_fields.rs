//! Resolve owned non-ISO date fields before constructing the final ISO date.
use icu_calendar::{AnyCalendar, AnyCalendarKind, Date, DateDuration};
use temporal_rs::{
    PlainDate, TemporalError, TemporalResult, options::Overflow, partial::PartialDate,
};

/// Temporal 0.1 applies day overflow only after calendar construction.
/// Resolve ordinal months and overflow at this boundary without reading
/// JavaScript properties a second time.
pub(super) fn from_partial(
    mut partial: PartialDate,
    overflow: Option<Overflow>,
) -> TemporalResult<PlainDate> {
    let kind = partial.calendar.kind();
    let fields = &partial.calendar_fields;
    if kind == AnyCalendarKind::Iso
        || fields.day.is_none()
        || (fields.month.is_none() && fields.month_code.is_none())
        || (fields.year.is_none() && (fields.era.is_none() || fields.era_year.is_none()))
    {
        return PlainDate::from_partial(partial, overflow);
    }
    let overflow = overflow.unwrap_or(Overflow::Constrain);
    let requested_day = partial.calendar_fields.day.expect("checked date field");
    let requested_month = partial.calendar_fields.month;
    let requested_code = partial.calendar_fields.month_code;
    partial.calendar_fields.day = Some(1);
    let ordinal = matches!(
        kind,
        AnyCalendarKind::Chinese | AnyCalendarKind::Dangi | AnyCalendarKind::Hebrew
    );
    if ordinal && requested_code.is_none() {
        partial.calendar_fields.month = Some(1);
    } else if requested_code.is_some() {
        partial.calendar_fields.month = None;
    }
    let calendar = partial.calendar.clone();
    let first = PlainDate::from_partial(partial, Some(overflow))?;
    let iso = first.with_calendar(temporal_rs::Calendar::default());
    let mut first = Date::try_new_iso(iso.year(), iso.month(), iso.day())
        .map_err(|_| {
            TemporalError::range().with_message("invalid ISO date while resolving calendar fields")
        })?
        .to_calendar(AnyCalendar::new(kind));
    if ordinal && requested_code.is_none() {
        let month = requested_month.expect("checked date field");
        let maximum = first.months_in_year();
        if month == 0 || (month > maximum && overflow == Overflow::Reject) {
            return Err(TemporalError::range().with_message("month out of range"));
        }
        first.add(DateDuration::new(
            0,
            i32::from(month.min(maximum)) - 1,
            0,
            0,
        ));
    } else if let (true, Some(month)) = (requested_code.is_some(), requested_month) {
        if first.month().ordinal != month {
            return Err(TemporalError::range().with_message("month and monthCode disagree"));
        }
    }
    let maximum = first.days_in_month();
    if requested_day == 0 || (requested_day > maximum && overflow == Overflow::Reject) {
        return Err(TemporalError::range().with_message("day out of range"));
    }
    first.add(DateDuration::new(
        0,
        0,
        0,
        i32::from(requested_day.min(maximum)) - 1,
    ));
    let iso = first.to_iso();
    PlainDate::new(
        iso.extended_year(),
        iso.month().ordinal,
        iso.day_of_month().0,
        calendar,
    )
}

/// Chinese and Dangi arithmetic years use the related ISO year, while the
/// dependency exposes ICU's historical extended-year epoch.
pub(super) fn arithmetic_year(date: &PlainDate) -> i32 {
    let kind = date.calendar().kind();
    if !matches!(kind, AnyCalendarKind::Chinese | AnyCalendarKind::Dangi) {
        return date.year();
    }
    let iso = date.with_calendar(temporal_rs::Calendar::default());
    Date::try_new_iso(iso.year(), iso.month(), iso.day())
        .expect("PlainDate contains valid ISO fields")
        .to_calendar(AnyCalendar::new(kind))
        .year()
        .cyclic()
        .expect("Chinese and Dangi have cyclic calendar years")
        .related_iso
}
