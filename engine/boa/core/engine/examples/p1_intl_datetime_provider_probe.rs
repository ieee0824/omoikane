//! Measures the bundled provider's date formatting before the native Intl integration.
use icu_calendar::Date;
use icu_datetime::{DateTimeFormatter, fieldsets::YMD, options::YearStyle};
use icu_locale::Locale;
use writeable::Writeable;

fn main() {
    let provider = boa_icu_provider::buffer();
    let date = Date::try_new_iso(1970, 1, 1).expect("valid reference date");
    for name in ["en-US", "ja-JP", "de-DE", "ar-EG"] {
        let locale: Locale = name.parse().expect("valid reference locale");
        let result = DateTimeFormatter::try_new_with_buffer_provider(
            &provider,
            locale.into(),
            YMD::short().with_year_style(YearStyle::Full),
        );
        match result {
            Ok(formatter) => println!("{name}: {}", formatter.format(&date).write_to_string()),
            Err(error) => println!("{name}: provider error: {error:?}"),
        }
    }
}
