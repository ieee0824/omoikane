#!/usr/bin/env python3
"""Extract pinned CLDR 47 data for native Intl NumberFormat, without Cargo or repo mutation."""
import argparse
import collections
import hashlib
import json
import pathlib
import urllib.request
import zipfile
import zlib

from relative_display_names import extract_relative_display_names, validate_relative_display_names
from patterns import parse_number_pattern
from root_symbols import extract_root
from calendar_patterns import extract_calendar_patterns, validate_calendar_patterns
from calendar_intervals import extract_calendar_intervals, validate_calendar_intervals
from compact_patterns import extract_compact_patterns, validate_compact_patterns
from number_range_patterns import (PLURAL_RANGES_FILE, extract_number_ranges, extract_plural_ranges,
                                   assemble_number_ranges, validate_number_ranges)

CLDR_TAG = "47.0.0"
CLDR_COMMIT = "16f6b8578ba5fe98959034706f337674f816fc3f"
SOURCE_URL = "https://github.com/unicode-org/cldr-json/releases/download/47.0.0/cldr-47.0.0-json-full.zip"
SOURCE_SHA256 = "bbb9a9aac2dfc534bd18288678a5984023d11d22f712f3c33425f3214bd1def6"
PATTERN_FIELDS = ("standard", "standard-alphaNextToNumber", "standard-noCurrency", "accounting", "accounting-alphaNextToNumber", "accounting-noCurrency")
SYMBOL_FIELDS = ("nan", "infinity", "plusSign", "minusSign", "decimal", "group", "currencyDecimal", "currencyGroup", "percentSign", "perMille", "exponential", "approximatelySign", "timeSeparator", "timeSeparator-alt-variant", "list", "superscriptingExponent")
SUPPLEMENTAL_FILES = ("cldr-core/coverageLevels.json", "cldr-core/defaultContent.json", "cldr-core/supplemental/parentLocales.json", "cldr-core/supplemental/numberingSystems.json", "cldr-core/supplemental/currencyData.json")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def canonical_json(value):
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")


def read_json(archive, name, inputs):
    data = archive.read(name)
    inputs[name] = {"bytes": len(data), "sha256": digest(data)}
    return json.loads(data)


def locale_numbers(document, expected_locale):
    if set(document["main"]) != {expected_locale}:
        raise ValueError(f"unexpected main locale for {expected_locale}")
    return document["main"][expected_locale]["numbers"]


def extract_symbols(numbers):
    result = {}
    for key, value in numbers.items():
        if not key.startswith("symbols-numberSystem-"):
            continue
        numbering_system = key.removeprefix("symbols-numberSystem-")
        missing = set(("nan", "infinity", "plusSign", "minusSign", "decimal", "group")) - value.keys()
        if missing:
            raise ValueError(f"missing required symbols: {numbering_system} {sorted(missing)}")
        symbols = {field: value[field] for field in SYMBOL_FIELDS if field in value}
        # Both effective and source values are explicit: preserve CLDR overrides and defaults.
        symbols["effectiveCurrencyDecimal"] = value.get("currencyDecimal", value["decimal"])
        symbols["effectiveCurrencyGroup"] = value.get("currencyGroup", value["group"])
        result[numbering_system] = symbols
    return result


def extract_patterns(numbers):
    result = {}
    for key, value in numbers.items():
        if not key.startswith("currencyFormats-numberSystem-"):
            continue
        numbering_system = key.removeprefix("currencyFormats-numberSystem-")
        parsed = {field: parse_number_pattern(value[field]) for field in PATTERN_FIELDS if field in value}
        unit_patterns = {key.removeprefix("unitPattern-count-"): pattern for key, pattern in value.items() if key.startswith("unitPattern-count-")}
        if "standard" not in parsed or "accounting" not in parsed:
            raise ValueError(f"missing required currency pattern: {numbering_system}")
        result[numbering_system] = {"patterns": parsed, "unitPatterns": unit_patterns, "currencySpacing": value.get("currencySpacing", {})}
        if "currencyPatternAppendISO" in value:
            result[numbering_system]["currencyPatternAppendISO"] = value["currencyPatternAppendISO"]
    return result


def extract_number_patterns(numbers):
    result = {}
    for kind in ("decimal", "percent", "scientific"):
        prefix = kind + "Formats-numberSystem-"
        for key, value in numbers.items():
            if key.startswith(prefix):
                nu = key.removeprefix(prefix)
                result.setdefault(nu, {})[kind] = value["standard"]
    return result


def extract_currencies(currencies):
    result = {}
    for code, value in currencies.items():
        if len(code) != 3 or not code.isascii() or not code.isalpha() or code != code.upper():
            raise ValueError(f"invalid currency code: {code!r}")
        fields = ("symbol", "symbol-alt-narrow", "symbol-alt-formal", "symbol-alt-variant", "displayName", "decimal", "group")
        record = {field: value[field] for field in fields if field in value}
        record["pluralNames"] = {key.removeprefix("displayName-count-"): name for key, name in value.items() if key.startswith("displayName-count-")}
        record["effectiveSymbol"] = value.get("symbol", code)
        record["effectiveNarrowSymbol"] = value.get("symbol-alt-narrow", value.get("symbol", code))
        if "pattern" in value:
            record["patternOverride"] = parse_number_pattern(value["pattern"])
        result[code] = record
    return result


DURATION_UNITS = ("year", "month", "week", "day", "hour", "minute", "second", "millisecond", "microsecond", "nanosecond")


def extract_unit_source(archive, locale, inputs):
    name = f"cldr-units-full/main/{locale}/units.json"
    document = read_json(archive, name, inputs)
    if set(document["main"]) != {locale}:
        raise ValueError(f"unexpected unit locale: {locale}")
    units = document["main"][locale]["units"]
    all_units = {width: {unit: record for unit, record in units[width].items() if any(key.startswith("unitPattern-count-") for key in record)} for width in ("long", "short", "narrow")}
    all_units["compound"] = {width: {key: units[width][key] for key in ("per", "times") if key in units[width]} for width in ("long", "short", "narrow")}
    result = {}
    for width in ("long", "short", "narrow"):
        result[width] = {unit: units[width][f"duration-{unit}"] for unit in DURATION_UNITS}
    result["digital"] = {form: units[f"durationUnit-type-{form}"]["durationUnitPattern"] for form in ("hm", "hms", "ms")}
    return result, all_units


def extract_fractions(document):
    fractions = document["supplemental"]["currencyData"]["fractions"]
    default_digits = int(fractions["DEFAULT"]["_digits"])
    records = {}
    for code, value in fractions.items():
        record = {key.removeprefix("_"): int(number) for key, number in value.items()}
        record["effectiveDigits"] = record.get("digits", default_digits)
        records[code] = record
    return {"defaultDigits": default_digits, "currencies": records}


def extract_all(source_zip):
    inputs = {}
    symbols, patterns, number_patterns, names, locales, duration, units, compact, number_ranges = {}, {}, {}, {}, {}, {}, {}, {}, {}
    with zipfile.ZipFile(source_zip) as archive:
        files = sorted(name for name in archive.namelist() if name.startswith("cldr-numbers-full/main/") and name.endswith("/numbers.json"))
        for number_file in files:
            locale = number_file.split("/")[-2]
            numbers = locale_numbers(read_json(archive, number_file, inputs), locale)
            currency_file = number_file.removesuffix("numbers.json") + "currencies.json"
            currencies = locale_numbers(read_json(archive, currency_file, inputs), locale)["currencies"]
            symbols[locale] = extract_symbols(numbers)
            patterns[locale] = extract_patterns(numbers)
            number_patterns[locale] = extract_number_patterns(numbers)
            compact[locale] = extract_compact_patterns(numbers)
            number_ranges[locale] = extract_number_ranges(numbers)
            names[locale] = extract_currencies(currencies)
            duration[locale], units[locale] = extract_unit_source(archive, locale, inputs)
            duration[locale]["timeSeparators"] = {nu: data["timeSeparator"] for nu, data in symbols[locale].items() if "timeSeparator" in data}
            locales[locale] = {"defaultNumberingSystem": numbers["defaultNumberingSystem"], "otherNumberingSystems": numbers.get("otherNumberingSystems", {}), "minimumGroupingDigits": int(numbers["minimumGroupingDigits"])}
        supplemental = {name: read_json(archive, name, inputs) for name in SUPPLEMENTAL_FILES}
        plural_ranges = extract_plural_ranges(read_json(archive, PLURAL_RANGES_FILE, inputs))
        license_data = archive.read("cldr-core/LICENSE")
        inputs["cldr-core/LICENSE"] = {"bytes": len(license_data), "sha256": digest(license_data)}
        calendar_patterns = extract_calendar_patterns(archive, lambda name: read_json(archive, name, inputs))
        calendar_intervals = extract_calendar_intervals(archive, lambda name: read_json(archive, name, inputs))
        native_names = extract_relative_display_names(archive, locales, lambda name: read_json(archive, name, inputs))
    coverage = supplemental["cldr-core/coverageLevels.json"]["coverageLevels"]
    metadata = {"locales": locales, "coverageLevels": coverage, "directModernLocales": sorted(locale for locale, level in coverage.items() if level == "modern"), "defaultContent": supplemental["cldr-core/defaultContent.json"]["defaultContent"], "parentLocales": supplemental["cldr-core/supplemental/parentLocales.json"]["supplemental"]["parentLocales"], "numberingSystems": supplemental["cldr-core/supplemental/numberingSystems.json"]["supplemental"]["numberingSystems"]}
    datasets = {"number_symbols.json": symbols, "number_patterns.json": number_patterns, "currency_patterns.json": patterns, "currency_names.json": names, "currency_digits.json": extract_fractions(supplemental["cldr-core/supplemental/currencyData.json"]), "locale_metadata.json": metadata, "duration_unit_patterns.json": duration, "all_unit_patterns.json": units}
    datasets["calendar_date_patterns.json"] = calendar_patterns
    datasets["calendar_interval_patterns.json"] = calendar_intervals
    datasets["compact_patterns.json"] = compact
    datasets.update(native_names)
    return datasets, inputs, license_data, number_ranges, plural_ranges


def validate_datasets(datasets):
    symbols, patterns = datasets["number_symbols.json"], datasets["currency_patterns.json"]
    names, digits = datasets["currency_names.json"], datasets["currency_digits.json"]
    if len(symbols) != 739 or "und" not in symbols or set(symbols) != set(patterns) or set(symbols) != set(names):
        raise ValueError("full CLDR 47 locale coverage was not preserved")
    if digits["defaultDigits"] != 2 or any(digits["currencies"][code]["effectiveDigits"] != expected for code, expected in {"JPY": 0, "KWD": 3, "CLF": 4, "CHF": 2}.items()):
        raise ValueError("currency precision validation failed")
    counts = collections.Counter()
    for locale, systems in patterns.items():
        for numbering_system, data in systems.items():
            counts["patternPayloads"] += 1
            if numbering_system not in symbols[locale]:
                raise ValueError(f"pattern lacks matching symbols: {locale} {numbering_system}")
            for name, parsed in data["patterns"].items():
                for sign in ("positive", "negative"):
                    counts["parsedSubpatterns"] += 1
                    if sum(token["type"] == "number" for token in parsed[sign]) != 1:
                        raise ValueError(f"bad number token count: {locale} {name} {sign}")
                if parsed["explicitNegative"]:
                    counts["explicitNegativePatterns"] += 1
            for count, pattern in data["unitPatterns"].items():
                if pattern.count("{0}") != 1 or pattern.count("{1}") != 1:
                    raise ValueError(f"invalid currency unit pattern: {locale} {count}")
                counts["pluralUnitPatterns"] += 1
    counts["symbolPayloads"] = sum(map(len, symbols.values()))
    number_patterns = datasets["number_patterns.json"]
    if set(number_patterns) != set(symbols):
        raise ValueError("number pattern locale coverage differs from symbols")
    for locale, systems in number_patterns.items():
        default = datasets["locale_metadata.json"]["locales"][locale]["defaultNumberingSystem"]
        if default not in systems or set(systems[default]) != {"decimal", "percent", "scientific"}:
            raise ValueError(f"default numbering system lacks number patterns: {locale}/{default}")
        counts["numberPatternPayloads"] += len(systems)
    counts["currencyRecords"] = sum(map(len, names.values()))
    counts["precisionRecords"] = len(digits["currencies"])
    counts["locales"] = len(symbols)
    counts.update(validate_calendar_patterns(datasets["calendar_date_patterns.json"], symbols))
    counts.update(validate_calendar_intervals(datasets["calendar_interval_patterns.json"], datasets["calendar_date_patterns.json"]))
    counts.update(validate_compact_patterns(datasets["compact_patterns.json"], symbols))
    counts.update(validate_relative_display_names(datasets, symbols))
    counts["directModernLocales"] = len(datasets["locale_metadata.json"]["directModernLocales"])
    for field in ("decimal", "group", "patternOverride"):
        counts[{"decimal": "currencyDecimalOverrides", "group": "currencyGroupOverrides", "patternOverride": "currencyPatternOverrides"}[field]] = sum(field in currency for currencies in names.values() for currency in currencies.values())
    duration = datasets["duration_unit_patterns.json"]
    if set(duration) != set(symbols):
        raise ValueError("duration locale coverage differs from number locale coverage")
    for locale, data in duration.items():
        for width in ("long", "short", "narrow"):
            if set(data[width]) != set(DURATION_UNITS):
                raise ValueError(f"duration units missing for {locale}/{width}")
            for unit, record in data[width].items():
                plurals = {key: value for key, value in record.items() if key.startswith("unitPattern-count-")}
                if "unitPattern-count-other" not in plurals:
                    raise ValueError(f"missing duration other pattern: {locale}/{width}/{unit}")
                for count, pattern in plurals.items():
                    if pattern.count("{0}") > 1:
                        raise ValueError(f"multiple duration numeric placeholders: {locale}/{width}/{unit}/{count}")
                    counts["durationPluralPatterns"] += 1
                    if "{0}" not in pattern:
                        counts["durationPatternsWithoutNumber"] += 1
                counts["durationUnitRecords"] += 1
        if set(data["digital"]) != {"hm", "hms", "ms"}:
            raise ValueError(f"missing digital patterns: {locale}")
        counts["digitalPatterns"] += 3
    all_units = datasets["all_unit_patterns.json"]
    if set(all_units) != set(symbols):
        raise ValueError("all-unit locale coverage differs from number locale coverage")
    for locale, data in all_units.items():
        for width in ("long", "short", "narrow"):
            for unit, record in data[width].items():
                counts["allUnitRecords"] += 1
                if "unitPattern-count-other" not in record:
                    raise ValueError(f"missing unit other pattern: {locale}/{width}/{unit}")
            if "per" not in data["compound"][width]:
                raise ValueError(f"missing compound per pattern: {locale}/{width}")
            counts["compoundUnitWidths"] += 1
    return dict(sorted(counts.items()))


def verify_tag():
    request = urllib.request.Request(f"https://api.github.com/repos/unicode-org/cldr-json/git/ref/tags/{CLDR_TAG}", headers={"Accept": "application/vnd.github+json", "User-Agent": "omoikane-cldr-number-generator"})
    with urllib.request.urlopen(request, timeout=30) as response:
        value = json.load(response)["object"]
    if value != {"sha": CLDR_COMMIT, "type": "commit", "url": f"https://api.github.com/repos/unicode-org/cldr-json/git/commits/{CLDR_COMMIT}"}:
        raise ValueError(f"upstream tag changed: {value}")


def download_source(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    with urllib.request.urlopen(SOURCE_URL, timeout=30) as response, path.open("xb") as sink:
        while block := response.read(1024 * 1024):
            sink.write(block)


def generate(source_zip, output_dir, verify_upstream_tag=False, source_root_xml=None):
    if verify_upstream_tag:
        verify_tag()
    if not source_zip.exists():
        download_source(source_zip)
    source_bytes = source_zip.read_bytes()
    if digest(source_bytes) != SOURCE_SHA256:
        raise ValueError("source ZIP hash differs from the pinned official CLDR 47 artifact")
    datasets, inputs, license_data, number_ranges, plural_ranges = extract_all(source_zip)
    validation = validate_datasets(datasets)
    root_data, root_provenance = extract_root(source_root_xml or source_zip.parent / "root-47.xml")
    datasets["root_number_symbols.json"] = root_data
    range_data = assemble_number_ranges(number_ranges, plural_ranges, source_root_xml or source_zip.parent / "root-47.xml")
    datasets["number_range_patterns.json"] = range_data
    validation.update(validate_number_ranges(range_data, datasets["number_symbols.json"], root_data["numberingSystemSymbols"]))
    validation["rootNumericNumberingSystems"] = len(root_data["numberingSystemSymbols"])
    output_dir.mkdir(parents=True, exist_ok=False)
    files = {}
    for name, value in sorted(datasets.items()):
        data = canonical_json(value)
        (output_dir / name).write_bytes(data)
        files[name] = {"bytes": len(data), "sha256": digest(data), "exploratoryZlib9Bytes": len(zlib.compress(data, 9))}
    (output_dir / "UNICODE-LICENSE.txt").write_bytes(license_data)
    script_dir = pathlib.Path(__file__).parent
    generator_files = {name: digest((script_dir / name).read_bytes()) for name in ("generate.py", "patterns.py", "root_symbols.py", "calendar_patterns.py", "compact_patterns.py", "number_range_patterns.py", "calendar_intervals.py", "relative_display_names.py")}
    provenance = {"schemaVersion": 1, "source": {"repository": "https://github.com/unicode-org/cldr-json", "tag": CLDR_TAG, "commit": CLDR_COMMIT, "url": SOURCE_URL, "sha256": SOURCE_SHA256, "bytes": len(source_bytes)}, "rootSource": root_provenance, "generatorFiles": generator_files, "inputFiles": inputs, "outputFiles": files, "validation": validation, "sizeMeasurementScope": "canonical JSON and exploratory zlib, not runtime postcard or executable size"}
    (output_dir / "provenance.json").write_bytes(canonical_json(provenance))
    print(json.dumps({"output": str(output_dir), "validation": validation, "files": files}, ensure_ascii=False, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-zip", type=pathlib.Path, required=True, help="existing verified ZIP or a new download path")
    parser.add_argument("--source-root-xml", type=pathlib.Path, required=True, help="existing pinned CLDR47 root XML or a new download path")
    parser.add_argument("--output-dir", type=pathlib.Path, required=True, help="new directory; existing output is never overwritten")
    parser.add_argument("--verify-tag", action="store_true", help="verify upstream tag still resolves to the pinned commit")
    args = parser.parse_args()
    generate(args.source_zip, args.output_dir, args.verify_tag, args.source_root_xml)


if __name__ == "__main__":
    main()
