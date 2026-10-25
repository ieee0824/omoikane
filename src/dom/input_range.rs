//! Pure numeric conversion and range bounds, independent of DOM and host clocks.

use super::input_value::{canonical_type, valid_calendar_value, valid_number};

mod decimal;

const DAY: f64 = 86_400_000.0;

/// A derived comparison value; the actual UTF-16 value remains in JavaScript.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NumericInputValue {
    kind: &'static str,
    number: f64,
}

impl NumericInputValue {
    pub(crate) fn parse(kind: &str, value: &str) -> Option<Self> {
        let kind = canonical_type(kind);
        let number = number(kind, value)?;
        Some(Self {
            kind: if kind == "range" { "number" } else { kind },
            number,
        })
    }

    pub(crate) fn for_type(self, kind: &str) -> Option<f64> {
        let kind = if kind == "range" { "number" } else { kind };
        (self.kind == kind).then_some(self.number)
    }
}

/// Converts valid input syntax to its HTML comparison scale, with no time zone.
pub(crate) fn number(kind: &str, value: &str) -> Option<f64> {
    if matches!(kind, "number" | "range") {
        return valid_number(value.encode_utf16())
            .then(|| value.parse().ok())
            .flatten();
    }
    if !valid_calendar_value(kind, value) {
        return None;
    }
    let result = match kind {
        "date" => date_days(value)? * DAY,
        "month" => {
            let (year, month) = value.split_once('-')?;
            (year.parse::<f64>().ok()? - 1970.0) * 12.0 + month.parse::<f64>().ok()? - 1.0
        }
        "week" => {
            let (year, week) = value.split_once("-W")?;
            let january_fourth = date_days(&format!("{year}-01-04"))?;
            let weekday = (january_fourth + 3.0).rem_euclid(7.0);
            (january_fourth - weekday + (week.parse::<f64>().ok()? - 1.0) * 7.0) * DAY
        }
        "time" => time_milliseconds(value)?,
        "datetime-local" => {
            let (date, time) = value.split_once(['T', ' '])?;
            date_days(date)? * DAY + time_milliseconds(time)?
        }
        _ => return None,
    };
    result.is_finite().then_some(result)
}

/// Converts an attribute using HTML's user-agent parsing rules. Numeric
/// attributes accept leading ASCII whitespace/signs and a numeric prefix;
/// sanitization of an input value still requires the entire valid syntax.
pub(crate) fn attribute_number(kind: &str, source: &str) -> Option<f64> {
    if !matches!(kind, "number" | "range") {
        return number(kind, source);
    }
    let source = source.trim_start_matches(['\t', '\n', '\u{c}', '\r', ' ']);
    let bytes = source.as_bytes();
    let mut cursor = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let start = cursor;
    while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
    }
    let mut digits = cursor - start;
    if bytes.get(cursor) == Some(&b'.') {
        cursor += 1;
        let fraction = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        digits += cursor - fraction;
    }
    if digits == 0 {
        return None;
    }
    let mut end = cursor;
    if matches!(bytes.get(cursor), Some(b'e' | b'E')) {
        cursor += 1;
        if matches!(bytes.get(cursor), Some(b'+' | b'-')) {
            cursor += 1;
        }
        let exponent = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if cursor != exponent {
            end = cursor;
        }
    }
    source[..end]
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

// Gregorian civil date to days since 1970-01-01, including years 0001..0099.
fn date_days(value: &str) -> Option<f64> {
    let mut parts = value.split('-');
    let mut year = parts.next()?.parse::<f64>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    if month <= 2 {
        year -= 1.0;
    }
    let era = (year / 400.0).floor();
    let yoe = year - era * 400.0;
    let month = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    Some(
        era * 146097.0 + yoe * 365.0 + (yoe / 4.0).floor() - (yoe / 100.0).floor() + f64::from(doy)
            - 719468.0,
    )
}

fn time_milliseconds(value: &str) -> Option<f64> {
    let mut parts = value.split(':');
    let hour = parts.next()?.parse::<u32>().ok()?;
    let minute = parts.next()?.parse::<u32>().ok()?;
    let seconds = parts.next().unwrap_or("0");
    let (second, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let second = second.parse::<u32>().ok()?;
    let fraction = if fraction.is_empty() {
        0
    } else {
        let padding = 3_u32.checked_sub(u32::try_from(fraction.len()).ok()?)?;
        fraction.parse::<u32>().ok()? * 10_u32.pow(padding)
    };
    // Validated time components fit within one day. Integral milliseconds
    // are exactly representable as binary64, including subsecond boundaries.
    Some(f64::from(
        ((hour * 60 + minute) * 60 + second) * 1000 + fraction,
    ))
}

/// Returns underflow/overflow, including both flags for a periodic range gap.
pub(crate) fn violations(
    kind: &str,
    value: Option<f64>,
    min: Option<f64>,
    max: Option<f64>,
) -> (bool, bool) {
    let Some(value) = value else {
        return (false, false);
    };
    if kind == "time" && min.zip(max).is_some_and(|(min, max)| min > max) {
        let gap = value > max.unwrap() && value < min.unwrap();
        return (gap, gap);
    }
    (
        min.is_some_and(|min| value < min),
        max.is_some_and(|max| value > max),
    )
}

/// Range value sanitization from explicit attributes. The caller formats the
/// returned finite number using ECMAScript's number-to-string operation.
pub(crate) fn sanitize_range(value: &str, min: &str, max: &str, step: &str, default: &str) -> f64 {
    let minimum = attribute_number("number", min).unwrap_or(0.0);
    let maximum = attribute_number("number", max).unwrap_or(100.0);
    let midpoint = if maximum < minimum {
        minimum
    } else {
        minimum / 2.0 + maximum / 2.0
    };
    let mut value = number("number", value).unwrap_or(midpoint).max(minimum);
    if maximum >= minimum {
        value = value.min(maximum);
    }
    if step.eq_ignore_ascii_case("any") {
        return value;
    }
    let step = attribute_number("number", step)
        .filter(|step| *step > 0.0)
        .unwrap_or(1.0);
    let base = attribute_number("number", min)
        .or_else(|| attribute_number("number", default))
        .unwrap_or(0.0);
    decimal::snap(value, minimum, maximum, step, base)
}

/// Computes step mismatch without an epsilon or an overflowing scaled step.
pub(crate) fn step_mismatch(value: f64, base: f64, step: f64, scale: u64) -> bool {
    value.is_finite()
        && base.is_finite()
        && step.is_finite()
        && step > 0.0
        && scale > 0
        && !decimal::on_grid(value, base, step, scale)
}

/// Owned numeric step configuration in the current type's comparison scale.
pub(crate) struct StepRange {
    pub(crate) minimum: Option<f64>,
    pub(crate) maximum: Option<f64>,
    pub(crate) step: f64,
    pub(crate) base: f64,
}

impl StepRange {
    /// Returns no change for an empty permitted grid or a blocked direction.
    pub(crate) fn advance(&self, value: f64, count: i32, up: bool) -> Option<f64> {
        if !self.step.is_finite() || self.step <= 0.0 || !self.base.is_finite() {
            return None;
        }
        decimal::advance(
            value,
            self.minimum.unwrap_or(-f64::MAX),
            self.maximum.unwrap_or(f64::MAX),
            self.step,
            self.base,
            count,
            up,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_numbers_use_html_scales_and_small_years() {
        for (kind, value, expected) in [
            ("date", "1970-01-01", 0.0),
            ("date", "0001-01-01", -62135596800000.0),
            ("date", "0099-01-01", -59042995200000.0),
            ("month", "1969-12", -1.0),
            ("week", "1970-W01", -259200000.0),
            ("week", "2020-W53", 1609113600000.0),
            ("time", "01:02:03.004", 3723004.0),
            ("time", "00:00:01.001", 1001.0),
            ("datetime-local", "1970-01-01T00:00:01.001", 1001.0),
            ("datetime-local", "1970-01-01T01:00", 3600000.0),
        ] {
            assert_eq!(number(kind, value), Some(expected), "{kind}:{value}");
        }
        assert_eq!(number("date", "1900-02-29"), None);
        assert_eq!(number("number", "+5"), None);
        assert_eq!(number("number", "5e1"), Some(50.0));
    }

    #[test]
    fn numeric_attributes_use_user_agent_parsing_without_relaxing_values() {
        for (source, expected) in [
            (" +5", 5.0),
            ("5x", 5.0),
            ("1.", 1.0),
            ("5e+", 5.0),
            ("5e1!", 50.0),
        ] {
            assert_eq!(attribute_number("number", source), Some(expected));
            assert_eq!(number("number", source), None);
        }
        assert_eq!(attribute_number("number", ".x"), None);
        assert_eq!(attribute_number("number", "1e9999"), None);
    }

    #[test]
    fn mismatch_uses_the_exact_grid_even_when_scaled_step_exceeds_binary64() {
        assert!(!step_mismatch(0.3, 0.0, 0.1, 1));
        assert!(step_mismatch(0.30000000000000004, 0.0, 0.1, 1));
        assert!(!step_mismatch(600.0, 0.0, 0.1, 1000));
        assert!(step_mismatch(DAY, 0.0, f64::MAX, DAY as u64));
        assert!(!step_mismatch(0.0, 0.0, f64::MAX, DAY as u64));
        assert!(!step_mismatch(f64::NAN, 0.0, 1.0, 1));
        assert!(!step_mismatch(1.0, 0.0, 0.0, 1));
    }

    #[test]
    fn stepping_uses_exact_decimal_grid_and_preserves_direction_at_bounds() {
        let range = StepRange {
            minimum: Some(0.0),
            maximum: Some(1.0),
            step: 0.1,
            base: 0.0,
        };
        assert_eq!(range.advance(0.2, 1, true), Some(0.3));
        assert_eq!(range.advance(0.3, -1, true), Some(0.2));
        assert_eq!(range.advance(0.95, 1, true), Some(1.0));
        assert_eq!(range.advance(2.0, 1, true), None);
        let empty = StepRange {
            minimum: Some(0.1),
            maximum: Some(0.2),
            step: 1.0,
            base: 0.0,
        };
        assert_eq!(empty.advance(0.0, 1, true), None);
        assert_eq!(sanitize_range("0.35", "0", "1", "0.1", ""), 0.4);
        assert_eq!(sanitize_range("-0.35", "-1", "1", "0.1", ""), -0.3);
        assert_eq!(sanitize_range("0.6", "0", "1", "0.1", ""), 0.6);
    }

    #[test]
    fn periodic_bounds_and_range_sanitization() {
        assert_eq!(
            violations("time", Some(12.0), Some(21.0), Some(3.0)),
            (true, true)
        );
        assert_eq!(
            violations("time", Some(22.0), Some(21.0), Some(3.0)),
            (false, false)
        );
        assert_eq!(sanitize_range("", "", "", "", ""), 50.0);
        assert_eq!(sanitize_range("99", "0", "10", "", ""), 10.0);
        assert_eq!(sanitize_range("5.5", "0", "10", "", ""), 6.0);
        assert_eq!(sanitize_range("5.5", "0", "10", "any", ""), 5.5);
        assert_eq!(sanitize_range("", "7", "2", "any", ""), 7.0);
    }
}
