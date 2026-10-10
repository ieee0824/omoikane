"""Preserve CLDR miscPatterns, same-locale numeric aliases and plural ranges."""
import hashlib
import re
import xml.etree.ElementTree as ET

from root_symbols import ROOT_SHA256, element_record

PLURAL_RANGES_FILE = "cldr-core/supplemental/pluralRanges.json"
CARDINALS = ("zero", "one", "two", "few", "many", "other")
FIELDS = ("range", "approximately")


def validate_pattern(pattern, field):
    """Validate structural placeholders without rewriting literal or quote data."""
    if not isinstance(pattern, str):
        raise ValueError(f"non-string number range pattern: {field}")
    expected = ("0", "1") if field == "range" else ("0",)
    if sorted(re.findall(r"\{([^{}]*)\}", pattern)) != sorted(expected):
        raise ValueError(f"invalid {field} placeholders: {pattern!r}")
    if "{" in re.sub(r"\{[01]\}", "", pattern) or "}" in re.sub(r"\{[01]\}", "", pattern):
        raise ValueError(f"unmatched pattern brace: {pattern!r}")


def extract_number_ranges(numbers):
    patterns = {}
    for key, value in numbers.items():
        if not key.startswith("miscPatterns-numberSystem-"):
            continue
        nu = key.removeprefix("miscPatterns-numberSystem-")
        patterns[nu] = {field: value[field] for field in FIELDS}
        for field, pattern in patterns[nu].items():
            validate_pattern(pattern, field)
    default = numbers["defaultNumberingSystem"]
    if default not in patterns or "latn" not in patterns:
        raise ValueError("locale default/Latin miscellaneous patterns are missing")
    return {"defaultNumberingSystem": default, "patterns": patterns}


def extract_plural_ranges(document):
    result = {}
    for locale, rules in document["supplemental"]["plurals"].items():
        rows = {}
        for key, category in rules.items():
            match = re.fullmatch(r"pluralRange-start-([a-z]+)-end-([a-z]+)", key)
            if not match or category not in CARDINALS or any(item not in CARDINALS for item in match.groups()):
                raise ValueError(f"invalid cardinal range rule: {locale}/{key}")
            start, end = match.groups()
            rows.setdefault(start, {})[end] = category
        result[locale] = rows
    return result


def resolve_plural_locale(locale, source):
    """Supplemental plural rules use simple language/script/region inheritance.

    Main-data nondefault-script parent overrides (for example sr-Latn -> und)
    must not discard Serbian cardinal rules. No English/root rules are invented.
    """
    candidate = locale
    while candidate:
        if candidate in source:
            return candidate
        candidate = candidate.rpartition("-")[0]
    return None


def root_alias(elements, nu, active=()):
    if nu in active:
        raise ValueError(f"cyclic miscellaneous-pattern alias: {nu}")
    alias = elements[nu].find("alias")
    if alias is None:
        return nu
    match = re.fullmatch(r"\.\./miscPatterns\[@numberSystem='([a-z0-9]+)'\]", alias.attrib["path"])
    if alias.attrib.get("source") != "locale" or not match:
        raise ValueError(f"unsupported miscellaneous-pattern alias: {alias.attrib}")
    target = match.group(1)
    if target not in elements:
        raise ValueError(f"missing miscellaneous-pattern alias target: {target}")
    return root_alias(elements, target, (*active, nu))


def extract_root_ranges(path):
    content = path.read_bytes()
    if hashlib.sha256(content).hexdigest() != ROOT_SHA256:
        raise ValueError("number-range root XML differs from pinned CLDR 47")
    numbers = ET.fromstring(content).find("numbers")
    elements = {item.attrib["numberSystem"]: item for item in numbers.findall("miscPatterns")}
    records = {}
    for nu, element in elements.items():
        target = root_alias(elements, nu)
        patterns = {item.attrib["type"]: item.text for item in elements[target].findall("pattern") if item.attrib["type"] in FIELDS}
        if set(patterns) != set(FIELDS):
            raise ValueError(f"missing root range patterns: {nu}")
        for field, pattern in patterns.items():
            validate_pattern(pattern, field)
        records[nu] = {"source": element_record(element), "resolvedPatterns": patterns, "localeAliasTarget": target if element.find("alias") is not None else None}
    return records


def assemble_number_ranges(locales, plural_source, root_path):
    for locale, record in locales.items():
        source_locale = resolve_plural_locale(locale, plural_source)
        record["pluralRangesLocale"] = source_locale
        record["pluralRanges"] = plural_source.get(source_locale, {})
    return {"locales": locales, "sourcePluralRanges": plural_source, "root": extract_root_ranges(root_path)}


def validate_number_ranges(data, expected_locales, numeric_systems):
    if set(data["locales"]) != set(expected_locales) or set(data["root"]) != set(numeric_systems):
        raise ValueError("number range locale/numbering-system coverage differs")
    source = data["sourcePluralRanges"]
    raw = sum(len(record["patterns"]) for record in data["locales"].values())
    pairs = sum(len(ends) for rules in source.values() for ends in rules.values())
    if len(source) != 91 or pairs != 441 or raw != 881:
        raise ValueError("unexpected CLDR 47 miscellaneous/plural-range coverage")
    for locale, record in data["locales"].items():
        resolved = resolve_plural_locale(locale, source)
        if record["pluralRangesLocale"] != resolved or record["pluralRanges"] != source.get(resolved, {}):
            raise ValueError(f"plural range inheritance differs: {locale}")
        for nu in numeric_systems:
            target = data["root"][nu]["localeAliasTarget"]
            if nu not in record["patterns"] and target not in record["patterns"]:
                raise ValueError(f"unresolved same-locale number range alias: {locale}/{nu}")
    return {"numberRangeSourcePayloads": raw, "pluralRangeSourceLocales": len(source), "pluralRangeSourcePairs": pairs, "numberRangeTypedPayloads": len(expected_locales) * (len(numeric_systems) + 1)}
