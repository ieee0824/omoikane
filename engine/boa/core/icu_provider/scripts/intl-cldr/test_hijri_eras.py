"""Hijri source names retain width, locale and chronological era indexes."""
import io
import json
import unittest
import zipfile
from extract_hijri_eras import extract


class HijriEraTests(unittest.TestCase):
    def archive(self, eras):
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, "w") as archive:
            archive.writestr("cldr-cal-islamic-full/main/en/ca-islamic.json", json.dumps({
                "main": {"en": {"dates": {"calendars": {"islamic": {"eras": eras}}}}}
            }))
        return zipfile.ZipFile(stream)

    def test_widths_and_both_era_indexes_are_preserved(self):
        eras = {width: {"0": "AH " + width, "1": "BH " + width}
                for width in ("eraNames", "eraAbbr", "eraNarrow")}
        with self.archive(eras) as archive:
            rows, inputs = extract(archive)
        self.assertEqual(len(inputs), 1)
        self.assertEqual(rows.splitlines(), [
            "en\teraAbbr\tAH eraAbbr\tBH eraAbbr",
            "en\teraNames\tAH eraNames\tBH eraNames",
            "en\teraNarrow\tAH eraNarrow\tBH eraNarrow",
        ])

    def test_incomplete_era_source_is_rejected(self):
        with self.archive({"eraAbbr": {"0": "AH"}}) as archive:
            with self.assertRaises(KeyError):
                extract(archive)

    def test_tsv_control_characters_are_rejected(self):
        with self.archive({"eraAbbr": {"0": "AH", "1": "B\tH"}}) as archive:
            with self.assertRaises(ValueError):
                extract(archive)


if __name__ == "__main__":
    unittest.main()
