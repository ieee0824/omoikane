#!/usr/bin/env python3
"""Independently compare generated payloads with every pinned CLDR input record."""
import argparse
import hashlib
import json
import pathlib
import zipfile

from generate import SOURCE_SHA256, SOURCE_URL, CLDR_COMMIT, CLDR_TAG
from root_symbols import ROOT_SHA256, ROOT_URL, ROOT_COMMIT
from verify_native_names import compare_native_names
import xml.etree.ElementTree as ET


def load(archive, name, locale):
    document = json.loads(archive.read(name))
    assert set(document["main"]) == {locale}, (name, locale)
    return document["main"][locale]


def compare_number_records(archive, filename, data):
    locale = filename.split("/")[-2]
    numbers = load(archive, filename, locale)["numbers"]
    symbols = data["number_symbols.json"][locale]
    expected_systems = {key.removeprefix("symbols-numberSystem-") for key in numbers if key.startswith("symbols-numberSystem-")}
    assert set(symbols) == expected_systems, locale
    for numbering_system, record in symbols.items():
        source = numbers["symbols-numberSystem-" + numbering_system]
        for key, value in record.items():
            if key == "effectiveCurrencyDecimal":
                assert value == source.get("currencyDecimal", source["decimal"])
            elif key == "effectiveCurrencyGroup":
                assert value == source.get("currencyGroup", source["group"])
            else:
                assert value == source[key], (locale, numbering_system, key)
    patterns = data["currency_patterns.json"][locale]
    number_patterns = data["number_patterns.json"][locale]
    for kind in ("decimal", "percent", "scientific"):
        prefix = kind + "Formats-numberSystem-"
        expected = {key.removeprefix(prefix): value["standard"] for key, value in numbers.items() if key.startswith(prefix)}
        actual = {nu: record[kind] for nu, record in number_patterns.items() if kind in record}
        assert actual == expected, (locale, kind)
    expected_pattern_systems = {key.removeprefix("currencyFormats-numberSystem-") for key in numbers if key.startswith("currencyFormats-numberSystem-")}
    assert set(patterns) == expected_pattern_systems, locale
    for numbering_system, record in patterns.items():
        source = numbers["currencyFormats-numberSystem-" + numbering_system]
        for key, value in record["patterns"].items():
            assert value["raw"] == source[key], (locale, numbering_system, key)
            assert all(sum(token["type"] == "number" for token in value[sign]) == 1 for sign in ("positive", "negative"))
        assert record["currencySpacing"] == source.get("currencySpacing", {})
        assert record["unitPatterns"] == {key.removeprefix("unitPattern-count-"): value for key, value in source.items() if key.startswith("unitPattern-count-")}
    metadata = data["locale_metadata.json"]["locales"][locale]
    assert metadata["defaultNumberingSystem"] == numbers["defaultNumberingSystem"]
    assert metadata["minimumGroupingDigits"] == int(numbers["minimumGroupingDigits"])
    return locale


def compare_currency_records(archive, locale, data):
    filename = f"cldr-numbers-full/main/{locale}/currencies.json"
    currencies = load(archive, filename, locale)["numbers"]["currencies"]
    records = data["currency_names.json"][locale]
    assert set(records) == set(currencies), locale
    for code, source in currencies.items():
        record = records[code]
        for key, value in source.items():
            if key.startswith("displayName-count-"):
                assert record["pluralNames"][key.removeprefix("displayName-count-")] == value
            elif key == "pattern":
                assert record["patternOverride"]["raw"] == value
            else:
                assert record[key] == value, (locale, code, key)
        assert record["effectiveSymbol"] == source.get("symbol", code)
        assert record["effectiveNarrowSymbol"] == source.get("symbol-alt-narrow", source.get("symbol", code))
    return len(records)


def compare_duration_records(archive, locale, data):
    source = load(archive, f"cldr-units-full/main/{locale}/units.json", locale)["units"]
    record = data["duration_unit_patterns.json"][locale]
    for width in ("long", "short", "narrow"):
        for unit, value in record[width].items():
            assert value == source[width]["duration-" + unit], (locale, width, unit)
    for form in ("hm", "hms", "ms"):
        assert record["digital"][form] == source["durationUnit-type-" + form]["durationUnitPattern"]
    all_units = data["all_unit_patterns.json"][locale]
    for width in ("long", "short", "narrow"):
        expected = {unit: value for unit, value in source[width].items() if any(key.startswith("unitPattern-count-") for key in value)}
        assert all_units[width] == expected, (locale, width)
        expected_compounds = {key: source[width][key] for key in ("per", "times") if key in source[width]}
        assert all_units["compound"][width] == expected_compounds
    symbols = data["number_symbols.json"][locale]
    assert record["timeSeparators"] == {nu: value["timeSeparator"] for nu, value in symbols.items() if "timeSeparator" in value}


def compare_calendar_records(archive, data):
    actual = data["calendar_date_patterns.json"]
    seen = set()
    patterns = 0
    for filename in archive.namelist():
        if "/main/" not in filename or not ((filename.startswith("cldr-cal-") and filename.endswith(".json")) or (filename.startswith("cldr-dates-full/") and filename.endswith("/ca-gregorian.json"))):
            continue
        locale = filename.split("/")[-2]
        calendars = load(archive, filename, locale)["dates"]["calendars"]
        assert len(calendars) == 1
        source_calendar, record = next(iter(calendars.items()))
        calendar = {"gregorian": "gregory", "ethiopic-amete-alem": "ethioaa"}.get(source_calendar, source_calendar)
        extracted = actual[locale][calendar]
        assert extracted["sourceCalendar"] == source_calendar
        assert extracted["availableFormats"] == record["dateTimeFormats"]["availableFormats"], (locale, calendar)
        assert (locale, calendar) not in seen
        seen.add((locale, calendar))
        patterns += len(extracted["availableFormats"])
    assert seen == {(locale, calendar) for locale, calendars in actual.items() for calendar in calendars}
    assert len(seen) == 739 * 17 and patterns == 575920
    return len(seen), patterns



def compare_calendar_interval_records(archive, data):
    # Independent traversal and reconstruction: do not call the extractor.
    actual = data["calendar_interval_patterns.json"]
    seen = set()
    patterns = skeletons = 0
    for filename in archive.namelist():
        if "/main/" not in filename or not filename.endswith(".json"):
            continue
        if not (filename.startswith("cldr-cal-") or
                filename.startswith("cldr-dates-full/") and filename.endswith("/ca-gregorian.json")):
            continue
        locale = filename.split("/")[-2]
        calendars = load(archive, filename, locale)["dates"]["calendars"]
        assert len(calendars) == 1
        source_calendar, source = next(iter(calendars.items()))
        calendar = {"gregorian": "gregory", "ethiopic-amete-alem": "ethioaa"}.get(source_calendar, source_calendar)
        record = actual[locale][calendar]
        expected = source["dateTimeFormats"]["intervalFormats"]
        assert record["sourceCalendar"] == source_calendar
        assert {"intervalFormatFallback": record["fallback"], **record["patterns"]} == expected
        assert (locale, calendar) not in seen
        seen.add((locale, calendar))
        skeletons += len(record["patterns"])
        patterns += sum(len(rows) for rows in record["patterns"].values())
    assert seen == {(locale, calendar) for locale, calendars in actual.items() for calendar in calendars}
    assert len(actual) == 739 and (len(seen), skeletons, patterns) == (12563, 393932, 911403)
    return {"calendarIntervalPayloadsCompared": len(seen),
            "calendarIntervalSkeletonsCompared": skeletons,
            "calendarIntervalRawPatternsCompared": patterns}

def compare_compact_records(archive, locale, data):
    numbers = load(archive, f"cldr-numbers-full/main/{locale}/numbers.json", locale)["numbers"]
    expected = {}
    for field, record in numbers.items():
        if field.startswith("decimalFormats-numberSystem-"):
            family, nu = "decimal", field.removeprefix("decimalFormats-numberSystem-")
            widths, pattern_set = ("short", "long"), "decimalFormat"
        elif field.startswith("currencyFormats-numberSystem-"):
            family, nu = "currency", field.removeprefix("currencyFormats-numberSystem-")
            widths, pattern_set = ("short",), "standard"
        else:
            continue
        for width in widths:
            rows = record.get(width, {}).get(pattern_set)
            if rows:
                expected[f"{family}-{width}-{nu}"] = (field, width, pattern_set, rows)
    actual = data["compact_patterns.json"][locale]
    assert set(actual) == set(expected), locale
    count = 0
    for attributes, (field, width, pattern_set, rows) in expected.items():
        record = actual[attributes]
        assert (record["sourceField"], record["sourceWidth"], record["sourcePatternSet"]) == (field, width, pattern_set)
        reconstructed = {}
        for variant, suffix in (("normal", ""), ("alphaNextToNumber", "-alt-alphaNextToNumber")):
            for threshold, categories in record[variant].items():
                for category, pattern in categories.items():
                    reconstructed[f"{threshold}-count-{category}{suffix}"] = pattern
        assert reconstructed == rows, (locale, attributes)
        count += len(rows)
    return len(actual), count


def compare_number_range_records(archive, data, root_content):
    actual = data["number_range_patterns.json"]
    source = json.loads(archive.read("cldr-core/supplemental/pluralRanges.json"))["supplemental"]["plurals"]
    expected_rules = {}
    for language, rules in source.items():
        rows = {}
        for key, result in rules.items():
            start, end = key.removeprefix("pluralRange-start-").split("-end-")
            rows.setdefault(start, {})[end] = result
        expected_rules[language] = rows
    assert actual["sourcePluralRanges"] == expected_rules
    count = locales = 0
    for filename in archive.namelist():
        if not filename.startswith("cldr-numbers-full/main/") or not filename.endswith("/numbers.json"):
            continue
        locale = filename.split("/")[-2]
        numbers = load(archive, filename, locale)["numbers"]
        expected = {key[len("miscPatterns-numberSystem-"):]: {field: value[field] for field in ("range", "approximately")} for key, value in numbers.items() if key.startswith("miscPatterns-numberSystem-")}
        record = actual["locales"][locale]
        assert record["patterns"] == expected, locale
        assert record["defaultNumberingSystem"] == numbers["defaultNumberingSystem"]
        ancestors = ["-".join(locale.split("-")[:length]) for length in range(len(locale.split("-")), 0, -1)]
        inherited = next((parent for parent in ancestors if parent in expected_rules), None)
        assert record["pluralRangesLocale"] == inherited, locale
        assert record["pluralRanges"] == expected_rules.get(inherited, {}), locale
        count += len(expected)
        locales += 1
    elements = {element.attrib["numberSystem"]: element for element in ET.fromstring(root_content).find("numbers").findall("miscPatterns")}
    assert set(actual["root"]) == set(elements)
    def tree(element):
        record = {"tag": element.tag, "attributes": dict(element.attrib)}
        if list(element):
            record["children"] = [tree(child) for child in element]
        elif element.text is not None:
            record["value"] = element.text
        return record
    for nu, element in elements.items():
        record = actual["root"][nu]
        assert record["source"] == tree(element), nu
        alias = element.find("alias")
        if alias is None:
            expected = {child.attrib["type"]: child.text for child in element if child.attrib["type"] in ("range", "approximately")}
            assert record["localeAliasTarget"] is None
        else:
            # The pinned input has exactly this same-locale Latin alias for all
            # non-Latin numeric systems, not a global English language fallback.
            assert alias.attrib == {"source": "locale", "path": "../miscPatterns[@numberSystem='latn']"}
            assert record["localeAliasTarget"] == "latn"
            expected = {child.attrib["type"]: child.text for child in elements["latn"] if child.attrib["type"] in ("range", "approximately")}
        assert record["resolvedPatterns"] == expected
    pairs = sum(len(rules) for rules in source.values())
    assert locales == 739 and count == 881 and len(source) == 91 and pairs == 441 and len(elements) == 77
    assert actual["locales"]["und"]["patterns"]["latn"] == actual["root"]["latn"]["resolvedPatterns"]
    return {"miscPatternSourcePayloadsCompared": count, "pluralRangeSourcePairsCompared": pairs, "numberRangeTypedPayloadsExpected": locales * (len(elements) + 1)}


def verify(source, output, compare=None, root_xml=None, legacy=None):
    assert hashlib.sha256(source.read_bytes()).hexdigest() == SOURCE_SHA256
    provenance = json.loads((output / "provenance.json").read_bytes())
    assert provenance["source"]["url"] == SOURCE_URL and provenance["source"]["commit"] == CLDR_COMMIT and provenance["source"]["tag"] == CLDR_TAG
    data = {}
    for filename, info in provenance["outputFiles"].items():
        payload = (output / filename).read_bytes()
        assert len(payload) == info["bytes"] and hashlib.sha256(payload).hexdigest() == info["sha256"], filename
        data[filename] = json.loads(payload)
    locale_count = currency_count = compact_count = compact_patterns = 0
    root_content = root_xml.read_bytes()
    assert hashlib.sha256(root_content).hexdigest() == ROOT_SHA256
    with zipfile.ZipFile(source) as archive:
        for filename, info in provenance["inputFiles"].items():
            content = archive.read(filename)
            assert len(content) == info["bytes"] and hashlib.sha256(content).hexdigest() == info["sha256"], filename
        for filename in archive.namelist():
            if not filename.startswith("cldr-numbers-full/main/") or not filename.endswith("/numbers.json"):
                continue
            locale = compare_number_records(archive, filename, data)
            currency_count += compare_currency_records(archive, locale, data)
            compare_duration_records(archive, locale, data)
            payloads, patterns = compare_compact_records(archive, locale, data)
            compact_count += payloads
            compact_patterns += patterns
            locale_count += 1
        source_fractions = json.loads(archive.read("cldr-core/supplemental/currencyData.json"))["supplemental"]["currencyData"]["fractions"]
        calendar_count, calendar_patterns = compare_calendar_records(archive, data)
        range_counts = compare_number_range_records(archive, data, root_content)
        interval_counts = compare_calendar_interval_records(archive, data)
        native_name_counts = compare_native_names(archive, data)
        digits = data["currency_digits.json"]
        assert set(digits["currencies"]) == set(source_fractions)
        assert digits["defaultDigits"] == int(source_fractions["DEFAULT"]["_digits"])
        for code, source_record in source_fractions.items():
            record = digits["currencies"][code]
            for key, value in source_record.items():
                assert record[key.removeprefix("_")] == int(value), (code, key)
    assert locale_count == 739 and currency_count == 155792
    assert compact_count == 2606 and compact_patterns == 59376
    assert provenance["rootSource"]["commit"] == ROOT_COMMIT and provenance["rootSource"]["url"] == ROOT_URL
    root_numbers = ET.fromstring(root_content).find("numbers")
    root_symbols = data["root_number_symbols.json"]["numberingSystemSymbols"]
    # Independently verify the preserved XML trees, including alias paths and ordering.
    def tree(element):
        result = {"tag": element.tag, "attributes": dict(element.attrib)}
        if list(element):
            result["children"] = [tree(child) for child in element]
        elif element.text is not None:
            result["value"] = element.text
        return result
    for kind in ("decimal", "percent", "scientific"):
        expected = {element.attrib.get("numberSystem", "default"): tree(element) for element in root_numbers.findall(kind + "Formats")}
        assert data["root_number_symbols.json"]["numberFormatsSource"][kind] == expected
    expected_currency = {element.attrib.get("numberSystem", "default"): tree(element) for element in root_numbers.findall("currencyFormats")}
    assert data["root_number_symbols.json"]["currencyFormatsSource"] == expected_currency
    assert set(root_symbols) == {element.attrib["numberSystem"] for element in root_numbers.findall("symbols") if "numberSystem" in element.attrib}
    for element in root_numbers.findall("symbols"):
        if "numberSystem" not in element.attrib:
            continue
        nu = element.attrib["numberSystem"]
        record = root_symbols[nu]
        if element.find("alias") is None:
            for child in element:
                key = child.tag + ("-alt-" + child.attrib["alt"] if "alt" in child.attrib else "")
                assert record["resolvedSymbols"][key] == child.text
        else:
            assert record["resolvedSymbols"] == root_symbols["latn"]["resolvedSymbols"]
    assert root_symbols["arabext"]["resolvedSymbols"]["timeSeparator"] == "٫"
    legacy_files = 0
    if legacy:
        old = json.loads((legacy / "provenance.json").read_bytes())
        for name, info in old["outputFiles"].items():
            original = (legacy / name).read_bytes()
            assert hashlib.sha256(original).hexdigest() == info["sha256"]
            assert original == (output / name).read_bytes(), ("legacy data changed", name)
            legacy_files += 1
    repeated = 0
    if compare:
        assert sorted(path.name for path in output.iterdir()) == sorted(path.name for path in compare.iterdir())
        for path in output.iterdir():
            assert path.read_bytes() == (compare / path.name).read_bytes(), path.name
            repeated += 1
    print(json.dumps({"result": "PASS", "sourceLocalesCompared": locale_count, "currencyRecordsCompared": currency_count, "calendarPayloadsCompared": calendar_count, "availableFormatPatternsCompared": calendar_patterns, "compactPayloadsCompared": compact_count, "compactPatternsCompared": compact_patterns, **range_counts, **interval_counts, **native_name_counts, "legacyDatasetFilesByteIdentical": legacy_files, "sourceInputFilesHashed": len(provenance["inputFiles"]), "byteIdenticalRepeatedFiles": repeated}, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-zip", type=pathlib.Path, required=True)
    parser.add_argument("--source-root-xml", type=pathlib.Path, required=True)
    parser.add_argument("--output-dir", type=pathlib.Path, required=True)
    parser.add_argument("--compare-dir", type=pathlib.Path)
    parser.add_argument("--legacy-dir", type=pathlib.Path, help="verified prior typed14 generation; all original dataset bytes must match")
    args = parser.parse_args()
    verify(args.source_zip, args.output_dir, args.compare_dir, args.source_root_xml, args.legacy_dir)
