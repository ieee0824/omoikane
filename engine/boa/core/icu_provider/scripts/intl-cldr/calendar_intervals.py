"""Preserve complete pinned calendar interval source records, without runtime policy."""
from calendar_patterns import CALENDAR_ALIASES, calendar_source_files


def extract_calendar_intervals(archive, read):
    result = {}
    for filename in calendar_source_files(archive):
        locale = filename.split("/")[-2]
        document = read(filename)
        if set(document["main"]) != {locale}:
            raise ValueError(f"unexpected interval locale: {filename}")
        calendars = document["main"][locale]["dates"]["calendars"]
        if len(calendars) != 1:
            raise ValueError(f"unexpected calendar source: {filename}")
        source_calendar, calendar = next(iter(calendars.items()))
        raw = calendar["dateTimeFormats"]["intervalFormats"]
        identifier = CALENDAR_ALIASES.get(source_calendar, source_calendar)
        record = {
            "sourceCalendar": source_calendar,
            "fallback": raw["intervalFormatFallback"],
            "patterns": {key: value for key, value in raw.items()
                         if key != "intervalFormatFallback"},
        }
        if identifier in result.setdefault(locale, {}):
            raise ValueError(f"duplicate interval locale/calendar: {locale}/{identifier}")
        result[locale][identifier] = record
    return result


def validate_calendar_intervals(data, calendar_patterns):
    if set(data) != set(calendar_patterns) or len(data) != 739:
        raise ValueError("interval locale coverage differs from the full calendar data")
    payloads = skeletons = patterns = 0
    for locale, calendars in data.items():
        if set(calendars) != set(calendar_patterns[locale]) or len(calendars) != 17:
            raise ValueError(f"interval calendar coverage differs: {locale}")
        for calendar, record in calendars.items():
            if record["sourceCalendar"] != calendar_patterns[locale][calendar]["sourceCalendar"]:
                raise ValueError("interval source-calendar identity changed")
            fallback = record["fallback"]
            if not isinstance(fallback, str) or fallback.count("{0}") != 1 or fallback.count("{1}") != 1:
                raise ValueError(f"invalid interval fallback: {locale}/{calendar}")
            if not record["patterns"]:
                raise ValueError(f"missing interval records: {locale}/{calendar}")
            for skeleton, differences in record["patterns"].items():
                if not skeleton or not isinstance(differences, dict) or not differences:
                    raise ValueError("invalid interval skeleton")
                if any(not key or not isinstance(value, str) or not value
                       for key, value in differences.items()):
                    raise ValueError("invalid raw interval pattern")
                patterns += len(differences)
            skeletons += len(record["patterns"])
            payloads += 1
    if (payloads, skeletons, patterns) != (12563, 393932, 911403):
        raise ValueError("pinned CLDR47 full interval inventory changed")
    return {"calendarIntervalPayloads": payloads,
            "calendarIntervalSkeletons": skeletons,
            "calendarIntervalRawPatterns": patterns}
