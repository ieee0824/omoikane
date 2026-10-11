"""Preserve pinned CLDR styles and flexible day-period names and source rules."""
from calendar_patterns import CALENDAR_ALIASES, calendar_source_files

DAY_PERIODS_FILE = "cldr-core/supplemental/dayPeriods.json"


def rule_locale(locale, rules):
    """Apply ICU's day-period rule lookup by removing locale subtags."""
    while locale not in rules:
        if locale == "und":
            raise ValueError("missing fixed AM/PM fallback rules")
        locale = locale.rsplit("-", 1)[0] if "-" in locale else "und"
    return locale


def extract_datetime_extras(archive, read):
    styles, periods = {}, {}
    source_rules = read(DAY_PERIODS_FILE)["supplemental"]["dayPeriodRuleSet"]
    for filename in calendar_source_files(archive):
        locale = filename.split("/")[-2]
        calendars = read(filename)["main"][locale]["dates"]["calendars"]
        source_calendar, calendar = next(iter(calendars.items()))
        identifier = CALENDAR_ALIASES.get(source_calendar, source_calendar)
        styles.setdefault(locale, {})[identifier] = {
            "sourceCalendar": source_calendar,
            "patterns": {
                "date": calendar["dateFormats"],
                "time": calendar["timeFormats"],
                "glue": {key: value for key, value in calendar["dateTimeFormats"].items()
                         if key in ("full", "long", "medium", "short")},
                "atTime": calendar.get("dateTimeFormats-atTime", {}),
                "monthNumeric": calendar.get("monthPatterns", {}).get("numeric", {}).get("all", {}),
                "monthNarrow": calendar["months"]["format"]["narrow"],
                "monthNarrowPattern": calendar.get("monthPatterns", {}).get("format", {}).get("narrow", {}),
            },
        }
        if identifier == "gregory":
            inherited = rule_locale(locale, source_rules)
            periods[locale] = {
                "names": calendar["dayPeriods"]["format"],
                "rules": source_rules[inherited],
                "sourceRuleLocale": inherited,
            }
    return {"datetime_style_patterns.json": styles, "day_periods.json": periods}


def validate_datetime_extras(datasets, locales):
    styles, periods = datasets["datetime_style_patterns.json"], datasets["day_periods.json"]
    if set(styles) != set(locales) or set(periods) != set(locales):
        raise ValueError("date/time extra locale coverage differs from CLDR source")
    for locale, calendars in styles.items():
        for record in calendars.values():
            for family in ("date", "time", "glue"):
                if not set(("full", "long", "medium", "short")) <= record["patterns"][family].keys():
                    raise ValueError(f"missing style patterns: {locale}/{family}")
                for value in record["patterns"][family].values():
                    if isinstance(value, dict):
                        if set(value) != {"_value", "_numbers"} or not isinstance(value["_numbers"], str) or not value["_numbers"]:
                            raise ValueError(f"unrecognized style attributes: {locale}/{family}")
                        value = value["_value"]
                    if not isinstance(value, str) or not value:
                        raise ValueError(f"style pattern is not raw text: {locale}/{family}")
        for width in ("wide", "abbreviated", "narrow"):
            if not all(key in periods[locale]["names"][width] for key in ("am", "pm")):
                raise ValueError(f"missing fixed period fallback: {locale}/{width}")
        for period, rule in periods[locale]["rules"].items():
            if set(rule) not in ({"_at"}, {"_from", "_before"}, {"_after", "_before"}, {"_from", "_to"}):
                raise ValueError(f"unrecognized day-period bounds: {locale}/{period}")
            for value in rule.values():
                parts = value.split(":")
                if len(parts) != 2 or not all(part.isdigit() for part in parts):
                    raise ValueError(f"invalid day-period time: {locale}/{period}")
                hour, minute = map(int, parts)
                if not 0 <= hour <= 24 or not 0 <= minute < 60 or (hour == 24 and minute):
                    raise ValueError(f"out-of-range day-period time: {locale}/{period}")
    return {"dateTimeStylePayloads": sum(map(len, styles.values())),
            "dayPeriodPayloads": len(periods),
            "dayPeriodRules": sum(len(record["rules"]) for record in periods.values())}
