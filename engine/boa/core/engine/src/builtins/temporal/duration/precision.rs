//! Exact arithmetic at the boundary between Temporal integers and JS Numbers.

use temporal_rs::{Duration, Sign, options::Unit};

use crate::JsResult;

/// CreateTemporalDuration stores Number-valued slots, even when the native
/// arithmetic returned an integer that binary64 cannot represent exactly.
pub(super) fn number_slots(duration: Duration) -> JsResult<Duration> {
    Duration::new(
        duration.years(),
        duration.months(),
        duration.weeks(),
        (duration.days() as f64) as i64,
        (duration.hours() as f64) as i64,
        (duration.minutes() as f64) as i64,
        (duration.seconds() as f64) as i64,
        (duration.milliseconds() as f64) as i64,
        (duration.microseconds() as f64) as i128,
        (duration.nanoseconds() as f64) as i128,
    )
    .map_err(Into::into)
}

/// Valid durations have less than 2^53 seconds of time, so all products fit
/// i128. No intermediate calculation rounds through binary64.
pub(super) fn time_nanoseconds(duration: &Duration) -> i128 {
    (((i128::from(duration.hours()) * 60 + i128::from(duration.minutes())) * 60
        + i128::from(duration.seconds()))
        * 1000
        + i128::from(duration.milliseconds()))
        * 1_000_000
        + duration.microseconds() * 1000
        + duration.nanoseconds()
}

pub(super) fn unit_nanoseconds(unit: Unit) -> Option<i128> {
    Some(match unit {
        Unit::Week => 604_800_000_000_000,
        Unit::Day => 86_400_000_000_000,
        Unit::Hour => 3_600_000_000_000,
        Unit::Minute => 60_000_000_000,
        Unit::Second => 1_000_000_000,
        Unit::Millisecond => 1_000_000,
        Unit::Microsecond => 1000,
        Unit::Nanosecond => 1,
        _ => return None,
    })
}

pub(super) fn largest_unit(duration: &Duration) -> Unit {
    [
        (Unit::Year, i128::from(duration.years())),
        (Unit::Month, i128::from(duration.months())),
        (Unit::Week, i128::from(duration.weeks())),
        (Unit::Day, i128::from(duration.days())),
        (Unit::Hour, i128::from(duration.hours())),
        (Unit::Minute, i128::from(duration.minutes())),
        (Unit::Second, i128::from(duration.seconds())),
        (Unit::Millisecond, i128::from(duration.milliseconds())),
        (Unit::Microsecond, duration.microseconds()),
        (Unit::Nanosecond, duration.nanoseconds()),
    ]
    .into_iter()
    .find_map(|(unit, value)| (value != 0).then_some(unit))
    .unwrap_or(Unit::Nanosecond)
}

pub(super) fn duration_sign(duration: &Duration) -> i8 {
    match duration.sign() {
        Sign::Negative => -1,
        Sign::Zero => 0,
        Sign::Positive => 1,
    }
}

/// Round a rational once to binary64, using a 53-bit significand plus the
/// exact remainder. Casting numerator and denominator separately can double
/// round and loses a fraction adjacent to Number.MAX_SAFE_INTEGER.
pub(super) fn rational_to_number(numerator: i128, denominator: i128) -> f64 {
    debug_assert!(denominator > 0);
    if numerator == 0 {
        return 0.0;
    }
    let negative = numerator < 0;
    let mut numerator = numerator.unsigned_abs();
    let mut denominator = denominator as u128;
    let mut exponent = denominator.leading_zeros() as i32 - numerator.leading_zeros() as i32;
    if exponent >= 0 {
        if numerator >> exponent < denominator {
            exponent -= 1;
        }
    } else if numerator << -exponent < denominator {
        exponent -= 1;
    }
    if exponent >= 0 {
        denominator <<= exponent;
    } else {
        numerator <<= -exponent;
    }
    let mut remainder = numerator - denominator;
    let mut significand = 1_u64;
    for _ in 0..52 {
        significand = (significand << 1) | next_bit(&mut remainder, denominator);
    }
    let guard = next_bit(&mut remainder, denominator);
    if guard != 0 && (remainder != 0 || significand & 1 != 0) {
        significand += 1;
        if significand == 1 << 53 {
            significand >>= 1;
            exponent += 1;
        }
    }
    // Temporal ratios are between roughly 10^-25 and 10^25; subnormals and
    // infinity cannot occur within the validated duration/date limits.
    let sign = u64::from(negative) << 63;
    let biased_exponent = ((exponent + 1023) as u64) << 52;
    f64::from_bits(sign | biased_exponent | (significand & ((1 << 52) - 1)))
}

fn next_bit(remainder: &mut u128, denominator: u128) -> u64 {
    let complement = denominator - *remainder;
    if *remainder >= complement {
        *remainder -= complement;
        1
    } else {
        *remainder *= 2;
        0
    }
}

#[cfg(test)]
mod tests {
    use super::rational_to_number;

    #[test]
    fn exact_ratio_rounds_once_and_ties_to_even() {
        assert_eq!(
            rational_to_number(2_939_649_187_497_660, 3_600_000_000_000),
            816.56921874935
        );
        assert_eq!(
            rational_to_number(9_007_199_254_740_993_999, 1000),
            9_007_199_254_740_994.0
        );
        assert_eq!(
            rational_to_number(9_007_199_254_740_993, 1),
            9_007_199_254_740_992.0
        );
        assert_eq!(
            rational_to_number(9_007_199_254_740_995, 1),
            9_007_199_254_740_996.0
        );
        assert_eq!(rational_to_number(42, 31), 1.3548387096774193);
        assert_eq!(rational_to_number(-42, 31), -1.3548387096774193);
        assert_eq!(rational_to_number(1, 1_000_000_000), 0.000000001);
    }
}
