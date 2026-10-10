"""Preserve pinned CLDR calendar availableFormats without selecting runtime policy."""

# CLDR source calendar IDs differ from Unicode ca identifiers for these aliases.
CALENDAR_ALIASES = {"gregorian": "gregory", "ethiopic-amete-alem": "ethioaa"}


def calendar_source_files(archive):
    return sorted(name for name in archive.namelist() if "/main/" in name and (
        (name.startswith("cldr-cal-") and name.endswith(".json")) or
        (name.startswith("cldr-dates-full/") and name.endswith("/ca-gregorian.json"))))


def extract_calendar_patterns(archive, read):
    result = {}
    for filename in calendar_source_files(archive):
        locale = filename.split("/")[-2]
        document = read(filename)
        if set(document["main"]) != {locale}:
            raise ValueError(f"unexpected calendar locale: {filename}")
        calendars = document["main"][locale]["dates"]["calendars"]
        if len(calendars) != 1:
            raise ValueError(f"unexpected calendar file contents: {filename}")
        source_calendar, calendar = next(iter(calendars.items()))
        identifier = CALENDAR_ALIASES.get(source_calendar, source_calendar)
        available = calendar["dateTimeFormats"]["availableFormats"]
        if not available or any(not isinstance(value, str) or not value for value in available.values()):
            raise ValueError(f"available format is not a raw pattern: {filename}")
        record = {"sourceCalendar": source_calendar, "availableFormats": available}
        if identifier in result.setdefault(locale, {}):
            raise ValueError(f"duplicate locale/calendar: {locale}/{identifier}")
        result[locale][identifier] = record
    return result


def validate_calendar_patterns(data, locales):
    if set(data) != set(locales):
        raise ValueError("calendar locale coverage differs from number locale coverage")
    expected = {"buddhist", "chinese", "coptic", "dangi", "ethioaa", "ethiopic", "gregory", "hebrew", "indian", "islamic", "islamic-civil", "islamic-rgsa", "islamic-tbla", "islamic-umalqura", "japanese", "persian", "roc"}
    for locale, calendars in data.items():
        if set(calendars) != expected:
            raise ValueError(f"calendar coverage differs from CLDR47: {locale}")
    if data["zh"]["chinese"]["availableFormats"]["yyyyMMMMd"] != "rU年MMMMd":
        raise ValueError("Chinese wide-month cyclic year pattern was not preserved")
    return {"calendarPatternPayloads": sum(map(len, data.values())), "availableFormatPatterns": sum(len(record["availableFormats"]) for calendars in data.values() for record in calendars.values())}
