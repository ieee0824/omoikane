//! Pure value sanitization for text, numeric, and calendar input states.

/// An owned edit plan over UTF-16 code-unit positions, without retaining input.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Normalization {
    Unchanged,
    Empty,
    /// Keep these bounds and discard CR/LF within them.
    Filter {
        start: usize,
        end: usize,
    },
    /// A rewritten UTF-16 value, for email lists or normalized local datetimes.
    Rewrite(Vec<u16>),
}

impl Normalization {
    pub(crate) fn produces_empty(&self, originally_empty: bool) -> bool {
        match self {
            Self::Unchanged => originally_empty,
            Self::Empty => true,
            Self::Filter { .. } => false,
            Self::Rewrite(units) => units.is_empty(),
        }
    }
}

/// Resolves an input type keyword using ASCII case folding and the text default.
/// This pure calculation does not read attributes or document state.
pub(crate) fn canonical_type(raw: &str) -> &'static str {
    const TYPES: &[&str] = &[
        "hidden",
        "text",
        "search",
        "tel",
        "url",
        "email",
        "password",
        "date",
        "month",
        "week",
        "time",
        "datetime-local",
        "number",
        "range",
        "color",
        "checkbox",
        "radio",
        "file",
        "submit",
        "image",
        "reset",
        "button",
    ];
    TYPES
        .iter()
        .copied()
        .find(|kind| raw.eq_ignore_ascii_case(kind))
        .unwrap_or("text")
}

/// Plans text, number, and calendar sanitization from explicit input state.
/// Remaining input types retain their existing value behavior. Unknown types
/// use text's newline removal. No DOM, JavaScript, or environment state is read.
pub(crate) fn normalize<I>(kind: &str, multiple: bool, units: I) -> Normalization
where
    I: Iterator<Item = u16> + Clone,
{
    let kind = canonical_type(kind);
    if kind == "number" {
        return if valid_number(units) {
            Normalization::Unchanged
        } else {
            Normalization::Empty
        };
    }
    if matches!(kind, "date" | "month" | "week" | "time" | "datetime-local") {
        let value: Result<String, _> = units
            .map(|unit| u8::try_from(unit).map(char::from))
            .collect();
        let Ok(value) = value else {
            return Normalization::Empty;
        };
        if !valid_calendar_value(kind, &value) {
            return Normalization::Empty;
        }
        return if kind == "datetime-local" {
            normalize_local_datetime(&value)
        } else {
            Normalization::Unchanged
        };
    }
    if matches!(
        kind,
        "hidden"
            | "range"
            | "color"
            | "checkbox"
            | "radio"
            | "file"
            | "submit"
            | "image"
            | "reset"
            | "button"
    ) {
        return Normalization::Unchanged;
    }
    if kind == "email" && multiple {
        return normalize_email_list(units);
    }
    normalize_text(units, matches!(kind, "url" | "email"))
}

fn ascii_whitespace(unit: u16) -> bool {
    matches!(unit, 9 | 10 | 12 | 13 | 32)
}

fn normalize_text(units: impl Iterator<Item = u16>, trim: bool) -> Normalization {
    let mut first = None;
    let mut end = 0;
    let mut total = 0;
    let mut removed_newline = false;
    for (index, unit) in units.enumerate() {
        total = index + 1;
        if matches!(unit, 10 | 13) {
            removed_newline = true;
            continue;
        }
        if trim && ascii_whitespace(unit) {
            continue;
        }
        first.get_or_insert(index);
        end = index + 1;
    }
    let Some(start) = first else {
        return Normalization::Empty;
    };
    if start == 0 && end == total && !removed_newline {
        Normalization::Unchanged
    } else {
        Normalization::Filter { start, end }
    }
}

fn email_list_needs_edit(units: impl Iterator<Item = u16>) -> bool {
    let mut leading = true;
    let mut trailing = false;
    for unit in units {
        if matches!(unit, 10 | 13) || (leading && ascii_whitespace(unit)) {
            return true;
        }
        if unit == 44 {
            if trailing {
                return true;
            }
            leading = true;
        } else {
            trailing = ascii_whitespace(unit);
            leading = false;
        }
    }
    trailing
}

fn normalize_email_list<I: Iterator<Item = u16> + Clone>(units: I) -> Normalization {
    if !email_list_needs_edit(units.clone()) {
        return Normalization::Unchanged;
    }
    let mut output = Vec::new();
    let mut segment_start = 0;
    let mut last_content = 0;
    for unit in units {
        if matches!(unit, 10 | 13) {
            continue;
        }
        if unit == 44 {
            output.truncate(last_content);
            output.push(unit);
            segment_start = output.len();
            last_content = output.len();
        } else if !ascii_whitespace(unit) || output.len() > segment_start {
            output.push(unit);
            if !ascii_whitespace(unit) {
                last_content = output.len();
            }
        }
    }
    output.truncate(last_content);
    if output.is_empty() {
        Normalization::Empty
    } else {
        Normalization::Rewrite(output)
    }
}

fn digits(bytes: &[u8], cursor: &mut usize) -> usize {
    let start = *cursor;
    while bytes.get(*cursor).is_some_and(u8::is_ascii_digit) {
        *cursor += 1;
    }
    *cursor - start
}

pub(super) fn valid_number(units: impl Iterator<Item = u16>) -> bool {
    let mut source = String::new();
    for unit in units {
        let Ok(byte) = u8::try_from(unit) else {
            return false;
        };
        if !byte.is_ascii_digit() && !matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E') {
            return false;
        }
        source.push(char::from(byte));
    }
    let bytes = source.as_bytes();
    let mut cursor = usize::from(bytes.first() == Some(&b'-'));
    let integer_digits = digits(bytes, &mut cursor);
    let mut fraction_digits = 0;
    if bytes.get(cursor) == Some(&b'.') {
        cursor += 1;
        fraction_digits = digits(bytes, &mut cursor);
        if fraction_digits == 0 {
            return false;
        }
    }
    if integer_digits + fraction_digits == 0 {
        return false;
    }
    if matches!(bytes.get(cursor), Some(b'e' | b'E')) {
        cursor += 1;
        if matches!(bytes.get(cursor), Some(b'+' | b'-')) {
            cursor += 1;
        }
        if digits(bytes, &mut cursor) == 0 {
            return false;
        }
    }
    cursor == bytes.len() && source.parse::<f64>().is_ok_and(f64::is_finite)
}

/// Uses the shortest time spelling and a T separator for local datetimes.
fn normalize_local_datetime(value: &str) -> Normalization {
    let (date, time) = value
        .split_once(['T', ' '])
        .expect("validated local datetime");
    let mut time = time;
    if time.contains('.') {
        time = time.trim_end_matches('0').trim_end_matches('.');
    }
    if time.ends_with(":00") && time.len() == 8 {
        time = &time[..5];
    }
    let normalized = format!("{date}T{time}");
    if normalized == value {
        Normalization::Unchanged
    } else {
        Normalization::Rewrite(normalized.encode_utf16().collect())
    }
}

/// Checks HTML calendar syntax without time zones, clocks, or a host date API.
/// Years are validated as decimal strings, so valid years need not fit an integer.
pub(super) fn valid_calendar_value(kind: &str, value: &str) -> bool {
    match kind {
        "date" => valid_date(value),
        "month" => month_parts(value).is_some(),
        "week" => {
            let Some((year, week)) = value.split_once("-W") else {
                return false;
            };
            let Some(year) = year_modulo_400(year) else {
                return false;
            };
            let Some(week) = two_digits(week) else {
                return false;
            };
            // Gregorian weekdays repeat every 400 years. 0001-01-01 is Monday.
            let previous = if year == 0 { 399 } else { year - 1 };
            let january_first = (previous * 365 + previous / 4 - previous / 100) % 7;
            let long = january_first == 3 || (january_first == 2 && leap_year(year));
            week >= 1 && week <= if long { 53 } else { 52 }
        }
        "time" => valid_time(value),
        "datetime-local" => value
            .split_once(['T', ' '])
            .is_some_and(|(date, time)| valid_date(date) && valid_time(time)),
        _ => false,
    }
}

fn year_modulo_400(year: &str) -> Option<u32> {
    if year.len() < 4
        || !year.bytes().all(|b| b.is_ascii_digit())
        || !year.bytes().any(|b| b != b'0')
    {
        return None;
    }
    Some(year.bytes().fold(0, |remainder, digit| {
        (remainder * 10 + u32::from(digit - b'0')) % 400
    }))
}

fn two_digits(value: &str) -> Option<u32> {
    (value.len() == 2 && value.bytes().all(|b| b.is_ascii_digit()))
        .then(|| u32::from(value.as_bytes()[0] - b'0') * 10 + u32::from(value.as_bytes()[1] - b'0'))
}

fn month_parts(value: &str) -> Option<(u32, u32)> {
    let (year, month) = value.split_once('-')?;
    let year = year_modulo_400(year)?;
    let month = two_digits(month)?;
    (1..=12).contains(&month).then_some((year, month))
}

fn leap_year(year: u32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn valid_date(value: &str) -> bool {
    let Some((month, day)) = value.rsplit_once('-') else {
        return false;
    };
    let Some((year, month)) = month_parts(month) else {
        return false;
    };
    let Some(day) = two_digits(day) else {
        return false;
    };
    let maximum = match month {
        2 => {
            if leap_year(year) {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=maximum).contains(&day)
}

fn valid_time(value: &str) -> bool {
    let mut parts = value.split(':');
    let Some(hour) = parts.next().and_then(two_digits) else {
        return false;
    };
    let Some(minute) = parts.next().and_then(two_digits) else {
        return false;
    };
    if hour > 23 || minute > 59 {
        return false;
    }
    let Some(seconds) = parts.next() else {
        return true;
    };
    if parts.next().is_some() {
        return false;
    }
    let (second, fraction) = seconds
        .split_once('.')
        .map_or((seconds, None), |(s, f)| (s, Some(f)));
    two_digits(second).is_some_and(|s| s <= 59)
        && fraction
            .is_none_or(|f| (1..=3).contains(&f.len()) && f.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_sanitization_rejects_invalid_fields_without_host_date_limits() {
        for (kind, valid, invalid) in [
            ("date", "2000-02-29", "1900-02-29"),
            ("date", "0001-01-01", "0000-01-01"),
            ("date", "10000000000000000000000-02-29", "2020-04-31"),
            ("month", "2020-12", "2020-13"),
            ("week", "2020-W53", "2021-W53"),
            ("week", "2004-W53", "2020-W00"),
            ("time", "23:59:59.999", "24:00"),
            ("time", "00:00", "12:00:00.0000"),
            ("datetime-local", "2020-02-29T12:00", "2019-02-29T12:00"),
        ] {
            assert_eq!(sanitized(kind, false, valid), valid, "{kind}");
            assert_eq!(sanitized(kind, false, invalid), "", "{kind}: {invalid}");
        }
    }

    #[test]
    fn local_datetime_normalizes_separator_and_redundant_seconds() {
        for (source, expected) in [
            ("2020-01-01 00:00:00.000", "2020-01-01T00:00"),
            ("2020-01-01T12:34:01.100", "2020-01-01T12:34:01.1"),
            ("2020-01-01T12:34:00.001", "2020-01-01T12:34:00.001"),
        ] {
            assert_eq!(sanitized("datetime-local", false, source), expected);
        }
    }

    fn sanitized(kind: &str, multiple: bool, value: &str) -> String {
        let plan = normalize(kind, multiple, value.encode_utf16());
        let units = match plan {
            Normalization::Unchanged => return value.to_string(),
            Normalization::Empty => return String::new(),
            Normalization::Rewrite(units) => units,
            Normalization::Filter { start, end } => value
                .encode_utf16()
                .enumerate()
                .filter_map(|(index, unit)| {
                    (index >= start && index < end && !matches!(unit, 10 | 13)).then_some(unit)
                })
                .collect(),
        };
        String::from_utf16(&units).unwrap()
    }

    #[test]
    fn text_sanitization_preserves_non_newline_units_and_uses_utf16_bounds() {
        assert_eq!(sanitized("text", false, " \tA\r\nB "), " \tAB ");
        assert_eq!(sanitized("unknown", false, "\r\n"), "");
        assert_eq!(sanitized("URL", false, "\t😀\nX \r"), "😀X");
        assert_eq!(sanitized("email", false, " a@b \r\n"), "a@b");
        assert_eq!(sanitized("text", false, "\u{b}"), "\u{b}");
        assert_eq!(sanitized("date", false, " a\n "), "");
        assert_eq!(
            normalize("text", false, [0xd800, 10].into_iter()),
            Normalization::Filter { start: 0, end: 1 }
        );
    }

    #[test]
    fn email_multiple_trims_each_address_and_keeps_empty_entries() {
        assert_eq!(sanitized("email", true, " a@b , c@d \r\n"), "a@b,c@d");
        assert_eq!(sanitized("email", true, " , \t, "), ",,");
        assert_eq!(sanitized("email", true, " a b , c d "), "a b,c d");
        assert_eq!(
            normalize("email", true, "a@b,c@d".encode_utf16()),
            Normalization::Unchanged
        );
    }

    #[test]
    fn numeric_sanitization_requires_strict_finite_html_numbers() {
        for valid in ["0", "-0", "01", ".5", "-.5", "1.5", "1e2", "1e+2", "1e-999"] {
            assert_eq!(sanitized("number", false, valid), valid);
        }
        for invalid in [
            "", "-", "+1", "1.", "1.e2", "1e", "1e+", "0x10", "NaN", "Infinity", "1e309", " 1",
            "1\n",
        ] {
            assert_eq!(sanitized("number", false, invalid), "", "{invalid:?}");
        }
    }
}
