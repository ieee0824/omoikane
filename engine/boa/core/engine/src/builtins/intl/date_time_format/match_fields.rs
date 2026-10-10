//! Typed ICU field metadata; unsupported CLDR fields remain distinguishable.
use super::matcher::{MatchComponents, Width, ZoneName};
use icu_datetime::{
    pattern::DateTimePattern,
    provider::{
        fields::{
            Day, DayPeriod, Field, FieldLength, FieldSymbol, Second, TimeZone, Weekday, Year,
        },
        pattern::{PatternItem, runtime::Pattern},
    },
};

/// Constructor-local owned pattern and its ECMA DateTime Format Record.
#[derive(Debug)]
pub(super) struct PatternProfile {
    pub(super) pattern: Option<Pattern<'static>>,
    pub(super) fields: MatchComponents,
    pub(super) twelve_hour: Option<bool>,
}

/// A valid CLDR format can describe fields outside ECMA's eleven components.
/// Invalid or unknown patterns instead propagate an error to initialization.
#[derive(Debug)]
pub(super) enum FieldError {
    NonEcmaRecord,
    Invalid(String),
}

pub(super) fn numeric(length: FieldLength) -> Width {
    if length == FieldLength::Two {
        Width::TwoDigit
    } else {
        Width::Numeric
    }
}

fn text(length: FieldLength) -> Width {
    match length {
        FieldLength::Four => Width::Long,
        FieldLength::Five => Width::Narrow,
        _ => Width::Short,
    }
}

fn add_field(fields: &mut MatchComponents, field: Field) -> Result<(), FieldError> {
    let numeric = numeric(field.length);
    let text = text(field.length);
    match field.symbol {
        FieldSymbol::Era => fields.era = Some(text),
        FieldSymbol::Year(Year::Calendar | Year::RelatedIso) => fields.year = Some(numeric),
        FieldSymbol::Year(Year::Cyclic) => {
            fields.year.get_or_insert(Width::Numeric);
        }
        FieldSymbol::Month(_) => {
            fields.month = Some(
                if matches!(field.length, FieldLength::One | FieldLength::Two) {
                    numeric
                } else {
                    text
                },
            )
        }
        FieldSymbol::Day(Day::DayOfMonth) => fields.day = Some(numeric),
        FieldSymbol::Weekday(Weekday::Local | Weekday::StandAlone)
            if matches!(field.length, FieldLength::One | FieldLength::Two) =>
        {
            return Err(FieldError::NonEcmaRecord);
        }
        FieldSymbol::Weekday(_) => fields.weekday = Some(text),
        // AM/PM belongs to pattern12 and is not an explicit dayPeriod option.
        FieldSymbol::DayPeriod(DayPeriod::AmPm) => {}
        FieldSymbol::DayPeriod(DayPeriod::NoonMidnight) => fields.day_period = Some(text),
        FieldSymbol::Hour(_) => fields.hour = Some(numeric),
        FieldSymbol::Minute => fields.minute = Some(numeric),
        FieldSymbol::Second(Second::Second) => fields.second = Some(numeric),
        FieldSymbol::DecimalSecond(digits) => {
            fields.second = Some(numeric);
            let digits = digits as u8;
            if digits > 3 {
                return Err(FieldError::NonEcmaRecord);
            }
            fields.fractional_digits = Some(digits);
        }
        FieldSymbol::TimeZone(zone) => {
            fields.zone_name = Some(match (zone, field.length) {
                (
                    TimeZone::SpecificNonLocation,
                    FieldLength::One | FieldLength::Two | FieldLength::Three,
                ) => ZoneName::Short,
                (TimeZone::SpecificNonLocation, FieldLength::Four) => ZoneName::Long,
                (TimeZone::LocalizedOffset, FieldLength::One) => ZoneName::ShortOffset,
                (TimeZone::LocalizedOffset, FieldLength::Four) => ZoneName::LongOffset,
                (TimeZone::GenericNonLocation, FieldLength::One) => ZoneName::ShortGeneric,
                (TimeZone::GenericNonLocation, FieldLength::Four) => ZoneName::LongGeneric,
                _ => return Err(FieldError::NonEcmaRecord),
            })
        }
        _ => return Err(FieldError::NonEcmaRecord),
    }
    Ok(())
}

pub(super) fn components_from_pattern(
    pattern: &Pattern<'_>,
) -> Result<MatchComponents, FieldError> {
    let mut fields = MatchComponents::default();
    for item in pattern.items.iter() {
        if let PatternItem::Field(field) = item {
            add_field(&mut fields, field)?;
        }
    }
    Ok(fields)
}

/// Quote-aware runs are needed only to recognize CLDR fields ICU cannot parse.
/// The quoted text is never rewritten or used to infer formatted values.
fn raw_fields(raw: &str) -> Result<Vec<(char, usize)>, FieldError> {
    let mut runs = Vec::new();
    let mut chars = raw.chars().peekable();
    let mut quoted = false;
    while let Some(ch) = chars.next() {
        if ch == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
            } else {
                quoted = !quoted;
            }
        } else if !quoted && ch.is_ascii_alphabetic() {
            let mut count = 1;
            while chars.peek() == Some(&ch) {
                chars.next();
                count += 1;
            }
            runs.push((ch, count));
        }
    }
    if quoted {
        return Err(FieldError::Invalid("unterminated CLDR literal".into()));
    }
    Ok(runs)
}

fn hour_family(runs: &[(char, usize)]) -> Result<Option<bool>, FieldError> {
    let mut result = None;
    for (ch, _) in runs {
        let value = match ch {
            'h' | 'K' => true,
            'H' | 'k' => false,
            _ => continue,
        };
        if result.is_some_and(|previous| previous != value) {
            return Err(FieldError::Invalid(
                "mixed hour-cycle families in one CLDR pattern".into(),
            ));
        }
        result = Some(value);
    }
    Ok(result)
}

pub(super) fn parse_profile(raw: &str) -> Result<PatternProfile, FieldError> {
    let runs = raw_fields(raw)?;
    if let Some((symbol, _)) = runs.iter().find(|(symbol, _)| {
        !matches!(
            symbol,
            'G' | 'y'
                | 'Y'
                | 'u'
                | 'U'
                | 'r'
                | 'Q'
                | 'q'
                | 'M'
                | 'L'
                | 'w'
                | 'W'
                | 'd'
                | 'D'
                | 'F'
                | 'g'
                | 'E'
                | 'e'
                | 'c'
                | 'a'
                | 'b'
                | 'B'
                | 'h'
                | 'H'
                | 'K'
                | 'k'
                | 'm'
                | 's'
                | 'S'
                | 'A'
                | 'z'
                | 'Z'
                | 'O'
                | 'v'
                | 'V'
                | 'x'
                | 'X'
        )
    }) {
        return Err(FieldError::Invalid(format!("unknown CLDR field {symbol}")));
    }
    // Quarter/week/ordinal/ISO-zone formats are not ECMA format records. These
    // valid raw formats remain in the provider; no locale list is hardcoded.
    if runs.iter().any(|(ch, _)| {
        matches!(
            ch,
            'Q' | 'q' | 'Y' | 'u' | 'w' | 'W' | 'D' | 'F' | 'g' | 'A' | 'V' | 'x' | 'X'
        )
    }) {
        return Err(FieldError::NonEcmaRecord);
    }
    let twelve_hour = hour_family(&runs)?;
    if runs.iter().any(|(ch, _)| *ch == 'B') {
        return flexible_profile(raw, &runs, twelve_hour);
    }
    let pattern: Pattern<'static> = DateTimePattern::try_from_pattern_str(raw)
        .map_err(|error| FieldError::Invalid(format!("{error:?}")))?
        .into();
    let fields = components_from_pattern(&pattern)?;
    Ok(PatternProfile {
        pattern: Some(pattern),
        fields,
        twelve_hour,
    })
}

fn flexible_profile(
    raw: &str,
    runs: &[(char, usize)],
    twelve_hour: Option<bool>,
) -> Result<PatternProfile, FieldError> {
    let mut fields = MatchComponents::default();
    for &(ch, count) in runs {
        if ch == 'B' {
            fields.day_period = Some(match count {
                1..=3 => Width::Short,
                4 => Width::Long,
                5 => Width::Narrow,
                _ => {
                    return Err(FieldError::Invalid(
                        "invalid flexible day-period width".into(),
                    ));
                }
            });
        } else {
            let pattern: Pattern<'static> =
                DateTimePattern::try_from_pattern_str(&ch.to_string().repeat(count))
                    .map_err(|error| FieldError::Invalid(format!("{error:?}")))?
                    .into();
            for item in pattern.items.iter() {
                if let PatternItem::Field(field) = item {
                    add_field(&mut fields, field)?;
                }
            }
        }
    }
    // ICU4X 2.0 does not expose the flexible `B` writer yet. Its native `b`
    // writer preserves the requested field, width, typed part and the
    // locale-specific noon/midnight distinction while falling back to AM/PM
    // for the remaining periods. ECMA-402 permits an implementation-dependent
    // best-fit record, so retain the original `B` metadata for matching and use
    // this native fallback only for rendering.
    let mut native = String::with_capacity(raw.len());
    let mut quoted = false;
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\'' {
            native.push(ch);
            if chars.peek() == Some(&'\'') {
                native.push(chars.next().expect("peeked apostrophe"));
            } else {
                quoted = !quoted;
            }
        } else if !quoted && ch == 'B' {
            native.push('b');
        } else {
            native.push(ch);
        }
    }
    let pattern: Pattern<'static> = DateTimePattern::try_from_pattern_str(&native)
        .map_err(|error| FieldError::Invalid(format!("{error:?}")))?
        .into();
    Ok(PatternProfile {
        pattern: Some(pattern),
        fields,
        twelve_hour,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_metadata_keeps_calendar_years_standalone_months_and_literals() {
        let profile = parse_profile("yy 'MMMM' LLLLL EEEEEE r U").unwrap();
        assert_eq!(profile.fields.month, Some(Width::Narrow));
        assert_eq!(profile.fields.weekday, Some(Width::Short));
        assert_eq!(profile.fields.year, Some(Width::Numeric));
        assert_eq!(
            parse_profile("yy U").unwrap().fields.year,
            Some(Width::TwoDigit)
        );
        assert_eq!(
            parse_profile("'y' '' yyyy").unwrap().fields.year,
            Some(Width::Numeric)
        );
        assert!(matches!(
            parse_profile("cc"),
            Err(FieldError::NonEcmaRecord)
        ));
        assert!(matches!(
            parse_profile("QQQ y"),
            Err(FieldError::NonEcmaRecord)
        ));
    }

    #[test]
    fn implicit_am_pm_is_distinct_from_flexible_period_and_fraction() {
        let clock = parse_profile("h:mm a").unwrap();
        assert_eq!(clock.fields.day_period, None);
        assert_eq!(clock.twelve_hour, Some(true));
        let flexible = parse_profile("h:mm BBBB").unwrap();
        assert_eq!(flexible.fields.day_period, Some(Width::Long));
        assert_eq!(
            components_from_pattern(flexible.pattern.as_ref().unwrap())
                .unwrap()
                .day_period,
            Some(Width::Long)
        );
        assert_eq!(parse_profile("'B' HH:mm").unwrap().fields.day_period, None);
        let decimal = parse_profile("mm:ss.SSS").unwrap();
        assert_eq!(decimal.fields.second, Some(Width::TwoDigit));
        assert_eq!(decimal.fields.fractional_digits, Some(3));
    }

    #[test]
    fn zone_metadata_preserves_all_six_semantics() {
        for (pattern, zone) in [
            ("z", ZoneName::Short),
            ("zzzz", ZoneName::Long),
            ("O", ZoneName::ShortOffset),
            ("OOOO", ZoneName::LongOffset),
            ("v", ZoneName::ShortGeneric),
            ("vvvv", ZoneName::LongGeneric),
        ] {
            assert_eq!(parse_profile(pattern).unwrap().fields.zone_name, Some(zone));
        }
        assert!(matches!(
            parse_profile("XXX"),
            Err(FieldError::NonEcmaRecord)
        ));
        assert!(matches!(
            parse_profile("'unfinished"),
            Err(FieldError::Invalid(_))
        ));
        assert!(matches!(
            parse_profile("QQQ T"),
            Err(FieldError::Invalid(_))
        ));
    }
}
