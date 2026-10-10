"""Extract exact CLDR fields for RelativeTimeFormat and DisplayNames."""

UNITS = ("second", "minute", "hour", "day", "week", "month", "quarter", "year")
STYLES = ("long", "short", "narrow")
DATE_FIELDS = {"era": "era", "year": "year", "quarter": "quarter", "month": "month",
               "weekOfYear": "week", "weekday": "weekday", "day": "day",
               "dayPeriod": "dayperiod", "hour": "hour", "minute": "minute",
               "second": "second", "timeZoneName": "zone"}


def fields(archive, locale, read):
    document = read(f"cldr-dates-full/main/{locale}/dateFields.json")
    return document["main"][locale]["dates"]["fields"]


def relative_records(source):
    records = {}
    for unit in UNITS:
        for style in STYLES:
            key = unit if style == "long" else f"{unit}-{style}"
            if key not in source:
                continue
            field = source[key]
            for name, value in field.items():
                if name.startswith("relative-type-"):
                    records[f"{unit}/{style}/auto/{name.removeprefix('relative-type-')}"] = value
            for tense in ("past", "future"):
                for count, pattern in field.get(f"relativeTime-type-{tense}", {}).items():
                    if count.startswith("relativeTimePattern-count-"):
                        category = count.removeprefix("relativeTimePattern-count-")
                        if pattern.count("{0}") > 1:
                            raise ValueError(f"relative pattern repeats its number: {key}/{category}")
                        records[f"{unit}/{style}/{tense}/{category}"] = pattern
    for unit in UNITS:
        for tense in ("past", "future"):
            if f"{unit}/long/{tense}/other" not in records:
                raise ValueError(f"missing relative other pattern: {unit}/{tense}")
    return records


def labels(records, kind, values):
    for code, value in values.items():
        if "-alt-" not in code and isinstance(value, str):
            records[f"{kind}/long/{code}"] = value
    for style in ("short", "narrow"):
        for code, value in values.items():
            suffix = f"-alt-{style}"
            if code.endswith(suffix) and isinstance(value, str):
                records[f"{kind}/{style}/{code.removesuffix(suffix)}"] = value


def script_labels(records, values):
    labels(records, "script", values)
    labels(records, "scriptContext", values)
    for code, value in values.items():
        if not code.endswith("-alt-stand-alone") or not isinstance(value, str):
            continue
        base = code.removesuffix("-alt-stand-alone")
        contextual = values.get(base)
        if not isinstance(contextual, str):
            continue
        # DisplayNames already falls back from narrow to short. Keep one
        # contextual record unless CLDR supplies an explicit narrow label.
        records.setdefault(f"script/short/{base}", contextual)
        records[f"script/long/{base}"] = value


def display_records(archive, locale, date_fields, read):
    records = {}
    for filename, kind, member in (("languages", "language", "languages"),
                                    ("scripts", "script", "scripts"),
                                    ("territories", "region", "territories"),
                                    ("variants", "variant", "variants")):
        path = f"cldr-localenames-full/main/{locale}/{filename}.json"
        # The official resolved JSON omits files containing no translated names.
        # These records stay absent, including CLDR root (und), not English.
        if path in archive.NameToInfo:
            source = read(path)
            values = source["main"][locale]["localeDisplayNames"][member]
            if kind == "script":
                script_labels(records, values)
            else:
                labels(records, kind, values)
    source = read(f"cldr-localenames-full/main/{locale}/localeDisplayNames.json")
    names = source["main"][locale]["localeDisplayNames"]
    records.update({f"pattern/{key}": value for key, value in names["localeDisplayPattern"].items()})
    calendars = {({"gregorian": "gregory", "ethiopic-amete-alem": "ethioaa"}.get(code, code)): value
                 for code, value in names.get("types", {}).get("calendar", {}).items()}
    labels(records, "calendar", calendars)
    for code, source_field in DATE_FIELDS.items():
        for style in STYLES:
            key = source_field if style == "long" else f"{source_field}-{style}"
            name = date_fields.get(key, {}).get("displayName")
            if name is not None:
                records[f"dateTimeField/{style}/{code}"] = name
    return records


def extract_relative_display_names(archive, locales, read):
    relative, names = {}, {}
    for locale in sorted(locales):
        source = fields(archive, locale, read)
        relative[locale] = relative_records(source)
        names[locale] = display_records(archive, locale, source, read)
    if len(relative) != 739 or set(relative) != set(names):
        raise ValueError("all 739 CLDR locales are required for both new services")
    return {"relative_time_patterns.json": relative, "display_names.json": names}


def validate_relative_display_names(datasets, locales):
    counts = {}
    for dataset, field in (("relative_time_patterns.json", "relativeTimeRecords"),
                            ("display_names.json", "displayNamesRecords")):
        records = datasets[dataset]
        if set(records) != set(locales):
            raise ValueError(f"locale closure differs for {dataset}")
        if any(not isinstance(value, str) for row in records.values() for value in row.values()):
            raise ValueError(f"nonstring CLDR label: {dataset}")
        counts[field] = sum(len(row) for row in records.values())
    return counts
