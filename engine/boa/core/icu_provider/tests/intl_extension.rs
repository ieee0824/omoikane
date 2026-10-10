use boa_intl_data::{
    CANONICAL_COMPOUND_UNIT_IDENTIFIERS, CurrencyPatternField, CurrencyTextField,
    DurationDigitalField, NumberPatternField, NumberSymbol, OmoikaneCalendarDatePatternsV1,
    OmoikaneCalendarIntervalPatternsV1, OmoikaneCompactPatternsV1, OmoikaneCompoundUnitPatternsV1,
    OmoikaneCurrencyDigitsV1, OmoikaneCurrencyPatternsV1, OmoikaneCurrencyTextV1,
    OmoikaneDurationDigitalV1, OmoikaneGlobalCurrencyPatternsV1, OmoikaneGlobalNumberPatternsV1,
    OmoikaneGlobalNumberSymbolsV1, OmoikaneNumberPatternsV1, OmoikaneNumberRangePatternsV1,
    OmoikaneNumberSymbolsV1, OmoikaneUnitPatternsV1, PluralCategory,
    SANCTIONED_SIMPLE_UNIT_IDENTIFIERS,
};
use icu_provider::{buf::AsDeserializingBufferProvider, prelude::*};
use yoke::Yokeable;

#[test]
fn full_calendar_intervals_borrow_raw_patterns_and_preserve_locale_order() {
    let gregorian = load::<OmoikaneCalendarIntervalPatternsV1>("en-US", "gregory");
    assert_eq!(
        gregorian.get().get("yMMMd", "d"),
        Some("MMM d\u{2009}–\u{2009}d, y")
    );
    assert_eq!(gregorian.get().fallback(), Some("{0}\u{2009}–\u{2009}{1}"));
    for calendar in [
        "buddhist",
        "chinese",
        "coptic",
        "dangi",
        "ethioaa",
        "ethiopic",
        "gregory",
        "hebrew",
        "indian",
        "islamic",
        "islamic-civil",
        "islamic-rgsa",
        "islamic-tbla",
        "islamic-umalqura",
        "japanese",
        "persian",
        "roc",
    ] {
        let payload = load::<OmoikaneCalendarIntervalPatternsV1>("zh", calendar);
        assert!(payload.get().skeletons().next().is_some());
        assert!(payload.get().fallback().is_some());
    }
    let bytes = include_bytes!("../data/omoikane_intl.postcard");
    let provider = icu_provider_blob::BlobDataProvider::try_new_from_static_blob(bytes).unwrap();
    let locale: DataLocale = "en".parse().unwrap();
    let attributes = DataMarkerAttributes::from_str_or_panic("gregory");
    let payload = DataProvider::<OmoikaneCalendarIntervalPatternsV1>::load(
        &provider.as_deserializing(),
        DataRequest {
            id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
            ..Default::default()
        },
    )
    .unwrap()
    .payload;
    for text in [payload.get().get("yMMMd", "d"), payload.get().fallback()] {
        let text = text.unwrap();
        let start = bytes.as_ptr() as usize;
        let pointer = text.as_ptr() as usize;
        assert!(pointer >= start && pointer + text.len() <= start + bytes.len());
    }
}

#[test]
fn number_ranges_keep_local_aliases_ordered_rules_and_borrowed_patterns() {
    use PluralCategory::{One, Other};
    let japanese = load::<OmoikaneNumberRangePatternsV1>("ja", "arab");
    assert_eq!(japanese.get().range(), Some("{0}～{1}"));
    assert_eq!(japanese.get().approximately(), Some("約 {0}"));
    let japanese_default = load::<OmoikaneNumberRangePatternsV1>("ja", "");
    assert_eq!(japanese_default.get(), japanese.get());
    let german = load::<OmoikaneNumberRangePatternsV1>("de", "latn");
    assert_eq!(german.get().approximately(), Some("≈{0}"));
    assert_eq!(german.get().plural_range(One, Other), Some(Other));
    assert_eq!(german.get().plural_range(Other, One), Some(One));
    assert_eq!(german.get().plural_range(One, One), None);
    let latin_serbian = load::<OmoikaneNumberRangePatternsV1>("sr-Latn", "latn");
    let serbian = load::<OmoikaneNumberRangePatternsV1>("sr", "latn");
    assert_eq!(
        latin_serbian.get().plural_results,
        serbian.get().plural_results
    );
    assert_eq!(
        latin_serbian.get().plural_present,
        serbian.get().plural_present
    );
    let portuguese = load::<OmoikaneNumberRangePatternsV1>("pt-PT", "latn");
    assert_eq!(portuguese.get().range(), Some("{0} - {1}"));
    assert_eq!(portuguese.get().plural_range(One, One), Some(One));
    let root = load::<OmoikaneNumberRangePatternsV1>("und", "thai");
    assert_eq!(root.get().range(), Some("{0}–{1}"));
    assert_eq!(root.get().plural_range(Other, Other), None);

    let bytes = include_bytes!("../data/omoikane_intl.postcard");
    let provider = icu_provider_blob::BlobDataProvider::try_new_from_static_blob(bytes).unwrap();
    let locale: DataLocale = "ja".parse().unwrap();
    let attributes = DataMarkerAttributes::from_str_or_panic("arab");
    let payload = DataProvider::<OmoikaneNumberRangePatternsV1>::load(
        &provider.as_deserializing(),
        DataRequest {
            id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
            ..Default::default()
        },
    )
    .unwrap()
    .payload;
    assert!(!payload.get().fields.values.is_owned());
    assert!(!payload.get().plural_results.is_owned());
    for pattern in [payload.get().range(), payload.get().approximately()] {
        let pattern = pattern.unwrap();
        let start = bytes.as_ptr() as usize;
        let pointer = pattern.as_ptr() as usize;
        assert!(pointer >= start && pointer + pattern.len() <= start + bytes.len());
    }
}

#[test]
fn compact_data_retains_source_thresholds_omission_sentinels_and_alpha_categories() {
    let english = load::<OmoikaneCompactPatternsV1>("en-US", "decimal-short-latn");
    assert_eq!(english.get().get(1000, PluralCategory::Other), Some("0K"));
    let french = load::<OmoikaneCompactPatternsV1>("fr", "decimal-long-latn");
    assert_eq!(
        french.get().get(1000, PluralCategory::ExplicitOne),
        Some("mille")
    );
    let japanese = load::<OmoikaneCompactPatternsV1>("ja", "decimal-short-latn");
    assert_eq!(japanese.get().get(1000, PluralCategory::Other), Some("0"));
    assert_eq!(
        japanese.get().get(10000, PluralCategory::Other),
        Some("0万")
    );
    let japanese_long = load::<OmoikaneCompactPatternsV1>("ja", "decimal-long-latn");
    assert_eq!(
        japanese_long.get().magnitudes().last(),
        Some(10_000_000_000_000_000_000)
    );
    assert_eq!(
        japanese_long
            .get()
            .get(10_000_000_000_000_000_000, PluralCategory::Other),
        Some("0000京")
    );
    let hebrew = load::<OmoikaneCompactPatternsV1>("he", "currency-short-latn");
    assert_eq!(hebrew.get().get(1000, PluralCategory::Many), None);
    assert_eq!(
        hebrew
            .get()
            .get_alpha_next_to_number(1000, PluralCategory::Many),
        Some("¤\u{a0}0K\u{200f}")
    );

    let bytes = include_bytes!("../data/omoikane_intl.postcard");
    let provider = icu_provider_blob::BlobDataProvider::try_new_from_static_blob(bytes).unwrap();
    let locale: DataLocale = "en".parse().unwrap();
    let attributes = DataMarkerAttributes::from_str_or_panic("decimal-short-latn");
    let payload = DataProvider::<OmoikaneCompactPatternsV1>::load(
        &provider.as_deserializing(),
        DataRequest {
            id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
            ..Default::default()
        },
    )
    .unwrap()
    .payload;
    let pattern = payload.get().get(1000, PluralCategory::Other).unwrap();
    let start = bytes.as_ptr() as usize;
    let pointer = pattern.as_ptr() as usize;
    assert!(pointer >= start && pointer + pattern.len() <= start + bytes.len());
}

#[test]
fn exact_calendar_patterns_preserve_cyclic_years_and_borrow_the_blob() {
    let chinese = load::<OmoikaneCalendarDatePatternsV1>("zh", "chinese");
    assert_eq!(chinese.get().get("yyyyMMMd"), Some("r年MMMd"));
    assert_eq!(chinese.get().get("yyyyMMMMd"), Some("rU年MMMMd"));
    let gregorian = load::<OmoikaneCalendarDatePatternsV1>("en-US", "gregory");
    assert_eq!(gregorian.get().get("yMd"), Some("M/d/y"));
    for calendar in [
        "buddhist",
        "chinese",
        "coptic",
        "dangi",
        "ethioaa",
        "ethiopic",
        "gregory",
        "hebrew",
        "indian",
        "islamic",
        "islamic-civil",
        "islamic-rgsa",
        "islamic-tbla",
        "islamic-umalqura",
        "japanese",
        "persian",
        "roc",
    ] {
        assert!(
            !load::<OmoikaneCalendarDatePatternsV1>("zh", calendar)
                .get()
                .available_formats
                .is_empty()
        );
    }

    // Load directly from the exact static backing slice so pointer containment
    // proves this map borrows serialized strings instead of allocating copies.
    let bytes = include_bytes!("../data/omoikane_intl.postcard");
    let provider = icu_provider_blob::BlobDataProvider::try_new_from_static_blob(bytes).unwrap();
    let locale: DataLocale = "zh".parse().unwrap();
    let attributes = DataMarkerAttributes::from_str_or_panic("chinese");
    let payload = DataProvider::<OmoikaneCalendarDatePatternsV1>::load(
        &provider.as_deserializing(),
        DataRequest {
            id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
            ..Default::default()
        },
    )
    .unwrap()
    .payload;
    let pattern = payload.get().get("yyyyMMMMd").unwrap();
    let start = bytes.as_ptr() as usize;
    let pointer = pattern.as_ptr() as usize;
    assert!(pointer >= start && pointer + pattern.len() <= start + bytes.len());
}

fn load<M: DataMarker>(locale: &str, attributes: &str) -> DataPayload<M>
where
    for<'data> <M::DataStruct as Yokeable<'data>>::Output: serde::Deserialize<'data>,
{
    let locale: DataLocale = locale.parse().unwrap();
    let attributes = DataMarkerAttributes::try_from_str(attributes).unwrap();
    let request = DataRequest {
        id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
        ..Default::default()
    };
    DataProvider::<M>::load(&boa_icu_provider::buffer().as_deserializing(), request)
        .unwrap()
        .payload
}

#[test]
fn currency_text_patterns_and_normal_digits_are_localized() {
    let patterns = load::<OmoikaneCurrencyPatternsV1>("en-US", "latn");
    assert_eq!(
        patterns.get().get(CurrencyPatternField::Standard),
        Some("¤#,##0.00")
    );
    assert_eq!(
        patterns.get().get(CurrencyPatternField::Accounting),
        Some("¤#,##0.00;(¤#,##0.00)")
    );
    let text = load::<OmoikaneCurrencyTextV1>("en-US", "USD");
    assert_eq!(text.get().get(CurrencyTextField::Symbol), Some("$"));
    assert_eq!(
        text.get().plural_names.get(PluralCategory::Other),
        Some("US dollars")
    );
    let cad = load::<OmoikaneCurrencyTextV1>("en-US", "CAD");
    assert_eq!(
        cad.get().get(CurrencyTextField::DisplayName),
        Some("Canadian Dollar")
    );
    assert_eq!(cad.get().get(CurrencyTextField::Symbol), Some("CA$"));
    assert_eq!(cad.get().get(CurrencyTextField::NarrowSymbol), Some("$"));
    let explicit_code = load::<OmoikaneCurrencyTextV1>("en-US", "AED");
    assert_eq!(
        explicit_code.get().get(CurrencyTextField::Symbol),
        Some("AED")
    );
    assert_eq!(
        explicit_code.get().get(CurrencyTextField::NarrowSymbol),
        None
    );
    let name_only = load::<OmoikaneCurrencyTextV1>("en-US", "ADP");
    assert_eq!(name_only.get().get(CurrencyTextField::Symbol), None);
    assert_eq!(name_only.get().get(CurrencyTextField::NarrowSymbol), None);
    let digits = load::<OmoikaneCurrencyDigitsV1>("und", "");
    assert_eq!(digits.get().digits("JPY"), 0);
    assert_eq!(digits.get().digits("KWD"), 3);
    assert_eq!(digits.get().digits("CLF"), 4);
    assert_eq!(digits.get().digits("XZZ"), 2);
}

#[test]
fn localized_nonfinite_signs_and_percent_prefix_survive_zero_copy_loading() {
    let arabic = load::<OmoikaneNumberSymbolsV1>("ar", "arab");
    assert_eq!(arabic.get().get(NumberSymbol::NaN), Some("ليس\u{a0}رقمًا"));
    assert_eq!(arabic.get().get(NumberSymbol::MinusSign), Some("\u{061c}-"));
    assert!(!arabic.get().fields.values.is_owned());
    let turkish = load::<OmoikaneNumberPatternsV1>("tr", "latn");
    assert_eq!(
        turkish.get().get(NumberPatternField::Percent),
        Some("%#,##0")
    );
    assert!(!turkish.get().fields.values.is_owned());
    let root = load::<OmoikaneGlobalNumberPatternsV1>("und", "thai");
    assert_eq!(
        root.get().get(NumberPatternField::Decimal),
        Some("#,##0.###")
    );
    let root_currency = load::<OmoikaneGlobalCurrencyPatternsV1>("und", "arab");
    assert_eq!(
        root_currency.get().get(CurrencyPatternField::Standard),
        Some("#,##0.00\u{a0}¤")
    );
}

#[test]
fn all_unit_widths_compound_per_and_omitted_number_patterns_are_available() {
    let arabic = load::<OmoikaneUnitPatternsV1>("ar", "long-duration-second");
    assert_eq!(
        arabic.get().patterns.get(PluralCategory::One),
        Some("ثانية")
    );
    assert_eq!(
        arabic.get().patterns.get(PluralCategory::Two),
        Some("ثانيتان")
    );
    assert!(!arabic.get().patterns.fields.values.is_owned());
    let meter = load::<OmoikaneUnitPatternsV1>("en", "long-length-meter");
    assert_eq!(
        meter.get().patterns.get(PluralCategory::Other),
        Some("{0} meters")
    );
    let compound = load::<OmoikaneCompoundUnitPatternsV1>("en", "long");
    assert_eq!(compound.get().per_pattern(), Some("{0} per {1}"));
    let speed = load::<OmoikaneUnitPatternsV1>("en", "long-speed-kilometer-per-hour");
    assert_eq!(
        speed.get().patterns.get(PluralCategory::Other),
        Some("{0} kilometers per hour")
    );

    for width in ["long", "short", "narrow"] {
        for &(_, cldr) in SANCTIONED_SIMPLE_UNIT_IDENTIFIERS
            .iter()
            .chain(CANONICAL_COMPOUND_UNIT_IDENTIFIERS)
        {
            let payload = load::<OmoikaneUnitPatternsV1>("en", &format!("{width}-{cldr}"));
            assert!(payload.get().patterns.get(PluralCategory::Other).is_some());
        }
    }

    let locale: DataLocale = "en".parse().unwrap();
    let attributes = DataMarkerAttributes::from_str_or_panic("long-acceleration-g-force");
    let request = DataRequest {
        id: DataIdentifierBorrowed::for_marker_attributes_and_locale(attributes, &locale),
        ..Default::default()
    };
    let error = DataProvider::<OmoikaneUnitPatternsV1>::load(
        &boa_icu_provider::buffer().as_deserializing(),
        request,
    )
    .unwrap_err();
    assert_eq!(error.kind, DataErrorKind::IdentifierNotFound);
}

#[test]
fn global_digital_separator_is_available_when_locale_has_no_numbering_system() {
    let digital = load::<OmoikaneDurationDigitalV1>("en-US", "arabext");
    assert_eq!(
        digital.get().get(DurationDigitalField::TimeSeparator),
        Some("٫")
    );
    assert_eq!(
        digital.get().get(DurationDigitalField::NumberingSystem),
        Some("arabext")
    );
    assert!(!digital.get().fields.values.is_owned());
    let digital_default = load::<OmoikaneDurationDigitalV1>("en-US", "");
    assert_eq!(
        digital_default
            .get()
            .get(DurationDigitalField::TimeSeparator),
        Some(":")
    );
    assert_eq!(
        digital_default
            .get()
            .get(DurationDigitalField::NumberingSystem),
        Some("latn")
    );
    let root = load::<OmoikaneGlobalNumberSymbolsV1>("und", "arabext");
    assert_eq!(root.get().get(NumberSymbol::TimeSeparator), Some("٫"));
    let default = load::<OmoikaneNumberSymbolsV1>("en-US", "");
    assert_eq!(
        default.get().get(NumberSymbol::NumberingSystem),
        Some("latn")
    );
}

#[test]
fn all_pinned_numeric_digits_are_formatted_without_latin_fallback() {
    use icu_decimal::{DecimalFormatter, options::GroupingStrategy};
    use icu_locale::extensions::unicode::Value;

    let expected = include_str!("../data/icu_decimal_digits.source.tsv");
    let mut tested = 0;
    for line in expected.lines() {
        let (nu, digits) = line.split_once('\t').unwrap();
        let mut prefs: icu_decimal::DecimalFormatterPreferences = icu_locale::locale!("en").into();
        prefs.numbering_system = Some(Value::try_from_str(nu).unwrap().try_into().unwrap());
        let mut options = icu_decimal::options::DecimalFormatterOptions::default();
        options.grouping_strategy = Some(GroupingStrategy::Never);
        let formatter = DecimalFormatter::try_new_with_buffer_provider(
            &boa_icu_provider::buffer(),
            prefs,
            options,
        )
        .unwrap();
        assert_eq!(digits.chars().count(), 10);
        for (digit, expected) in digits.chars().enumerate() {
            let actual = formatter.format(&(digit as u64).into()).to_string();
            assert_eq!(actual, expected.to_string(), "{nu}: digit {digit}");
        }
        tested += 1;
    }
    assert_eq!(tested, 77);
}

#[test]
fn native_names_keep_localized_raw_labels_and_absent_translations() {
    use boa_intl_data::{OmoikaneDisplayNamesV1, OmoikaneRelativeTimePatternsV1};
    let relative = load::<OmoikaneRelativeTimePatternsV1>("ja", "");
    assert_eq!(relative.get().records.get("day/long/auto/-1"), Some("昨日"));
    let english = load::<OmoikaneDisplayNamesV1>("en", "");
    assert_eq!(
        english.get().records.get("region/long/US"),
        Some("United States")
    );
    assert_eq!(english.get().records.get("currency/long/ZZZ"), None);
    assert_eq!(
        english.get().records.get("scriptContext/long/Hans"),
        Some("Simplified")
    );
    assert_eq!(
        english.get().records.get("script/long/Hans"),
        Some("Simplified Han")
    );
    let root = load::<OmoikaneDisplayNamesV1>("und", "");
    assert_eq!(root.get().records.get("language/long/en"), None);
}

#[test]
fn native_name_payloads_are_borrowed_from_blob_for_outside_modern_locales() {
    use boa_intl_data::{OmoikaneDisplayNamesV1, OmoikaneRelativeTimePatternsV1};
    let bytes = include_bytes!("../data/omoikane_intl.postcard");
    let provider = icu_provider_blob::BlobDataProvider::try_new_from_static_blob(bytes).unwrap();
    for name in ["en", "ja", "de", "bal", "aa", "und"] {
        let locale: DataLocale = name.parse().unwrap();
        let request = DataRequest {
            id: DataIdentifierBorrowed::for_locale(&locale),
            ..Default::default()
        };
        let names =
            DataProvider::<OmoikaneDisplayNamesV1>::load(&provider.as_deserializing(), request)
                .unwrap()
                .payload;
        let relative = DataProvider::<OmoikaneRelativeTimePatternsV1>::load(
            &provider.as_deserializing(),
            request,
        )
        .unwrap()
        .payload;
        for value in [
            names.get().records.get("pattern/localePattern").unwrap(),
            relative.get().records.get("day/long/future/other").unwrap(),
        ] {
            let pointer = value.as_ptr() as usize;
            assert!(
                pointer >= bytes.as_ptr() as usize
                    && pointer + value.len() <= bytes.as_ptr() as usize + bytes.len()
            );
        }
    }
}
