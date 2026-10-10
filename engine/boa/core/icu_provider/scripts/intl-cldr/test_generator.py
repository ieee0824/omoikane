import pathlib
import tempfile
import unittest
import io
import json
import zipfile

from generate import extract_currencies, extract_fractions, extract_number_patterns, extract_patterns, extract_symbols, generate
from patterns import parse_number_pattern, split_subpatterns
from root_symbols import resolve_symbols
from calendar_patterns import extract_calendar_patterns
from calendar_intervals import extract_calendar_intervals
from compact_patterns import extract_compact_patterns, has_unquoted_zero, split_key
from number_range_patterns import extract_number_ranges, extract_plural_ranges, resolve_plural_locale, root_alias, validate_pattern
import xml.etree.ElementTree as ET


class CalendarIntervalTests(unittest.TestCase):
    def test_source_variants_calendar_identity_quotes_and_reverse_order_survive(self):
        buffer = io.BytesIO()
        intervals = {"intervalFormatFallback": "{1} - {0}",
                     "yMMMd": {"d": "MMM d 'M'–d, y", "d-alt-variant": "d–d MMM y"},
                     "yyyyMMMMd": {"y": "rU年MMMMd–rU年MMMMd"}}
        document = {"main": {"zh": {"dates": {"calendars": {"chinese": {
            "dateTimeFormats": {"intervalFormats": intervals}}}}}}}
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("cldr-cal-fixture-full/main/zh/ca-fixture.json", json.dumps(document))
        with zipfile.ZipFile(buffer) as archive:
            actual = extract_calendar_intervals(archive, lambda filename: json.loads(archive.read(filename)))
        self.assertEqual(actual["zh"]["chinese"], {
            "sourceCalendar": "chinese", "fallback": "{1} - {0}",
            "patterns": {key: value for key, value in intervals.items() if key != "intervalFormatFallback"}})

    def test_duplicate_calendar_source_is_rejected_instead_of_overwritten(self):
        buffer = io.BytesIO()
        document = {"main": {"en": {"dates": {"calendars": {"gregorian": {
            "dateTimeFormats": {"intervalFormats": {"intervalFormatFallback": "{0}–{1}",
                                                      "d": {"d": "d–d"}}}}}}}}}
        with zipfile.ZipFile(buffer, "w") as archive:
            for filename in ["a", "b"]:
                archive.writestr(f"cldr-cal-{filename}-full/main/en/ca-{filename}.json", json.dumps(document))
        with zipfile.ZipFile(buffer) as archive:
            with self.assertRaisesRegex(ValueError, "duplicate"):
                extract_calendar_intervals(archive, lambda filename: json.loads(archive.read(filename)))


class PatternTests(unittest.TestCase):
    def test_standard_negative_sign_precedes_currency(self):
        parsed = parse_number_pattern("¤#,##0.00")
        self.assertFalse(parsed["explicitNegative"])
        self.assertEqual([part["type"] for part in parsed["negative"]], ["minusSign", "currency", "number"])

    def test_accounting_preserves_parentheses(self):
        parsed = parse_number_pattern("¤#,##0.00;(¤#,##0.00)")
        self.assertTrue(parsed["explicitNegative"])
        self.assertEqual(parsed["negative"][0], {"type": "literal", "value": "("})
        self.assertEqual(parsed["negative"][-1], {"type": "literal", "value": ")"})

    def test_locale_negative_sign_after_currency(self):
        parsed = parse_number_pattern("¤\xa0#,##0.00;¤-#,##0.00")
        self.assertEqual([part["type"] for part in parsed["negative"]], ["currency", "minusSign", "number"])

    def test_quoted_semicolon_and_escaped_apostrophe(self):
        pattern = "'a;''b'¤0.00;('a;''b'¤0.00)"
        self.assertEqual(len(split_subpatterns(pattern)), 2)
        self.assertEqual(parse_number_pattern(pattern)["positive"][0], {"type": "literal", "value": "a;'b"})

    def test_quoted_pattern_characters_stay_literals(self):
        parsed = parse_number_pattern("'¤-#0'¤0.00")
        self.assertEqual(parsed["positive"][0], {"type": "literal", "value": "¤-#0"})
        self.assertEqual(sum(part["type"] == "currency" for part in parsed["positive"]), 1)

    def test_bidi_literal_preserved(self):
        parsed = parse_number_pattern("\u200e¤\xa0#,##0.00;\u200e(¤\xa0#,##0.00)")
        self.assertEqual(parsed["positive"][0]["value"], "\u200e")
        self.assertEqual(parsed["negative"][0]["value"], "\u200e(")

    def test_currency_width_is_not_collapsed(self):
        self.assertEqual(parse_number_pattern("¤¤0")["positive"][0], {"type": "currency", "width": 2})

    def test_plus_suffix_is_an_affix(self):
        parsed = parse_number_pattern("¤0.00+")
        self.assertEqual(parsed["positive"][-1], {"type": "plusSign"})

    def test_scientific_exponent_sign_is_part_of_number(self):
        parsed = parse_number_pattern("¤0.00E+00")
        self.assertEqual(parsed["positive"][-1], {"type": "number", "pattern": "0.00E+00"})

    def test_invalid_patterns_raise(self):
        for pattern in ("'unclosed¤0", "¤0;;¤0", "¤0;", "no number", "¤0 x 0"):
            with self.subTest(pattern=pattern), self.assertRaises(ValueError):
                parse_number_pattern(pattern)


class RootAliasTests(unittest.TestCase):
    @staticmethod
    def alias(path):
        element = ET.Element("symbols")
        ET.SubElement(element, "alias", {"source": "locale", "path": path})
        return element

    def test_resolve_numeric_alias_keeps_latin_symbols(self):
        elements = {"latn": ET.fromstring("<symbols><nan>NaN</nan><timeSeparator>:</timeSeparator></symbols>"), "thai": self.alias("../symbols[@numberSystem='latn']")}
        self.assertEqual(resolve_symbols(elements, "thai"), {"nan": "NaN", "timeSeparator": ":"})

    def test_alias_cycles_raise(self):
        elements = {"thai": self.alias("../symbols[@numberSystem='thai']")}
        with self.assertRaisesRegex(ValueError, "cyclic"):
            resolve_symbols(elements, "thai")

    def test_unknown_alias_path_raises(self):
        elements = {"thai": self.alias("../../unknown")}
        with self.assertRaisesRegex(ValueError, "unsupported"):
            resolve_symbols(elements, "thai")


class DataTests(unittest.TestCase):
    def test_percent_prefix_and_scientific_pattern_are_preserved(self):
        numbers = {"decimalFormats-numberSystem-latn": {"standard": "#,##0.###"}, "percentFormats-numberSystem-latn": {"standard": "%#,##0"}, "scientificFormats-numberSystem-latn": {"standard": "#E0"}}
        self.assertEqual(extract_number_patterns(numbers)["latn"], {"decimal": "#,##0.###", "percent": "%#,##0", "scientific": "#E0"})

    def test_nonfinite_localized_symbols_are_kept(self):
        symbols = {"nan": "ليس\xa0رقمًا", "infinity": "∞", "minusSign": "\u061c-", "plusSign": "\u061c+", "decimal": "٫", "group": "٬", "currencyDecimal": "."}
        result = extract_symbols({"symbols-numberSystem-arab": symbols})["arab"]
        self.assertEqual(result["nan"], symbols["nan"])
        self.assertEqual(result["minusSign"], symbols["minusSign"])
        self.assertEqual(result["effectiveCurrencyDecimal"], ".")
        self.assertEqual(result["effectiveCurrencyGroup"], "٬")

    def test_currency_fallback_symbols_and_override_are_explicit(self):
        result = extract_currencies({"USD": {"symbol": "US$", "displayName-count-other": "US dollars"}, "EUR": {"symbol": "€", "symbol-alt-narrow": "€", "decimal": ",", "group": ".", "pattern": "#,##0.00\xa0¤"}})
        self.assertEqual(result["USD"]["effectiveNarrowSymbol"], "US$")
        self.assertEqual(result["USD"]["pluralNames"]["other"], "US dollars")
        self.assertEqual(result["EUR"]["decimal"], ",")
        self.assertEqual(result["EUR"]["patternOverride"]["raw"], "#,##0.00\xa0¤")

    def test_numbering_system_partial_unit_data_remains_visible(self):
        data = {"currencyFormats-numberSystem-arab": {"standard": "¤0.00", "accounting": "¤0.00;(¤0.00)"}}
        self.assertEqual(extract_patterns(data)["arab"]["unitPatterns"], {})

    def test_normal_precision_does_not_apply_cash_rounding(self):
        fractions = {"DEFAULT": {"_digits": "2", "_rounding": "0"}, "CHF": {"_digits": "2", "_rounding": "0", "_cashRounding": "5"}, "JPY": {"_digits": "0", "_rounding": "0"}}
        result = extract_fractions({"supplemental": {"currencyData": {"fractions": fractions}}})
        self.assertEqual(result["currencies"]["CHF"]["effectiveDigits"], 2)
        self.assertEqual(result["currencies"]["CHF"]["cashRounding"], 5)
        self.assertEqual(result["currencies"]["JPY"]["effectiveDigits"], 0)

    def test_wrong_source_hash_creates_no_outputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            source = root / "wrong.zip"
            source.write_bytes(b"not the pinned CLDR ZIP")
            output = root / "output"
            with self.assertRaisesRegex(ValueError, "hash differs"):
                generate(source, output)
            self.assertFalse(output.exists())


class CalendarTests(unittest.TestCase):
    def extract(self, calendar, available):
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("cldr-cal-fixture-full/main/zh/ca-fixture.json", json.dumps({"main": {"zh": {"dates": {"calendars": {calendar: {"dateTimeFormats": {"availableFormats": available}}}}}}}))
        with zipfile.ZipFile(buffer) as archive:
            return extract_calendar_patterns(archive, lambda filename: json.loads(archive.read(filename)))

    def test_cyclic_year_fields_and_month_width_variants_are_preserved(self):
        formats = {"yyyyMMMd": "r年MMMd", "yyyyMMMMd": "rU年MMMMd", "yyyyMMMEd": "rU年MMMdE"}
        self.assertEqual(self.extract("chinese", formats)["zh"]["chinese"]["availableFormats"], formats)

    def test_unicode_calendar_alias_retains_original_source_identifier(self):
        record = self.extract("ethiopic-amete-alem", {"y": "y G"})["zh"]["ethioaa"]
        self.assertEqual(record, {"sourceCalendar": "ethiopic-amete-alem", "availableFormats": {"y": "y G"}})

    def test_quoted_literals_bidi_and_source_variants_are_unchanged(self):
        formats = {"GyMMM": "\u200fMMM 'of' y G", "hm-alt-ascii": "h:mm a"}
        self.assertEqual(self.extract("gregorian", formats)["zh"]["gregory"]["availableFormats"], formats)

    def test_non_string_patterns_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "raw pattern"):
            self.extract("chinese", {"yyyyMMMMd": {"_value": "rU年MMMMd"}})


class CompactTests(unittest.TestCase):
    def test_currency_uses_standard_path_and_keeps_alpha_only_categories(self):
        numbers = {"currencyFormats-numberSystem-latn": {"short": {"standard": {
            "1000-count-other": "¤0K", "1000-count-many-alt-alphaNextToNumber": "¤\xa00K"}}}}
        data = extract_compact_patterns(numbers)["currency-short-latn"]
        self.assertEqual(data["sourcePatternSet"], "standard")
        self.assertEqual(data["normal"], {"1000": {"other": "¤0K"}})
        self.assertEqual(data["alphaNextToNumber"], {"1000": {"many": "¤\xa00K"}})

    def test_raw_zero_explicit_one_and_number_omission_are_not_rewritten(self):
        rows = {"1000-count-other": "0", "1000-count-1": "mille", "10000-count-other": "0万"}
        numbers = {"decimalFormats-numberSystem-latn": {"long": {"decimalFormat": rows}}}
        data = extract_compact_patterns(numbers)["decimal-long-latn"]["normal"]
        self.assertEqual(data, {"1000": {"other": "0", "1": "mille"}, "10000": {"other": "0万"}})

    def test_largest_source_threshold_is_exact_u64(self):
        self.assertEqual(split_key("10000000000000000000-count-other"), ("10000000000000000000", "other", False))
        for key in ("1500-count-other", "100000000000000000000-count-other", "0-count-other", "1000-count-unknown"):
            with self.subTest(key=key), self.assertRaises(ValueError):
                split_key(key)

    def test_quoted_zeros_and_escaped_apostrophes_are_literals(self):
        self.assertFalse(has_unquoted_zero("'0' mille"))
        self.assertTrue(has_unquoted_zero("'0' 0K"))
        self.assertTrue(has_unquoted_zero("''0K"))
        with self.assertRaisesRegex(ValueError, "quote"):
            has_unquoted_zero("'0K")

    def test_wrong_currency_path_does_not_invent_a_payload(self):
        numbers = {"currencyFormats-numberSystem-latn": {"short": {"currencyFormat": {"1000-count-other": "¤0K"}}}}
        self.assertEqual(extract_compact_patterns(numbers), {})

    def test_negative_subpatterns_quotes_and_bidi_are_preserved(self):
        pattern = "\u200f¤' '0K;(\u200f¤' '0K)"
        numbers = {"currencyFormats-numberSystem-latn": {"short": {"standard": {"1000-count-other": pattern}}}}
        self.assertEqual(extract_compact_patterns(numbers)["currency-short-latn"]["normal"]["1000"]["other"], pattern)


class NumberRangeTests(unittest.TestCase):
    def test_order_spacing_and_local_approximation_are_retained(self):
        raw = {"range": "{1}\u200f – '{0}'", "approximately": "約 {0}"}
        numbers = {"defaultNumberingSystem": "latn", "miscPatterns-numberSystem-latn": raw}
        self.assertEqual(extract_number_ranges(numbers)["patterns"]["latn"], raw)

    def test_missing_duplicate_unknown_and_unmatched_placeholders_are_rejected(self):
        for raw, field in (("{0}", "range"), ("{0}-{0}-{1}", "range"), ("{0}-{2}", "range"), ("{0}{", "approximately"), ("{0}-{1}", "approximately")):
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                validate_pattern(raw, field)

    def test_ordered_plural_rules_do_not_fill_missing_pairs(self):
        source = {"supplemental": {"plurals": {"de": {"pluralRange-start-one-end-other": "other", "pluralRange-start-other-end-one": "one"}}}}
        rules = extract_plural_ranges(source)["de"]
        self.assertEqual(rules, {"one": {"other": "other"}, "other": {"one": "one"}})
        self.assertNotIn("one", rules["one"])
        source["supplemental"]["plurals"]["de"]["pluralRange-start-1-end-other"] = "other"
        with self.assertRaisesRegex(ValueError, "cardinal"):
            extract_plural_ranges(source)

    def test_supplemental_script_inheritance_keeps_language_and_unknown_stays_empty(self):
        source = {"sr": {}, "zh": {}, "en": {}}
        self.assertEqual(resolve_plural_locale("sr-Latn-RS", source), "sr")
        self.assertEqual(resolve_plural_locale("zh-Hant-HK", source), "zh")
        self.assertIsNone(resolve_plural_locale("und", source))
        self.assertIsNone(resolve_plural_locale("xyz", source))

    def test_root_alias_is_same_locale_and_cycles_are_rejected(self):
        latin = ET.fromstring('<miscPatterns><pattern type="range">{0}–{1}</pattern></miscPatterns>')
        other = ET.fromstring('<miscPatterns><alias source="locale" path="../miscPatterns[@numberSystem=\'latn\']"/></miscPatterns>')
        self.assertEqual(root_alias({"latn": latin, "arab": other}, "arab"), "latn")
        with self.assertRaisesRegex(ValueError, "cyclic"):
            root_alias({"latn": other}, "latn")
        other.find("alias").set("source", "en")
        with self.assertRaisesRegex(ValueError, "unsupported"):
            root_alias({"latn": latin, "arab": other}, "arab")


if __name__ == "__main__":
    unittest.main(verbosity=2)
