"""Compare global source inventories without invoking the extraction helper."""
import json


def compare_supported_values(archive, data):
    actual = data["supported_values.json"]
    currencies, locales, calendars = set(), set(), {"iso8601"}
    for filename in archive.namelist():
        if filename.startswith("cldr-numbers-full/main/") and filename.endswith("/currencies.json"):
            records = json.loads(archive.read(filename))["main"]
            locales.update(records)
            for record in records.values():
                currencies.update(record["numbers"]["currencies"])
        if "/main/" in filename and filename.endswith(".json") and (
            filename.startswith("cldr-cal-") or filename.endswith("/ca-gregorian.json")
        ):
            for record in json.loads(archive.read(filename))["main"].values():
                for calendar in record["dates"]["calendars"]:
                    calendars.add({"gregorian": "gregory", "ethiopic-amete-alem": "ethioaa"}.get(calendar, calendar))
    source = json.loads(archive.read("cldr-bcp47/bcp47/collation.json"))["keyword"]["u"]["co"]
    collations = {key for key in source if not key.startswith("_") and key not in {"search", "standard"}
                  and source[key].get("_deprecated") is not True}
    systems = json.loads(archive.read("cldr-core/supplemental/numberingSystems.json"))["supplemental"]["numberingSystems"]
    numbering = {key for key, record in systems.items() if record["_type"] == "numeric"}
    expected = {"calendar": sorted(calendars), "currency": sorted(currencies), "locales": sorted(locales),
                "collation": sorted(collations), "numberingSystem": sorted(numbering)}
    time_data = json.loads(archive.read("cldr-core/supplemental/timeData.json"))["supplemental"]["timeData"]
    preferred_zero_based = []
    for region, record in time_data.items():
        alternatives = record["_allowed"].split(" ")
        twelve = [entry for entry in alternatives if entry.startswith(("h", "K"))]
        if twelve and twelve[0].startswith("K"):
            preferred_zero_based.append(region)
    expected["hourCycle12h11Regions"] = sorted(preferred_zero_based)
    assert actual == expected
    return {"supportedValueSourceInventoriesCompared": len(expected),
            "supportedValueSourceEntriesCompared": sum(map(len, expected.values()))}
