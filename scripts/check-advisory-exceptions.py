#!/usr/bin/env python3
"""Reject advisory exceptions without a reason, scope or unexpired review date."""
import datetime
import re
import tomllib
from pathlib import Path

# Only the maintenance findings reviewed in docs/security/boa-dependency-audit.md
# may be excepted. Vulnerability and unsound findings must remain fatal.
MAINTENANCE_EXCEPTIONS = {
    "RUSTSEC-2024-0384", "RUSTSEC-2024-0436", "RUSTSEC-2025-0134",
    "RUSTSEC-2026-0206", "RUSTSEC-2026-0192",
}


def validate(config, today):
    errors = []
    for entry in config.get("advisories", {}).get("ignore", []):
        if not isinstance(entry, dict):
            errors.append("Each exception must include an ID and reason")
            continue
        advisory = entry.get("id", "")
        reason = entry.get("reason", "")
        if not isinstance(advisory, str) or not re.fullmatch(r"RUSTSEC-\d{4}-\d{4}", advisory):
            errors.append(f"Invalid advisory ID: {advisory}")
        elif advisory not in MAINTENANCE_EXCEPTIONS:
            errors.append(f"{advisory}: not a reviewed maintenance exception")
        if not isinstance(reason, str):
            errors.append(f"{advisory}: reason must be a string")
            continue
        match = re.search(r"review_by=(\d{4}-\d{2}-\d{2})", reason)
        try:
            expiry = datetime.date.fromisoformat(match[1]) if match else None
        except ValueError:
            expiry = None
        if expiry is None or expiry <= today:
            errors.append(f"{advisory}: missing, invalid or expired review date")
        if not match or len(reason[:match.start()].strip()) < 20:
            errors.append(f"{advisory}: describe scope and reason")
    return errors


if __name__ == "__main__":
    config = tomllib.loads(Path("deny.toml").read_text())
    errors = validate(config, datetime.datetime.now(datetime.timezone.utc).date())
    for error in errors:
        print(error)
    raise SystemExit(bool(errors))
