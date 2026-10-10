"""Regression fixtures for the independent CLDR name data extraction."""
import unittest
from relative_display_names import UNITS, labels, relative_records, script_labels


class NativeNameDataTests(unittest.TestCase):
    def test_relative_patterns_keep_unicode_negative_zero_label_and_plural_categories(self):
        source = {unit: {"relativeTime-type-past": {"relativeTimePattern-count-other": "{0}\u00a0ago"},
                         "relativeTime-type-future": {"relativeTimePattern-count-other": "in\u200f {0}"}}
                  for unit in UNITS}
        source["day"]["relative-type-0"] = "today"
        source["day"]["relativeTime-type-future"]["relativeTimePattern-count-one"] = "in {0} day"
        records = relative_records(source)
        self.assertEqual(records["day/long/auto/0"], "today")
        self.assertEqual(records["day/long/past/other"], "{0}\u00a0ago")
        self.assertEqual(records["day/long/future/other"], "in\u200f {0}")
        self.assertEqual(records["day/long/future/one"], "in {0} day")
        self.assertNotIn("day/short/auto/0", records)

    def test_arabic_singular_without_numeric_placeholder_is_preserved(self):
        source = {unit: {"relativeTime-type-past": {"relativeTimePattern-count-other": "{0} ago"},
                         "relativeTime-type-future": {"relativeTimePattern-count-other": "in {0}"}}
                  for unit in UNITS}
        source["year"]["relativeTime-type-future"]["relativeTimePattern-count-one"] = "خلال سنة واحدة"
        self.assertEqual(relative_records(source)["year/long/future/one"], "خلال سنة واحدة")

    def test_missing_translations_and_short_labels_remain_distinct(self):
        records = {}
        labels(records, "region", {"US": "United States", "US-alt-short": "US", "ZZ-alt-variant": "variant"})
        self.assertEqual(records["region/long/US"], "United States")
        self.assertEqual(records["region/short/US"], "US")
        self.assertNotIn("region/narrow/US", records)
        self.assertNotIn("region/long/ZZ", records)

    def test_script_stand_alone_long_keeps_contextual_short_for_narrow_fallback(self):
        records = {}
        script_labels(records, {"Hans": "Simplified", "Hans-alt-stand-alone": "Simplified Han",
                                "Cans": "Unified Canadian Aboriginal Syllabics",
                                "Cans-alt-short": "UCAS", "Cans-alt-stand-alone": "Canadian Syllabics"})
        self.assertEqual(records["script/long/Hans"], "Simplified Han")
        self.assertEqual(records["script/short/Hans"], "Simplified")
        self.assertNotIn("script/narrow/Hans", records)
        self.assertEqual(records["script/long/Cans"], "Canadian Syllabics")
        self.assertEqual(records["script/short/Cans"], "UCAS")
        self.assertNotIn("script/narrow/Cans", records)

    def test_malformed_relative_templates_are_rejected(self):
        source = {unit: {"relativeTime-type-past": {"relativeTimePattern-count-other": "{0} ago"},
                         "relativeTime-type-future": {"relativeTimePattern-count-other": "in {0}"}}
                  for unit in UNITS}
        source["day"]["relativeTime-type-past"]["relativeTimePattern-count-other"] = "{0} / {0}"
        with self.assertRaises(ValueError):
            relative_records(source)


if __name__ == "__main__":
    unittest.main()
