//! Pure value sanitization for placeholder-capable input states.

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
    /// Email lists also trim each comma-delimited address.
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

/// Plans text/URL/email/number sanitization from explicit type and multiple state.
/// Other known input types retain their existing value behavior. Unknown types
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
    if matches!(
        kind,
        "hidden"
            | "date"
            | "month"
            | "week"
            | "time"
            | "datetime-local"
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

fn valid_number(units: impl Iterator<Item = u16>) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(sanitized("date", false, " a\n "), " a\n ");
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
