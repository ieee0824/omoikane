use boa_intl_data::{
    CANONICAL_COMPOUND_UNIT_IDENTIFIERS, CalendarDatePatterns, CalendarIntervalPatterns,
    CompactPatterns, CurrencyDigits, NumberRangePatterns, PluralCategory, PluralPatterns,
    SANCTIONED_SIMPLE_UNIT_IDENTIFIERS, StringFields, UnitPatterns,
    canonical_compound_unit_cldr_id, is_reachable_unit_cldr_id, sanctioned_simple_unit,
};
use zerovec::{ZeroMap, ZeroMap2d, ZeroVec};

#[test]
fn shared_unit_taxonomy_is_sorted_unique_and_covers_only_runtime_records() {
    assert_eq!(SANCTIONED_SIMPLE_UNIT_IDENTIFIERS.len(), 45);
    for pair in SANCTIONED_SIMPLE_UNIT_IDENTIFIERS.windows(2) {
        assert!(
            pair[0].0 < pair[1].0,
            "ECMA identifiers must be unique and sorted"
        );
    }
    let cldr_identifiers = SANCTIONED_SIMPLE_UNIT_IDENTIFIERS
        .iter()
        .chain(CANONICAL_COMPOUND_UNIT_IDENTIFIERS)
        .map(|&(_, cldr)| cldr)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        cldr_identifiers.len(),
        46,
        "CLDR identifiers must be unique"
    );
    for &(ecma, cldr) in SANCTIONED_SIMPLE_UNIT_IDENTIFIERS {
        assert_eq!(sanctioned_simple_unit(ecma), Some((ecma, cldr)));
        assert!(is_reachable_unit_cldr_id(cldr));
    }
    assert_eq!(sanctioned_simple_unit("g-force"), None);
    assert_eq!(CANONICAL_COMPOUND_UNIT_IDENTIFIERS.len(), 1);
    assert_eq!(
        canonical_compound_unit_cldr_id("kilometer-per-hour"),
        Some("speed-kilometer-per-hour")
    );
    assert!(is_reachable_unit_cldr_id("speed-kilometer-per-hour"));
    assert!(!is_reachable_unit_cldr_id("acceleration-g-force"));
}

#[test]
fn calendar_intervals_preserve_quotes_variants_order_and_distinct_calendar_fields() {
    let mut patterns = ZeroMap2d::<str, str, str>::new();
    patterns.insert("yMMMd", "d", "MMM d 'M'\u{2009}–\u{2009}d, y");
    patterns.insert("yMMMd", "d-alt-variant", "d–d MMM y");
    patterns.insert("yyyyMMMMd", "y", "rU年MMMMd–rU年MMMMd");
    let data = CalendarIntervalPatterns {
        patterns,
        fields: StringFields::from_fields(&[Some("{1} - {0}")]).unwrap(),
    };
    assert_eq!(
        data.get("yMMMd", "d"),
        Some("MMM d 'M'\u{2009}–\u{2009}d, y")
    );
    assert_eq!(data.get("yMMMd", "d-alt-variant"), Some("d–d MMM y"));
    assert_eq!(data.get("yyyyMMMMd", "y"), Some("rU年MMMMd–rU年MMMMd"));
    assert_eq!(data.get("yyyyMMMMd", "G"), None);
    assert_eq!(data.fallback(), Some("{1} - {0}"));
    assert_eq!(data.skeletons().collect::<Vec<_>>(), ["yMMMd", "yyyyMMMMd"]);
}

#[test]
fn number_ranges_preserve_order_missing_rules_and_raw_literals() {
    use PluralCategory::{ExplicitOne, ExplicitZero, One, Other};
    let mut results = [0; 36];
    results[One as usize * 6 + Other as usize] = Other as u8;
    results[Other as usize * 6 + One as usize] = One as u8;
    let data = NumberRangePatterns {
        fields: StringFields::from_fields(&[Some("{1}\u{200f} – '{0}'"), Some("約 {0}")]).unwrap(),
        plural_results: ZeroVec::alloc_from_slice(&results),
        plural_present: (1 << (One as usize * 6 + Other as usize))
            | (1 << (Other as usize * 6 + One as usize)),
    };
    assert_eq!(data.range(), Some("{1}\u{200f} – '{0}'"));
    assert_eq!(data.approximately(), Some("約 {0}"));
    assert_eq!(data.plural_range(One, Other), Some(Other));
    assert_eq!(data.plural_range(Other, One), Some(One));
    assert_eq!(data.plural_range(One, One), None);
    assert_eq!(data.plural_range(ExplicitZero, Other), None);
    assert_eq!(data.plural_range(Other, ExplicitOne), None);
}

#[test]
fn malformed_custom_range_rules_do_not_create_cardinal_categories() {
    let data = NumberRangePatterns {
        fields: StringFields::from_fields(&[None, Some("")]).unwrap(),
        plural_results: ZeroVec::alloc_from_slice(&[u8::MAX]),
        plural_present: 1 | (1 << 35),
    };
    assert_eq!(data.range(), None);
    assert_eq!(data.approximately(), Some(""));
    assert_eq!(
        data.plural_range(PluralCategory::Zero, PluralCategory::Zero),
        None
    );
    assert_eq!(
        data.plural_range(PluralCategory::Other, PluralCategory::Other),
        None
    );
}

#[test]
fn compact_thresholds_keep_sentinels_explicit_counts_and_independent_variants() {
    let mut normal = ZeroMap2d::<u64, u8, str>::new();
    normal.insert(&1000, &(PluralCategory::Other as u8), "0");
    normal.insert(&1000, &(PluralCategory::ExplicitOne as u8), "mille");
    normal.insert(&10000, &(PluralCategory::Other as u8), "0万");
    normal.insert(
        &10_000_000_000_000_000_000,
        &(PluralCategory::Other as u8),
        "0K",
    );
    let mut alpha_next_to_number = ZeroMap2d::<u64, u8, str>::new();
    alpha_next_to_number.insert(&1000, &(PluralCategory::Many as u8), "¤\u{a0}0K");
    let data = CompactPatterns {
        normal,
        alpha_next_to_number,
    };
    assert_eq!(data.get(1000, PluralCategory::Other), Some("0"));
    assert_eq!(data.get(1000, PluralCategory::ExplicitOne), Some("mille"));
    assert_eq!(data.get(1000, PluralCategory::One), None);
    assert_eq!(data.get(1000, PluralCategory::Many), None);
    assert_eq!(
        data.get_alpha_next_to_number(1000, PluralCategory::Many),
        Some("¤\u{a0}0K")
    );
    assert_eq!(
        data.magnitudes().collect::<Vec<_>>(),
        [1000, 10000, 10_000_000_000_000_000_000]
    );
}

#[test]
fn calendar_skeletons_preserve_width_dependent_year_fields() {
    let mut available_formats = ZeroMap::<str, str>::new();
    available_formats.insert("yyyyMMMd", "r年MMMd");
    available_formats.insert("yyyyMMMMd", "rU年MMMMd");
    let data = CalendarDatePatterns { available_formats };
    assert_eq!(data.get("yyyyMMMd"), Some("r年MMMd"));
    assert_eq!(data.get("yyyyMMMMd"), Some("rU年MMMMd"));
    assert_eq!(data.get("yMMMMd"), None);
}

#[test]
fn optional_fields_preserve_present_empty_and_absent() {
    let fields = StringFields::from_fields(&[Some(""), None, Some("\u{061c}-")]).unwrap();
    assert_eq!(fields.get(0), Some(""));
    assert_eq!(fields.get(1), None);
    assert_eq!(fields.get(2), Some("\u{061c}-"));
    assert_eq!(fields.get(64), None);
    assert!(StringFields::from_fields(&[None; 65]).is_none());
}

#[test]
fn unit_only_plural_patterns_do_not_require_numeric_placeholders() {
    let patterns = PluralPatterns {
        fields: StringFields::from_fields(&[
            None,
            Some("ثانية"),
            Some("ثانيتان"),
            None,
            None,
            Some("{0} ثانية"),
        ])
        .unwrap(),
    };
    let unit = UnitPatterns {
        patterns,
        fields: StringFields::from_fields(&[Some("second"), Some("{0}/s")]).unwrap(),
    };
    assert_eq!(
        unit.patterns.get_or_other(PluralCategory::One),
        Some("ثانية")
    );
    assert_eq!(
        unit.patterns.get_or_other(PluralCategory::Two),
        Some("ثانيتان")
    );
    assert_eq!(
        unit.patterns.get_or_other(PluralCategory::Few),
        Some("{0} ثانية")
    );
    assert_eq!(unit.per_unit_pattern(), Some("{0}/s"));
}

#[test]
fn normal_currency_precision_is_separate_from_cash_overrides() {
    let mut normal_digits = ZeroMap::new();
    normal_digits.insert("JPY", &0);
    normal_digits.insert("KWD", &3);
    normal_digits.insert("CLF", &4);
    normal_digits.insert("CHF", &2);
    let mut cash_rounding = ZeroMap::new();
    cash_rounding.insert("CHF", &5);
    let digits = CurrencyDigits {
        default_digits: 2,
        normal_digits,
        normal_rounding: ZeroMap::new(),
        cash_digits: ZeroMap::new(),
        cash_rounding,
    };
    assert_eq!(digits.digits("JPY"), 0);
    assert_eq!(digits.digits("KWD"), 3);
    assert_eq!(digits.digits("CLF"), 4);
    assert_eq!(digits.digits("CHF"), 2);
    assert_eq!(digits.digits("XZZ"), 2);
    assert_eq!(digits.cash_rounding.get_copied("CHF"), Some(5));
}
