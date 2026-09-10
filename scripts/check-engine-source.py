#!/usr/bin/env python3
"""Verify retained Boa source and the browser's resolved in-tree dependencies."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[1]
ENGINE_CRATES = {
    "boa_ast", "boa_engine", "boa_gc", "boa_interner", "boa_macros",
    "boa_parser", "boa_string", "small_btree", "tag_ptr",
}


def inspect(pristine=False):
    origin = json.loads((ROOT / "engine/boa-origin.json").read_text())
    source = ROOT / origin["path"]
    missing, changed = [], []
    assert origin["file_count"] == len(origin["files"])
    assert origin["bytes"] == sum(item["bytes"] for item in origin["files"].values())
    for name, expected in origin["files"].items():
        path = source / name
        if not path.is_file():
            missing.append(name)
        elif (hashlib.sha256(path.read_bytes()).hexdigest() != expected["sha256"]
              or path.stat().st_mode & 0o777 != int(expected["mode"], 8) & 0o777):
            changed.append(name)
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1"], cwd=ROOT,
    ))
    packages = [package for package in metadata["packages"]
                if package["name"] in ENGINE_CRATES or package["name"].startswith("boa_")]
    names = {package["name"] for package in packages}
    invalid = [package["name"] for package in packages
               if package["source"] is not None
               or not Path(package["manifest_path"]).is_relative_to(source)]
    report = {
        "imported_revision": origin["revision"],
        "imported_tree": origin["tree"],
        "original_files": origin["file_count"],
        "missing_files": missing,
        "modified_since_import": changed,
        "engine_crates": [{"name": package["name"], "version": package["version"],
                           "manifest": str(Path(package["manifest_path"]).relative_to(ROOT))}
                          for package in packages if package["name"] not in invalid],
        "invalid_engine_sources": invalid,
        "missing_engine_crates": sorted(ENGINE_CRATES - names),
    }
    report["passed"] = not (missing or invalid or ENGINE_CRATES - names
                             or (pristine and changed))
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pristine", action="store_true",
                        help="also require byte-identical source for the initial import")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    report = inspect(args.pristine)
    encoded = json.dumps(report, indent=2) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded)
    print(encoded, end="")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
