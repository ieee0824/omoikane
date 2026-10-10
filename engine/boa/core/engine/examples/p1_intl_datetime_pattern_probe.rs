//! Checks owned ICU pattern width adaptation without changing locale literals.
use icu_calendar::{Date, Gregorian};
use icu_datetime::{
    DateTimeFormatter,
    fieldsets::YMD,
    options::YearStyle,
    pattern::{DateTimePattern, FixedCalendarDateTimeNames},
    provider::fields::FieldLength,
};
use icu_locale::Locale;
use writeable::TryWriteable;

#[path = "../src/builtins/intl/date_time_format/pattern.rs"]
mod pattern;

fn numeric_widths(pattern: DateTimePattern, width: FieldLength) -> DateTimePattern {
    pattern::ComponentWidths {
        year: Some(FieldLength::One),
        month: Some(width),
        day: Some(width),
        ..Default::default()
    }
    .apply(pattern)
}

fn main() {
    let mixed = DateTimePattern::try_from_pattern_str("yyyy/MM/dd 'literal 00' r U D A HH:mm:ss")
        .expect("valid mixed pattern");
    let adjusted = pattern::ComponentWidths {
        year: Some(FieldLength::One),
        month: Some(FieldLength::One),
        day: Some(FieldLength::One),
        hour: Some(FieldLength::One),
        minute: Some(FieldLength::One),
        second: Some(FieldLength::One),
        ..Default::default()
    }
    .apply(mixed);
    let expected = DateTimePattern::try_from_pattern_str("y/M/d 'literal 00' r U D A HH:mm:ss")
        .expect("valid expected pattern");
    let adjusted: icu_datetime::provider::pattern::runtime::Pattern<'static> = adjusted.into();
    let expected: icu_datetime::provider::pattern::runtime::Pattern<'static> = expected.into();
    assert_eq!(
        adjusted, expected,
        "literal and non-component symbols are preserved"
    );
    println!("mixed numeric fields: literals and non-component symbols preserved");
    let provider = boa_icu_provider::buffer();
    let date = Date::try_new_gregorian(1970, 1, 1).expect("valid reference date");
    for (name, numeric, padded) in [
        ("en-US", "1/1/1970", "01/01/1970"),
        ("ja-JP", "1970/1/1", "1970/01/01"),
        ("de-DE", "1.1.1970", "01.01.1970"),
        (
            "ar-EG",
            "١\u{200f}/١\u{200f}/١٩٧٠",
            "٠١\u{200f}/٠١\u{200f}/١٩٧٠",
        ),
    ] {
        let locale: Locale = name.parse().expect("valid reference locale");
        let formatter = DateTimeFormatter::try_new_with_buffer_provider(
            &provider,
            locale.clone().into(),
            YMD::short().with_year_style(YearStyle::Full),
        )
        .expect("bundled date patterns");
        let names = FixedCalendarDateTimeNames::<Gregorian, YMD>::try_new_with_buffer_provider(
            &provider,
            locale.into(),
        )
        .expect("bundled numeric formatting data");
        for (width, expected) in [(FieldLength::One, numeric), (FieldLength::Two, padded)] {
            let pattern = numeric_widths(formatter.format(&date).pattern(), width);
            let formatted = names.with_pattern_unchecked(&pattern).format(&date);
            let actual = formatted
                .try_write_to_string()
                .expect("numeric pattern needs no names");
            assert_eq!(actual, expected, "locale {name}, width {width:?}");
            println!("{name} {width:?}: {actual}");
        }
    }
}
