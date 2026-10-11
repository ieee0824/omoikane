//! Owned formatting input, converted before any native formatter borrow.
use crate::{Context, JsResult, JsValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(feature = "temporal")]
pub(super) enum Kind {
    Date,
    YearMonth,
    MonthDay,
    Time,
    DateTime,
    Instant,
}

#[cfg(not(feature = "temporal"))]
pub(super) type Kind = std::convert::Infallible;

pub(super) enum Input {
    Number(f64),
    #[cfg(feature = "temporal")]
    Instant(f64),
    #[cfg(feature = "temporal")]
    Plain {
        kind: Kind,
        datetime: icu_datetime::input::DateTime<icu_calendar::Iso>,
        calendar: Option<&'static str>,
    },
    #[cfg(feature = "temporal")]
    UnsupportedTemporal,
}

impl Input {
    pub(super) fn same_type(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other) && self.kind() == other.kind()
    }

    pub(super) fn kind(&self) -> Option<Kind> {
        match self {
            Self::Number(_) => None,
            #[cfg(feature = "temporal")]
            Self::Instant(_) => Some(Kind::Instant),
            #[cfg(feature = "temporal")]
            Self::Plain { kind, .. } => Some(*kind),
            #[cfg(feature = "temporal")]
            Self::UnsupportedTemporal => None,
        }
    }
}

/// Recognizes native Temporal brands without reading author properties. All
/// other defined values undergo exactly one observable numeric conversion.
pub(super) fn read(value: &JsValue, context: &mut Context) -> JsResult<Input> {
    let _value_root = value.as_object().map(|object| object.root());
    #[cfg(feature = "temporal")]
    if let Some(object) = value.as_object() {
        if let Some(input) = temporal_input(&object, context)? {
            return Ok(input);
        }
    }
    if value.is_undefined() {
        Ok(Input::Number(
            context.clock().now().millis_since_epoch() as f64
        ))
    } else {
        value.to_number(context).map(Input::Number)
    }
}

#[cfg(feature = "temporal")]
fn temporal_input(object: &crate::JsObject, context: &Context) -> JsResult<Option<Input>> {
    use crate::builtins::temporal::{Instant, PlainDate, PlainDateTime, PlainTime, ZonedDateTime};
    if let Some(value) = object.downcast_ref::<Instant>() {
        return Ok(Some(
            Input::Instant(value.inner.epoch_milliseconds() as f64),
        ));
    }
    if object.downcast_ref::<ZonedDateTime>().is_some() {
        return Ok(Some(Input::UnsupportedTemporal));
    }
    if let Some(value) = object.downcast_ref::<PlainDateTime>() {
        return plain_datetime(
            Kind::DateTime,
            value.inner(),
            Some(value.inner().calendar().identifier()),
        )
        .map(Some);
    }
    if let Some(value) = object.downcast_ref::<PlainDate>() {
        let noon =
            temporal_rs::PlainTime::try_new(12, 0, 0, 0, 0, 0).expect("noon is a valid wall time");
        let datetime = value.inner.to_plain_date_time(Some(noon))?;
        return plain_datetime(
            Kind::Date,
            &datetime,
            Some(value.inner.calendar().identifier()),
        )
        .map(Some);
    }
    if let Some(value) = object.downcast_ref::<PlainTime>() {
        let datetime = temporal_rs::PlainDateTime::try_new_iso(
            1970,
            1,
            1,
            value.inner().hour(),
            value.inner().minute(),
            value.inner().second(),
            value.inner().millisecond(),
            value.inner().microsecond(),
            value.inner().nanosecond(),
        )?;
        return plain_datetime(Kind::Time, &datetime, None).map(Some);
    }
    calendar_period(object, context)
}

#[cfg(feature = "temporal")]
fn plain_datetime(
    kind: Kind,
    value: &temporal_rs::PlainDateTime,
    calendar: Option<&'static str>,
) -> JsResult<Input> {
    let date =
        icu_calendar::Date::try_new_iso(value.iso_year(), value.iso_month(), value.iso_day())
            .map_err(|error| {
                crate::JsNativeError::range().with_message(format!("date fields: {error:?}"))
            })?;
    let nanosecond = u32::from(value.millisecond()) * 1_000_000
        + u32::from(value.microsecond()) * 1_000
        + u32::from(value.nanosecond());
    let time = icu_datetime::input::Time::try_new(
        value.hour(),
        value.minute(),
        value.second(),
        nanosecond,
    )
    .map_err(|error| {
        crate::JsNativeError::range().with_message(format!("time fields: {error:?}"))
    })?;
    Ok(Input::Plain {
        kind,
        datetime: icu_datetime::input::DateTime { date, time },
        calendar,
    })
}

#[cfg(feature = "temporal")]
fn calendar_period(object: &crate::JsObject, context: &Context) -> JsResult<Option<Input>> {
    use crate::builtins::temporal::{PlainMonthDay, PlainYearMonth};
    if let Some(value) = object.downcast_ref::<PlainYearMonth>() {
        return year_month(&value.inner, context).map(Some);
    }
    let (kind, epoch, calendar) = if let Some(value) = object.downcast_ref::<PlainMonthDay>() {
        (
            Kind::MonthDay,
            value.inner.epoch_ns_for_with_provider(
                temporal_rs::TimeZone::utc_with_provider(context.tz_provider()),
                context.tz_provider(),
            )?,
            value.inner.calendar_id(),
        )
    } else {
        return Ok(None);
    };
    let datetime = crate::builtins::date::Date::intl_utc_datetime(
        epoch.as_i128().div_euclid(1_000_000) as f64,
    )?;
    Ok(Some(Input::Plain {
        kind,
        datetime,
        calendar: Some(calendar),
    }))
}

/// ISO and Gregorian reference fields are already ISO civil fields. Construct
/// them directly because minimum YearMonth references precede Instant limits.
#[cfg(feature = "temporal")]
fn year_month(value: &temporal_rs::PlainYearMonth, context: &Context) -> JsResult<Input> {
    let datetime = if matches!(value.calendar_id(), "iso8601" | "gregory") {
        let date =
            icu_calendar::Date::try_new_iso(value.year(), value.month(), value.reference_day())
                .map_err(|error| {
                    crate::JsNativeError::range().with_message(format!("date fields: {error:?}"))
                })?;
        let time =
            icu_datetime::input::Time::try_new(12, 0, 0, 0).expect("noon is a valid wall time");
        icu_datetime::input::DateTime { date, time }
    } else {
        let epoch = value.epoch_ns_for_with_provider(
            temporal_rs::TimeZone::utc_with_provider(context.tz_provider()),
            context.tz_provider(),
        )?;
        crate::builtins::date::Date::intl_utc_datetime(epoch.as_i128().div_euclid(1_000_000) as f64)?
    };
    Ok(Input::Plain {
        kind: Kind::YearMonth,
        datetime,
        calendar: Some(value.calendar_id()),
    })
}
