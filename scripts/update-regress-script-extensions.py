#!/usr/bin/env python3
"""Regenerate four Script_Extensions tables from pinned Unicode 17 UCD files.

Pass a directory containing Scripts.txt and ScriptExtensions.txt downloaded from
https://www.unicode.org/Public/17.0.0/ucd/. Other Unicode tables retain their
existing version; this does not claim a complete Unicode 17 upgrade.
"""
import argparse
import hashlib
from pathlib import Path
import re

HASHES = {
    "Scripts.txt": "9f5e50d3abaee7d6ce09480f325c706f485ae3240912527e651954d2d6b035bf",
    "ScriptExtensions.txt": "ec2107e58825a1586acee8e0911ce18260394ac8b87e535ca325f1ccbeb06bc6",
}
SCRIPTS = {"Arabic": "Arab", "Bengali": "Beng", "Cyrillic": "Cyrl", "Devanagari": "Deva"}


def records(path):
    data = path.read_bytes()
    if hashlib.sha256(data).hexdigest() != HASHES[path.name]:
        raise ValueError(f"unexpected Unicode 17 input: {path}")
    for line in data.decode().splitlines():
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        interval, value = map(str.strip, line.split(";"))
        bounds = interval.split("..")
        yield range(int(bounds[0], 16), int(bounds[-1], 16) + 1), value


def intervals(points):
    result = []
    for point in sorted(points):
        if result and result[-1][1] + 1 == point:
            result[-1][1] = point
        else:
            result.append([point, point])
    return result


def regenerate(input_dir, table_path):
    scripts = list(records(input_dir / "Scripts.txt"))
    extensions = list(records(input_dir / "ScriptExtensions.txt"))
    text = table_path.read_text()
    for name, alias in SCRIPTS.items():
        points = {point for span, value in scripts if value == name for point in span}
        # Explicit Script_Extensions values replace the default Script value.
        for span, value in extensions:
            points.difference_update(span)
            if alias in value.split():
                points.update(span)
        ranges = intervals(points)
        constant = name.upper() + "_EXTENSIONS"
        replacement = f"const {constant}: [Interval; {len(ranges)}] = [\n"
        replacement += "".join(f"    Interval::new({first}, {last}),\n" for first, last in ranges)
        replacement += "];"
        text, count = re.subn(rf"const {constant}:.*?= \[.*?\];", replacement, text, flags=re.S)
        if count != 1:
            raise ValueError(f"expected exactly one {constant} table")
    return text


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input_dir", type=Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    table = Path(__file__).resolve().parents[1] / "engine/regress/src/unicodetables.rs"
    text = regenerate(args.input_dir, table)
    if args.check:
        if table.read_text() != text:
            raise SystemExit("Script_Extensions tables need regeneration")
    else:
        table.write_text(text)


if __name__ == "__main__":
    main()
