import unittest
from datetime_extras import rule_locale, validate_datetime_extras


class DateTimeExtrasTests(unittest.TestCase):
    def test_day_period_rules_follow_language_script_fallback(self):
        rules = {"en": {}, "zh-Hant": {}, "zh": {}, "und": {}}
        self.assertEqual(rule_locale("en-GB", rules), "en")
        self.assertEqual(rule_locale("zh-Hant-HK", rules), "zh-Hant")
        self.assertEqual(rule_locale("zh-Hans-CN", rules), "zh")
        self.assertEqual(rule_locale("zz-Latn", rules), "und")
        with self.assertRaises(ValueError):
            rule_locale("zz", {"en": {}})

    def test_style_validation_preserves_numbering_overrides_and_quotes(self):
        short = {"_value": "d/M/yy 'quoted'", "_numbers": "M=romanlow"}
        styles = {family: dict.fromkeys(["full", "long", "medium", "short"], "h:mm")
                  for family in ["date", "time", "glue"]}
        styles["date"]["short"] = short
        names = {width: {"am": "AM", "pm": "PM"} for width in ["wide", "abbreviated", "narrow"]}
        periods = {"names": names, "rules": {"night1": {"_from": "21:00", "_before": "06:00"}}}
        datasets = {"datetime_style_patterns.json": {"en": {"gregory": {"patterns": styles}}},
                    "day_periods.json": {"en": periods}}
        self.assertEqual(validate_datetime_extras(datasets, {"en"})["dayPeriodRules"], 1)
        self.assertEqual(styles["date"]["short"], short)
        periods["rules"]["night1"]["_before"] = "24:01"
        with self.assertRaises(ValueError):
            validate_datetime_extras(datasets, {"en"})
