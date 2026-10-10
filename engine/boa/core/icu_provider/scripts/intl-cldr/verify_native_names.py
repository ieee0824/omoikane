"""Independently compare every emitted native name label to pinned CLDR JSON."""
import json


def source_labels(values, kind):
    result = {}
    for code, value in values.items():
        if "-alt-" not in code:
            result[f"{kind}/long/{code}"] = value
        for width in ("short", "narrow"):
            if code.endswith(f"-alt-{width}"):
                result[f"{kind}/{width}/{code[:-(5 + len(width))]}"] = value
    return result


def compare_native_names(archive, data):
    locales = set(data["number_symbols.json"])
    relative = data["relative_time_patterns.json"]
    names = data["display_names.json"]
    assert len(locales) == 739 and set(relative) == set(names) == locales
    relative_count = display_count = 0
    units = ("year", "quarter", "month", "week", "day", "hour", "minute", "second")
    for locale in sorted(locales):
        def read(path):
            return json.loads(archive.read(path))["main"][locale]
        dates = read(f"cldr-dates-full/main/{locale}/dateFields.json")["dates"]["fields"]
        expected = {}
        for unit in units:
            for style in ("long", "short", "narrow"):
                field = dates.get(unit + ("" if style == "long" else "-" + style), {})
                for key, value in field.items():
                    if key.startswith("relative-type-"):
                        expected[f"{unit}/{style}/auto/{key[14:]}"] = value
                for tense in ("past", "future"):
                    for key, value in field.get("relativeTime-type-" + tense, {}).items():
                        if key.startswith("relativeTimePattern-count-"):
                            expected[f"{unit}/{style}/{tense}/{key[26:]}"] = value
        assert relative[locale] == expected, (locale, "relative time source mismatch")
        relative_count += len(expected)
        expected = {}
        for filename, kind in (("languages", "language"), ("scripts", "script"),
                               ("territories", "region"), ("variants", "variant")):
            path = f"cldr-localenames-full/main/{locale}/{filename}.json"
            if path not in archive.NameToInfo:
                continue
            values = read(path)["localeDisplayNames"][filename]
            if kind == "script":
                expected.update(source_labels(values, kind))
                expected.update(source_labels(values, "scriptContext"))
                for key, value in values.items():
                    if key.endswith("-alt-stand-alone"):
                        code = key[:-16]
                        contextual = values.get(code)
                        if not isinstance(contextual, str):
                            continue
                        short_key = f"script/short/{code}"
                        expected.setdefault(short_key, contextual)
                        expected[f"script/long/{code}"] = value
            else:
                expected.update(source_labels(values, kind))
        source = read(f"cldr-localenames-full/main/{locale}/localeDisplayNames.json")["localeDisplayNames"]
        expected.update({f"pattern/{key}": value for key, value in source["localeDisplayPattern"].items()})
        calendars = source.get("types", {}).get("calendar", {})
        calendars = {{"gregorian": "gregory", "ethiopic-amete-alem": "ethioaa"}.get(key, key): value
                     for key, value in calendars.items()}
        expected.update(source_labels(calendars, "calendar"))
        fields = {"era": "era", "year": "year", "quarter": "quarter", "month": "month",
                  "weekOfYear": "week", "weekday": "weekday", "day": "day", "dayPeriod": "dayperiod",
                  "hour": "hour", "minute": "minute", "second": "second", "timeZoneName": "zone"}
        for code, field in fields.items():
            for style in ("long", "short", "narrow"):
                key = field + ("" if style == "long" else "-" + style)
                value = dates.get(key, {}).get("displayName")
                if value is not None:
                    expected[f"dateTimeField/{style}/{code}"] = value
        assert names[locale] == expected, (locale, "display name source mismatch")
        display_count += len(expected)
    return {"relativeTimeSourceRecordsCompared": relative_count,
            "displayNamesSourceRecordsCompared": display_count}
