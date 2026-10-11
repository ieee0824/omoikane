"""Independently compare additional date/time datasets to their pinned source."""
import json


def compare_datetime_extras(archive, data):
    styles = data["datetime_style_patterns.json"]
    periods = data["day_periods.json"]
    source_rules = json.loads(archive.read("cldr-core/supplemental/dayPeriods.json"))["supplemental"]["dayPeriodRuleSet"]
    assert set(styles) == set(data["calendar_date_patterns.json"]) == set(periods)
    style_count = override_count = 0
    source_files = {}
    for name in archive.namelist():
        if "/main/" not in name or not name.endswith(".json"):
            continue
        if not (name.startswith("cldr-cal-") or name.endswith("/ca-gregorian.json")):
            continue
        document = json.loads(archive.read(name))
        for source_locale, record in document["main"].items():
            for source_calendar, calendar in record["dates"]["calendars"].items():
                key = (source_locale, source_calendar)
                assert key not in source_files, key
                source_files[key] = calendar
    for locale, calendars in styles.items():
        assert set(calendars) == set(data["calendar_date_patterns.json"][locale])
        for identifier, actual in calendars.items():
            source_calendar = data["calendar_date_patterns.json"][locale][identifier]["sourceCalendar"]
            source = source_files[(locale, source_calendar)]
            expected = {"date": source["dateFormats"], "time": source["timeFormats"],
                        "glue": {style: source["dateTimeFormats"][style] for style in ["full", "long", "medium", "short"]},
                        "atTime": source.get("dateTimeFormats-atTime", {}),
                        "monthNumeric": source.get("monthPatterns", {}).get("numeric", {}).get("all", {}),
                        "monthNarrow": source["months"]["format"]["narrow"],
                        "monthNarrowPattern": source.get("monthPatterns", {}).get("format", {}).get("narrow", {})}
            assert actual == {"sourceCalendar": source_calendar, "patterns": expected}, (locale, identifier)
            override_count += sum(isinstance(value, dict) and "_numbers" in value
                                  for family in expected.values() for value in family.values())
            style_count += 1
            if source_calendar == "gregorian":
                candidates = [locale.rsplit("-", count)[0] for count in range(len(locale.split("-")))] + ["und"]
                inherited = next(candidate for candidate in candidates if candidate in source_rules)
                assert periods[locale] == {"names": source["dayPeriods"]["format"],
                                           "rules": source_rules[inherited], "sourceRuleLocale": inherited}, locale
    return {"dateTimeStyleSourcePayloadsCompared": style_count,
            "dayPeriodSourcePayloadsCompared": len(periods),
            "styleNumberingOverridesCompared": override_count}
