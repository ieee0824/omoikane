"""Hebrew CLDR ordinal names must be stored by stable month code."""
import unittest
from extract_hebrew_months import names_by_code


class HebrewMonthTests(unittest.TestCase):
    def test_leap_and_common_adar_names_keep_distinct_codes(self):
        source = dict(zip(map(str, range(1, 14)), [
            "Tishri", "Heshvan", "Kislev", "Tevet", "Shevat", "Adar I", "Adar",
            "Nisan", "Iyar", "Sivan", "Tamuz", "Av", "Elul",
        ]))
        source["7-yeartype-leap"] = "Adar II"
        names = names_by_code(source)
        self.assertEqual(names[:12], ["Tishri", "Heshvan", "Kislev", "Tevet", "Shevat", "Adar",
                                     "Nisan", "Iyar", "Sivan", "Tamuz", "Av", "Elul"])
        self.assertEqual(names[12:], ["", "", "", "", "Adar I", "Adar II", "", "", "", "", "", ""])

    def test_missing_leap_name_is_rejected(self):
        with self.assertRaises(KeyError):
            names_by_code({str(number): "month" for number in range(1, 14)})


if __name__ == "__main__":
    unittest.main()
