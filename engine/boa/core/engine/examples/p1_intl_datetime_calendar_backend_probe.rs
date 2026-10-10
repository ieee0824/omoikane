//! Compares native Intl calendar formatting with the bundled ICU reference.
use boa_engine::{Context, Source};
use icu_calendar::Date;
use icu_datetime::{
    DateTimeFormatter,
    fieldsets::{
        YMD,
        enums::{CompositeDateTimeFieldSet, DateFieldSet},
    },
    input::{DateTime, Time},
    options::YearStyle,
};
use icu_locale::Locale;
use writeable::Writeable;

fn main() {
    let provider = boa_icu_provider::buffer();
    let mut context = Context::default();
    let mut missing = Vec::new();
    for locale in [
        "en-US",
        "ja-JP",
        "de-DE",
        "ar-EG",
        "th-TH-u-ca-buddhist",
        "en-US-u-ca-chinese",
        "en-US-u-ca-coptic",
        "ko-KR-u-ca-dangi",
        "en-US-u-ca-ethiopic",
        "en-US-u-ca-hebrew",
        "en-US-u-ca-indian",
        "en-US-u-ca-islamic-civil",
        "en-US-u-ca-islamic",
        "ar-SA-u-ca-islamic-umalqura",
        "ja-JP-u-ca-japanese",
        "fa-IR-u-ca-persian",
        "zh-TW-u-ca-roc",
    ] {
        let locale: Locale = locale.parse().expect("reference locale");
        let prefs = locale.clone().into();
        let fields = CompositeDateTimeFieldSet::Date(DateFieldSet::YMD(
            YMD::long().with_year_style(YearStyle::Full),
        ));
        let original =
            match DateTimeFormatter::try_new_with_buffer_provider(&provider, prefs, fields) {
                Ok(formatter) => formatter,
                Err(error) => {
                    println!("{locale}: missing bundled data: {error:?}");
                    missing.push(locale.to_string());
                    continue;
                }
            };
        for year in [1970, 2025] {
            let datetime = DateTime {
                date: Date::try_new_iso(year, 1, 1).expect("reference date"),
                time: Time::start_of_day(),
            };
            let expected = original.format(&datetime).write_to_string().into_owned();
            let source = format!(
                "new Intl.DateTimeFormat('{locale}', {{dateStyle:'long',timeZone:'UTC'}}).format(Date.UTC({year},0,1))"
            );
            let actual = context
                .eval(Source::from_bytes(&source))
                .expect("native calendar formatting")
                .as_string()
                .expect("formatter returns a string")
                .to_std_string()
                .expect("formatted date is valid UTF-16");
            assert_eq!(actual, expected, "calendar {locale}, ISO year {year}");
            println!("{locale} {year}: {actual}");
        }
    }
    assert!(missing.is_empty(), "missing calendar data for {missing:?}");
}
