//! Calendar rounding windows anchored to the original relative date.

use temporal_rs::{
    Duration, PlainDateTime,
    options::{DifferenceSettings, RelativeTo, RoundingOptions, Unit, UnsignedRoundingMode},
};

use crate::{JsNativeError, JsResult};

use super::precision::{
    duration_sign, largest_unit, rational_to_number, time_nanoseconds, unit_nanoseconds,
};

/// Recompute totals using exact integer ratios after the native operation has
/// performed its option and range validation. Zoned calendars retain their
/// provider-aware native path.
pub(super) fn exact_total(
    duration: &Duration,
    unit: Unit,
    relative: Option<&RelativeTo>,
) -> JsResult<Option<f64>> {
    if duration_sign(duration) == 0 {
        return Ok(Some(0.0));
    }
    match relative {
        None => Ok(unit_nanoseconds(unit).map(|divisor| {
            let time =
                time_nanoseconds(duration) + i128::from(duration.days()) * 86_400_000_000_000;
            rational_to_number(time, divisor)
        })),
        Some(RelativeTo::PlainDate(date)) => {
            let origin = date.to_plain_date_time(None)?;
            let target = origin.add(duration, None)?;
            if matches!(unit, Unit::Year | Unit::Month) {
                let window = calendar_window(&origin, &target, unit, 1)?;
                Ok(Some(rational_to_number(window.numerator(), window.span)))
            } else {
                let Some(divisor) = unit_nanoseconds(unit) else {
                    return Ok(None);
                };
                Ok(Some(rational_to_number(
                    difference_nanoseconds(&origin, &target)?,
                    divisor,
                )))
            }
        }
        Some(RelativeTo::ZonedDateTime(_)) => Ok(None),
    }
}

/// Rounding uses distances to calendar-unit boundaries, not a binary64 total.
/// Only the plain calendar route needs replacement for the native 0.1 kernel.
pub(super) fn exact_round(
    duration: &Duration,
    options: RoundingOptions,
    relative: Option<&RelativeTo>,
    validated: Duration,
) -> JsResult<Duration> {
    if duration_sign(duration) == 0 {
        return Ok(Duration::default());
    }
    let Some(RelativeTo::PlainDate(date)) = relative else {
        return Ok(validated);
    };
    let Some(unit @ (Unit::Year | Unit::Month)) = options.smallest_unit else {
        return Ok(validated);
    };
    let origin = date.to_plain_date_time(None)?;
    let target = origin.add(duration, None)?;
    let increment = options.increment.unwrap_or_default().get();
    let window = calendar_window(&origin, &target, unit, increment)?;
    let positive = window.sign > 0;
    let mode = options
        .rounding_mode
        .unwrap_or_default()
        .get_unsigned_round_mode(positive);
    let distance = window.distance.unsigned_abs();
    let span = window.span as u128;
    // An exact upper endpoint belongs to r2, even for truncation. This is
    // NudgeToCalendarUnit's progress = 1 case, before unsigned rounding.
    let expand = distance == span
        || distance != 0
            && match mode {
                UnsignedRoundingMode::Infinity => true,
                UnsignedRoundingMode::Zero => false,
                UnsignedRoundingMode::HalfInfinity => distance >= span - distance,
                UnsignedRoundingMode::HalfZero => distance > span - distance,
                UnsignedRoundingMode::HalfEven => {
                    distance > span - distance
                        || (distance == span - distance
                            && (window.whole / i64::from(increment)) % 2 != 0)
                }
            };
    let units = window.whole
        + if expand {
            i64::from(increment) * i64::from(window.sign)
        } else {
            0
        };
    let largest = options
        .largest_unit
        .filter(|unit| *unit != Unit::Auto)
        .unwrap_or(largest_unit(duration).max(unit));
    rounded_calendar_duration(&origin, unit, units, largest)
}

/// Owned mathematical state; no formatter, provider or Context borrow escapes.
struct CalendarWindow {
    whole: i64,
    span: i128,
    distance: i128,
    sign: i8,
}

impl CalendarWindow {
    fn numerator(&self) -> i128 {
        i128::from(self.whole) * self.span + self.distance
    }

    /// Exact endpoints count as a complete unit in either direction.
    fn complete_units(&self) -> i64 {
        self.whole
            + if self.distance.abs() == self.span {
                i64::from(self.sign)
            } else {
                0
            }
    }
}

fn calendar_window(
    origin: &PlainDateTime,
    target: &PlainDateTime,
    unit: Unit,
    increment: u32,
) -> JsResult<CalendarWindow> {
    calendar_window_after_years(origin, target, unit, increment, 0)
}

/// Retain the original day when balancing a rounded month endpoint to years.
/// Adding years first and then months would lose a constrained leap-day anchor.
fn calendar_window_after_years(
    origin: &PlainDateTime,
    target: &PlainDateTime,
    unit: Unit,
    increment: u32,
    years: i64,
) -> JsResult<CalendarWindow> {
    let anchor = origin.add(&calendar_duration_after_years(unit, 0, years)?, None)?;
    let mut settings = DifferenceSettings::default();
    settings.largest_unit = Some(unit);
    let difference = anchor.until(target, settings)?;
    let units = match unit {
        Unit::Year => difference.years(),
        Unit::Month => difference.months(),
        _ => unreachable!(),
    };
    let sign = match origin.compare_iso(target) {
        std::cmp::Ordering::Less => 1,
        std::cmp::Ordering::Greater => -1,
        std::cmp::Ordering::Equal => 0,
    };
    let mut whole = units / i64::from(increment) * i64::from(increment);
    // A constrained date may sit on the wrong side of the target's time. Move
    // to the adjacent window while keeping every boundary anchored to origin.
    let mut start = origin.add(&calendar_duration_after_years(unit, whole, years)?, None)?;
    let mut distance = difference_nanoseconds(&start, target)?;
    if distance.signum() != 0 && distance.signum() != i128::from(sign) {
        whole -= i64::from(increment) * i64::from(sign);
        start = origin.add(&calendar_duration_after_years(unit, whole, years)?, None)?;
        distance = difference_nanoseconds(&start, target)?;
    }
    let step = i64::from(increment) * i64::from(if sign == 0 { 1 } else { sign });
    let mut end = origin.add(
        &calendar_duration_after_years(unit, whole + step, years)?,
        None,
    )?;
    let mut span = difference_nanoseconds(&start, &end)?.abs();
    if distance.abs() > span {
        // CalendarDateUntil can undercount a constrained month/year. The
        // specification's additionalShift advances both boundaries from the
        // original date; adding a unit to `end` would drift at month ends.
        whole += step;
        start = end;
        end = origin.add(
            &calendar_duration_after_years(unit, whole + step, years)?,
            None,
        )?;
        distance = difference_nanoseconds(&start, target)?;
        span = difference_nanoseconds(&start, &end)?.abs();
    }
    if span == 0
        || distance.abs() > span
        || (distance != 0 && distance.signum() != i128::from(sign))
    {
        return Err(JsNativeError::range()
            .with_message("Invalid calendar rounding window")
            .into());
    }
    Ok(CalendarWindow {
        whole,
        span,
        distance,
        sign,
    })
}

/// Keep calendar units in the rounded result, rather than redifferencing a
/// constrained endpoint into days. Only month-to-year balancing needs another
/// calendar window; its month boundaries still use the original date.
fn rounded_calendar_duration(
    origin: &PlainDateTime,
    unit: Unit,
    count: i64,
    largest: Unit,
) -> JsResult<Duration> {
    if count == 0 || largest == unit {
        return calendar_duration(unit, count);
    }
    debug_assert_eq!(unit, Unit::Month);
    debug_assert_eq!(largest, Unit::Year);
    let endpoint = origin.add(&calendar_duration(unit, count)?, None)?;
    let years = calendar_window(origin, &endpoint, Unit::Year, 1)?.complete_units();
    let months = calendar_window_after_years(origin, &endpoint, Unit::Month, 1, years)?;
    calendar_duration_after_years(Unit::Month, months.complete_units(), years)
}

fn calendar_duration(unit: Unit, count: i64) -> JsResult<Duration> {
    calendar_duration_after_years(unit, count, 0)
}

fn calendar_duration_after_years(unit: Unit, count: i64, years: i64) -> JsResult<Duration> {
    Duration::new(
        if unit == Unit::Year { count } else { years },
        if unit == Unit::Month { count } else { 0 },
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    )
    .map_err(Into::into)
}

fn difference_nanoseconds(start: &PlainDateTime, end: &PlainDateTime) -> JsResult<i128> {
    let mut settings = DifferenceSettings::default();
    settings.largest_unit = Some(Unit::Nanosecond);
    Ok(start.until(end, settings)?.nanoseconds())
}
