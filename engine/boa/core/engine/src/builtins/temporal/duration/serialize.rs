//! ISO duration serialization including nonzero fractional-only time fields.

use std::fmt::Write;

use temporal_rs::{
    Duration,
    options::{ToStringRoundingOptions, Unit, UnsignedRoundingMode},
    parsers::Precision,
};

use crate::{JsNativeError, JsResult};

use super::precision::{duration_sign, largest_unit, time_nanoseconds, unit_nanoseconds};

pub(super) fn temporal_string(
    duration: &Duration,
    options: ToStringRoundingOptions,
) -> JsResult<String> {
    let precision = options.precision;
    let smallest = options.smallest_unit;
    let mode = options
        .rounding_mode
        .unwrap_or(temporal_rs::options::RoundingMode::Trunc);
    let precision = match smallest {
        Some(Unit::Second) => Precision::Digit(0),
        Some(Unit::Millisecond) => Precision::Digit(3),
        Some(Unit::Microsecond) => Precision::Digit(6),
        Some(Unit::Nanosecond) => Precision::Digit(9),
        None if matches!(precision, Precision::Auto | Precision::Digit(0..=9)) => precision,
        _ => {
            return Err(JsNativeError::range()
                .with_message("invalid duration string precision or smallest unit")
                .into());
        }
    };
    let increment = match precision {
        Precision::Digit(digits) => 10_i128.pow(u32::from(9 - digits)),
        _ => 1,
    };
    let rounded = if increment == 1 {
        None
    } else {
        let negative = duration_sign(duration) < 0;
        let absolute = time_nanoseconds(duration).unsigned_abs();
        let divisor = increment as u128;
        let quotient = absolute / divisor;
        let remainder = absolute % divisor;
        let mode = mode.get_unsigned_round_mode(!negative);
        let expand = remainder != 0
            && match mode {
                UnsignedRoundingMode::Infinity => true,
                UnsignedRoundingMode::Zero => false,
                UnsignedRoundingMode::HalfInfinity => remainder >= divisor - remainder,
                UnsignedRoundingMode::HalfZero => remainder > divisor - remainder,
                UnsignedRoundingMode::HalfEven => {
                    remainder > divisor - remainder
                        || (remainder == divisor - remainder && quotient % 2 != 0)
                }
            };
        let rounded = ((quotient + u128::from(expand)) * divisor) as i128;
        Some(balance_rounded_time(
            duration,
            rounded * if negative { -1 } else { 1 },
        )?)
    };
    Ok(format_iso(rounded.as_ref().unwrap_or(duration), precision))
}

fn balance_rounded_time(duration: &Duration, mut time: i128) -> JsResult<Duration> {
    let largest = largest_unit(duration).max(Unit::Second);
    let mut components = [0_i64; 4];
    for (index, unit) in [Unit::Day, Unit::Hour, Unit::Minute, Unit::Second]
        .into_iter()
        .enumerate()
    {
        if largest >= unit {
            let divisor = unit_nanoseconds(unit).expect("fixed-length unit");
            components[index] = (time / divisor) as i64;
            time %= divisor;
        }
    }
    Duration::new(
        duration.years(),
        duration.months(),
        duration.weeks(),
        duration.days() + components[0],
        components[1],
        components[2],
        components[3],
        0,
        0,
        time,
    )
    .map_err(Into::into)
}

fn format_iso(duration: &Duration, precision: Precision) -> String {
    let duration_abs = duration.abs();
    let mut output = String::with_capacity(48);
    if duration_sign(duration) < 0 {
        output.push('-');
    }
    output.push('P');
    for (value, suffix) in [
        (duration_abs.years(), 'Y'),
        (duration_abs.months(), 'M'),
        (duration_abs.weeks(), 'W'),
        (duration_abs.days(), 'D'),
    ] {
        if value != 0 {
            write!(&mut output, "{value}{suffix}").expect("String writes cannot fail");
        }
    }
    let seconds_ns = i128::from(duration_abs.seconds()) * 1_000_000_000
        + i128::from(duration_abs.milliseconds()) * 1_000_000
        + duration_abs.microseconds() * 1000
        + duration_abs.nanoseconds();
    let second = seconds_ns / 1_000_000_000;
    let fraction = seconds_ns % 1_000_000_000;
    let hours = duration_abs.hours();
    let minutes = duration_abs.minutes();
    let write_second =
        seconds_ns != 0 || duration_sign(duration) == 0 || matches!(precision, Precision::Digit(_));
    if hours != 0 || minutes != 0 || write_second {
        output.push('T');
        if hours != 0 {
            write!(&mut output, "{hours}H").expect("String writes cannot fail");
        }
        if minutes != 0 {
            write!(&mut output, "{minutes}M").expect("String writes cannot fail");
        }
        if write_second {
            write!(&mut output, "{second}").expect("String writes cannot fail");
            let digits = match precision {
                Precision::Digit(digits) => usize::from(digits),
                _ if fraction == 0 => 0,
                _ => 9,
            };
            if digits != 0 {
                output.push('.');
                let fraction_start = output.len();
                write!(&mut output, "{fraction:09}").expect("String writes cannot fail");
                if matches!(precision, Precision::Digit(_)) {
                    output.truncate(fraction_start + digits);
                } else {
                    while output.ends_with('0') {
                        output.pop();
                    }
                }
            }
            output.push('S');
        }
    }
    output
}
