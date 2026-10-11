"""Extract pinned CLDR 48 Hijri names for ICU's two-era calendar model."""
import argparse
import hashlib
import json
from pathlib import Path
import zipfile

SOURCE_SHA256 = "25ab2651a1a874dd62a727d3c213d7c6bf0fc269b09d2937f0499b82d99fa58e"
SOURCE_URL = "https://github.com/unicode-org/cldr-json/releases/download/48.0.0/cldr-48.0.0-json-full.zip"
SOURCE_COMMIT = "4d06be52b51bb2f75688d0abe55c52a66afed790"


def extract(archive):
    """Preserve source locale, width and era order AH/BH without fallback."""
    rows, inputs = [], {}
    for name in sorted(archive.namelist()):
        if not (name.startswith("cldr-cal-islamic-full/main/") and name.endswith("/ca-islamic.json")):
            continue
        raw = archive.read(name)
        inputs[name] = hashlib.sha256(raw).hexdigest()
        for locale, record in json.loads(raw)["main"].items():
            eras = record["dates"]["calendars"]["islamic"]["eras"]
            for width in ("eraAbbr", "eraNames", "eraNarrow"):
                names = [eras[width][str(index)] for index in (0, 1)]
                if any(not value or any(char in value for char in "\t\n\r") for value in names):
                    raise ValueError("era name cannot be encoded as a TSV field")
                rows.append("\t".join([locale, width, *names]))
    if not rows:
        raise ValueError("no Hijri era data found")
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
    print(f"{len(inputs)} locales; {report['rows']} era payloads")


if __name__ == "__main__":
    main()
