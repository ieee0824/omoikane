#!/usr/bin/env python3
"""Generate event handler inventories from the pinned WPT WebIDL sources."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

REPOSITORY = Path(__file__).resolve().parents[1]
MIXINS = {"GlobalEventHandlers": "global", "WindowEventHandlers": "window"}
MIXIN_PATTERN = re.compile(
    r"(?:partial\s+)?interface\s+mixin\s+(GlobalEventHandlers|WindowEventHandlers)\s*\{([^{}]*)\};",
    re.S,
)
MEMBER_PATTERN = re.compile(r"attribute\s+\w+\s+on(\w+)\s*;")


def collect_inventory(checkout):
    expected = (REPOSITORY / "tests/wpt/revision.txt").read_text().strip()
    actual = subprocess.check_output(
        ["git", "-C", str(checkout), "rev-parse", "HEAD"], text=True
    ).strip()
    if actual != expected:
        raise ValueError(f"WPT revision {actual} differs from pinned {expected}")
    names = subprocess.check_output(
        ["git", "-C", str(checkout), "ls-tree", "-r", "--name-only", "HEAD", "interfaces"], text=True
    ).splitlines()
    paths = [checkout / name for name in names if name.endswith(".idl")]
    missing = [str(path.relative_to(checkout)) for path in paths if not path.is_file()]
    if missing:
        raise ValueError("Fetch the complete pinned /interfaces/ directory; missing " + ", ".join(missing))
    dirty = subprocess.check_output(
        ["git", "-C", str(checkout), "status", "--porcelain", "--", "interfaces"], text=True
    ).strip()
    if dirty:
        raise ValueError("Pinned WPT interfaces have local changes")
    sources, groups = {}, {"global": set(), "window": set(), "secureWindow": set()}
    for path in sorted(paths):
        source = path.read_bytes()
        text = re.sub(r"//[^\n]*|/\*.*?\*/", "", source.decode(), flags=re.S)
        sections = list(MIXIN_PATTERN.finditer(text))
        for section in sections:
            groups[MIXINS[section[1]]].update(MEMBER_PATTERN.findall(section[2]))
        if sections:
            sources[path.name] = hashlib.sha256(source).hexdigest()
    orientation = checkout / "interfaces/orientation-event.idl"
    source = orientation.read_bytes()
    groups["secureWindow"].update(re.findall(
        r"\[SecureContext\]\s+attribute\s+EventHandler\s+on(\w+)\s*;", source.decode()
    ))
    sources[orientation.name] = hashlib.sha256(source).hexdigest()
    if "html.idl" not in sources or len(groups["secureWindow"]) != 3:
        raise ValueError("Missing HTML handler mixins or secure sensor handlers")
    extras = json.loads((REPOSITORY / "scripts/event-handler-extras.json").read_text())
    for group, handlers in extras.items():
        groups[group].update(handlers)
    return {"revision": expected, "sources": sources, "extras": extras,
            **{group: sorted(values) for group, values in groups.items()}}


def declaration(name, values):
    lines = []
    for offset in range(0, len(values), 6):
        lines.append("    " + ", ".join(json.dumps(value) for value in values[offset:offset + 6]) + ",")
    return "  const " + name + " = [\n" + "\n".join(lines) + "\n  ];"


def generated_handlers(inventory):
    path = REPOSITORY / "src/js/event_handlers.js"
    text = path.read_text()
    for group, name in [("global", "globalTypes"), ("window", "windowTypes"),
                        ("secureWindow", "secureWindowTypes")]:
        pattern = r"  const " + name + r" = \[.*?\];"
        text, count = re.subn(pattern, lambda _: declaration(name, inventory[group]), text, flags=re.S)
        if count != 1:
            raise ValueError(f"Expected one {name} declaration, found {count}")
    return text


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--wpt-root", type=Path, default=Path(os.environ.get("WPT_ROOT", "target/wpt")))
    parser.add_argument("--check", action="store_true", help="Fail when generated files differ")
    args = parser.parse_args()
    inventory = collect_inventory(args.wpt_root.resolve())
    files = {
        REPOSITORY / "src/js/event_handler_inventory.json": json.dumps(inventory, indent=2, sort_keys=True) + "\n",
        REPOSITORY / "src/js/event_handlers.js": generated_handlers(inventory),
    }
    differences = [str(path.relative_to(REPOSITORY)) for path, text in files.items()
                   if not path.exists() or path.read_text() != text]
    if args.check and differences:
        raise SystemExit("Regenerate handler inventory: " + ", ".join(differences))
    if not args.check:
        for path, text in files.items():
            path.write_text(text)
    print("Handler inventory: " + ", ".join(f"{group}={len(inventory[group])}" for group in ["global", "window", "secureWindow"]))


if __name__ == "__main__":
    main()
