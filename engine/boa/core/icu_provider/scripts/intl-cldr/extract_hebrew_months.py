"""Extract CLDR Hebrew month names indexed by Temporal month codes."""
import argparse
import hashlib
import json
from pathlib import Path
import zipfile

SOURCE_SHA256 = "bbb9a9aac2dfc534bd18288678a5984023d11d22f712f3c33425f3214bd1def6"
SOURCE_URL = "https://github.com/unicode-org/cldr-json/releases/download/47.0.0/cldr-47.0.0-json-full.zip"
SOURCE_COMMIT = "16f6b8578ba5fe98959034706f337674f816fc3f"


def names_by_code(months):
    """CLDR's ordinal 6 is M05L; ordinal 7 is M06, or M06L for display."""
    normal = [months[str(number if number <= 5 else number + 1)] for number in range(1, 13)]
    leap = [""] * 12
    leap[4] = months["6"]
    leap[5] = months["7-yeartype-leap"]
    return normal + leap


def extract(archive):
    rows, inputs = [], {}
    for name in sorted(archive.namelist()):
        if not (name.startswith("cldr-cal-hebrew-full/main/") and name.endswith("/ca-hebrew.json")):
            continue
        raw = archive.read(name)
        inputs[name] = hashlib.sha256(raw).hexdigest()
        for locale, record in json.loads(raw)["main"].items():
            months = record["dates"]["calendars"]["hebrew"]["months"]
            for context in ("format", "stand-alone"):
                for width in ("abbreviated", "wide", "narrow"):
                    names = names_by_code(months[context][width])
                    if any(any(char in value for char in "\t\n\r") for value in names):
                        raise ValueError("month name cannot be encoded as a TSV field")
                    rows.append("\t".join([locale, context, width, *names]))
    if not rows:
        raise ValueError("no Hebrew month data found")
    return "\n".join(sorted(rows)) + "\n", inputs


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source_zip", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    actual = hashlib.sha256(args.source_zip.read_bytes()).hexdigest()
    if actual != SOURCE_SHA256:
        raise ValueError("source ZIP SHA256 mismatch")
    with zipfile.ZipFile(args.source_zip) as archive:
        text, inputs = extract(archive)
        license_text = archive.read("cldr-core/LICENSE")
    with args.output.open("x") as output:
        output.write(text)
    report = {"sourceUrl": SOURCE_URL, "sourceCommit": SOURCE_COMMIT,
              "sourceSha256": actual, "inputs": inputs, "rows": len(text.splitlines()),
              "outputSha256": hashlib.sha256(text.encode()).hexdigest()}
    with args.output.with_suffix(".provenance.json").open("x") as output:
        json.dump(report, output, indent=2, sort_keys=True)
        output.write("\n")
    with args.output.with_suffix(".LICENSE.txt").open("xb") as output:
        output.write(license_text)
    print(f"{len(inputs)} locales; {report['rows']} month payloads")


if __name__ == "__main__":
    main()
