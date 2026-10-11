import unittest
from supported_values import extract_supported_values, validate_supported_values


class SupportedValuesTests(unittest.TestCase):
    def test_inventories_preserve_canonical_codes_and_exclude_special_collations(self):
        data = {"calendar_date_patterns.json": {"en": {"gregory": {}}, "fr": {"gregory": {}}},
                "currency_names.json": {"en": {"USD": {}, "XXX": {}}, "fr": {"USD": {}, "JPY": {}}},
                "root_number_symbols.json": {"numberingSystemSymbols": {"latn": {}, "arab": {}}}}
        source = {"keyword": {"u": {"co": {"_alias": "collation", "phonebk": {"_alias": "phonebook"},
                                               "search": {}, "standard": {}, "direct": {"_deprecated": True}}}}}
        time_data = {"supplemental": {"timeData": {"JP": {"_allowed": "H K h"},
                                                       "US": {"_allowed": "h hb H hB"}}}}
        values = extract_supported_values(data, source, time_data)
        self.assertEqual(values["hourCycle12h11Regions"], ["JP"])
        self.assertEqual(values["calendar"], ["gregory", "iso8601"])
        self.assertEqual(values["collation"], ["phonebk"])
        self.assertEqual(values["currency"], ["JPY", "USD", "XXX"])
        self.assertEqual(values["numberingSystem"], ["arab", "latn"])
        self.assertEqual(values["locales"], ["en", "fr"])
        validate_supported_values(values)
        values["currency"].append("usd")
        with self.assertRaises(ValueError):
            validate_supported_values(values)


if __name__ == "__main__":
    unittest.main()
