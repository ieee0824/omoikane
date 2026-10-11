"""Preserve canonical source inventories; runtime providers decide availability."""

TIME_DATA_FILE = "cldr-core/supplemental/timeData.json"
COLLATIONS_FILE = "cldr-bcp47/bcp47/collation.json"


def extract_supported_values(datasets, source, time_data):
    collations = source["keyword"]["u"]["co"]
    calendars = datasets["calendar_date_patterns.json"]
    return {
        "calendar": sorted({calendar for values in calendars.values() for calendar in values} | {"iso8601"}),
        "collation": sorted(key for key, value in collations.items()
                            if not key.startswith("_") and key not in ("standard", "search")
                            and not value.get("_deprecated", False)),
        "currency": sorted({code for currencies in datasets["currency_names.json"].values()
                            for code in currencies}),
        "numberingSystem": sorted(datasets["root_number_symbols.json"]["numberingSystemSymbols"]),
        "locales": sorted(calendars),
        "hourCycle12h11Regions": sorted(key for key, record in time_data["supplemental"]["timeData"].items()
                                       if next((clock[0] for clock in record["_allowed"].split()
                                                if clock[0] in {"h", "K"}), None) == "K"),
    }


def validate_supported_values(data):
    if set(data) != {"calendar", "collation", "currency", "numberingSystem", "locales", "hourCycle12h11Regions"}:
        raise ValueError("unexpected supported-value inventories")
    for category, values in data.items():
        if not values or values != sorted(set(values)) or any(not isinstance(value, str) or not value for value in values):
            raise ValueError(f"invalid canonical inventory: {category}")
    if "iso8601" not in data["calendar"] or not {"USD", "JPY", "XXX"} <= set(data["currency"]):
        raise ValueError("missing baseline calendar or currencies")
    if any(len(code) != 3 or not code.isascii() or not code.isalpha() or code != code.upper()
           for code in data["currency"]):
        raise ValueError("invalid currency inventory")
