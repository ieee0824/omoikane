//! Checks semantic time skeletons across lengths against the bundled data.
use icu_calendar::Date;
use icu_datetime::{
    DateTimeFormatter,
    fieldsets::builder::FieldSetBuilder,
    input::{DateTime, Time},
    options::{Length, TimePrecision},
};
use icu_locale::Locale;

fn main() {
    let provider = boa_icu_provider::buffer();
    let input = DateTime {
        date: Date::try_new_iso(1970, 1, 1).unwrap(),
        time: Time::start_of_day(),
    };
    for locale in ["en-US", "en", "ja-JP", "de-DE", "ar-EG"] {
        let locale: Locale = locale.parse().unwrap();
        for length in [Length::Short, Length::Medium, Length::Long] {
            let mut builder = FieldSetBuilder::new();
            builder.length = Some(length);
            builder.time_precision = Some(TimePrecision::Second);
            let formatter = DateTimeFormatter::try_new_with_buffer_provider(
                &provider,
                locale.clone().into(),
                builder.build_composite_datetime().unwrap(),
            )
            .unwrap();
            let formatted = formatter.format(&input);
            println!(
                "{locale} {length:?} pattern={} result={}",
                formatted.pattern(),
                formatted.to_string()
            );
        }
    }
}
