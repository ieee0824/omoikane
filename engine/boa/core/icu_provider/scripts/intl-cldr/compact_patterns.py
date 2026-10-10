"""Preserve CLDR47 compact thresholds, plural counts and raw pattern variants."""
import collections
import re

KEY = re.compile(r"([1-9][0-9]*)-count-(zero|one|two|few|many|other|0|1)(-alt-alphaNextToNumber)?")


def split_key(key):
    match = KEY.fullmatch(key)
    if match is None:
        raise ValueError(f"unexpected compact pattern key: {key}")
    threshold, category, alpha = match.groups()
    if threshold != "1" + "0" * (len(threshold) - 1) or int(threshold) >= 2**64:
        raise ValueError(f"compact threshold is not a u64 power of ten: {threshold}")
    return threshold, category, alpha is not None


def has_unquoted_zero(pattern):
    quoted = False
    found = False
    index = 0
    while index < len(pattern):
        if pattern[index] == "'":
            if index + 1 < len(pattern) and pattern[index + 1] == "'":
                index += 2
                continue
            quoted = not quoted
        elif pattern[index] == "0" and not quoted:
            found = True
        index += 1
    if quoted:
        raise ValueError("unbalanced quote in compact pattern")
    return found


def extract_compact_patterns(numbers):
    result = {}
    for field, data in numbers.items():
        if field.startswith("decimalFormats-numberSystem-"):
            family, nu = "decimal", field.removeprefix("decimalFormats-numberSystem-")
            widths, pattern_set = ("short", "long"), "decimalFormat"
        elif field.startswith("currencyFormats-numberSystem-"):
            family, nu = "currency", field.removeprefix("currencyFormats-numberSystem-")
            widths, pattern_set = ("short",), "standard"
        else:
            continue
        for width in widths:
            rows = data.get(width, {}).get(pattern_set)
            if not rows:
                continue
            record = {"sourceField": field, "sourceWidth": width, "sourcePatternSet": pattern_set, "normal": {}, "alphaNextToNumber": {}}
            for key, pattern in rows.items():
                threshold, category, alpha = split_key(key)
                if not isinstance(pattern, str) or not pattern:
                    raise ValueError(f"compact pattern is not a nonempty string: {field}/{key}")
                has_unquoted_zero(pattern)
                table = record["alphaNextToNumber" if alpha else "normal"]
                table.setdefault(threshold, {})[category] = pattern
            result[f"{family}-{width}-{nu}"] = record
    return result


def validate_compact_patterns(data, locales):
    if set(data) != set(locales):
        raise ValueError("compact locale coverage differs from number locale coverage")
    counts = collections.Counter()
    for locale, formats in data.items():
        for attributes, record in formats.items():
            family = attributes.rsplit("-", 1)[0]
            counts[f"compact-{family}-payloads"] += 1
            normal = record["normal"]
            if not normal or any("other" not in categories for categories in normal.values()):
                raise ValueError(f"compact normal threshold lacks other pattern: {locale}/{attributes}")
            if not set(record["alphaNextToNumber"]) <= set(normal):
                raise ValueError(f"compact alpha threshold lacks normal threshold: {locale}/{attributes}")
            for variant in ("normal", "alphaNextToNumber"):
                for threshold, categories in record[variant].items():
                    for category, pattern in categories.items():
                        split_key(f"{threshold}-count-{category}")
                        counts[f"compact-{variant}-rows"] += 1
                        counts["compactExplicitOnePatterns"] += category == "1"
                        counts["compactRawZeroPatterns"] += pattern == "0"
                        counts["compactPatternsWithoutNumber"] += not has_unquoted_zero(pattern)
                        counts["compactAlphaOnlyPluralPairs"] += variant == "alphaNextToNumber" and category not in normal[threshold]
    expected = {"compact-decimal-short-payloads": 881, "compact-decimal-long-payloads": 881, "compact-currency-short-payloads": 844, "compact-normal-rows": 54442, "compact-alphaNextToNumber-rows": 4934, "compactRawZeroPatterns": 240, "compactPatternsWithoutNumber": 52, "compactExplicitOnePatterns": 47, "compactAlphaOnlyPluralPairs": 13}
    if counts != expected:
        raise ValueError(f"compact CLDR47 coverage changed: {dict(counts)}")
    return dict(counts)
