"""Pinned CLDR47 XML root symbols needed for numbering systems absent from JSON locales."""
import hashlib
import re
import urllib.request
import xml.etree.ElementTree as ET

ROOT_URL = "https://raw.githubusercontent.com/unicode-org/cldr/2ef784e3a4168bc2a43cd1b5b9839b6636f5899c/common/main/root.xml"
ROOT_COMMIT = "2ef784e3a4168bc2a43cd1b5b9839b6636f5899c"
ROOT_TAG = "release-47"
ROOT_TAG_OBJECT = "adbaf76bf62572cbfe2044564c40ed44beb00f8a"
ROOT_SHA256 = "1ac705812e9e186b8d904813a4989d160729cc4d217849ad3736257da10f69b0"


def element_record(element):
    """Preserve complete source fields and ordering while discarding structural indentation."""
    result = {"tag": element.tag, "attributes": dict(element.attrib)}
    children = list(element)
    if children:
        result["children"] = [element_record(child) for child in children]
    elif element.text is not None:
        result["value"] = element.text
    return result


def resolve_symbols(elements, numbering_system, resolving=()):
    if numbering_system in resolving:
        raise ValueError(f"cyclic root symbol alias: {resolving} {numbering_system}")
    element = elements[numbering_system]
    alias = element.find("alias")
    if alias is not None:
        match = re.fullmatch(r"\.\./symbols\[@numberSystem='([a-z0-9]+)'\]", alias.attrib["path"])
        if alias.attrib["source"] != "locale" or not match:
            raise ValueError(f"unsupported root symbol alias: {alias.attrib}")
        return resolve_symbols(elements, match.group(1), (*resolving, numbering_system))
    fields = {}
    for child in element:
        key = child.tag + ("-alt-" + child.attrib["alt"] if "alt" in child.attrib else "")
        if child.text is None:
            raise ValueError(f"empty root symbol: {numbering_system} {key}")
        fields[key] = child.text
    return fields


def extract_root(source):
    if not source.exists():
        source.parent.mkdir(parents=True, exist_ok=True)
        with urllib.request.urlopen(ROOT_URL, timeout=30) as response, source.open("xb") as sink:
            while block := response.read(1024 * 1024):
                sink.write(block)
    content = source.read_bytes()
    if hashlib.sha256(content).hexdigest() != ROOT_SHA256:
        raise ValueError("root XML hash differs from the pinned CLDR47 source")
    numbers = ET.fromstring(content).find("numbers")
    elements = {element.attrib["numberSystem"]: element for element in numbers.findall("symbols") if "numberSystem" in element.attrib}
    records = {nu: {"source": element_record(element), "resolvedSymbols": resolve_symbols(elements, nu)} for nu, element in elements.items()}
    if len(records) != 77 or records["arabext"]["resolvedSymbols"]["timeSeparator"] != "٫":
        raise ValueError("unexpected root numbering-system symbol data")
    formats = {element.attrib.get("numberSystem", "default"): element_record(element) for element in numbers.findall("currencyFormats")}
    number_formats = {kind: {element.attrib.get("numberSystem", "default"): element_record(element) for element in numbers.findall(kind + "Formats")} for kind in ("decimal", "percent", "scientific")}
    metadata = {"repository": "https://github.com/unicode-org/cldr", "tag": ROOT_TAG, "tagObject": ROOT_TAG_OBJECT, "commit": ROOT_COMMIT, "url": ROOT_URL, "sha256": ROOT_SHA256, "bytes": len(content)}
    default_alias = element_record(next(element for element in numbers.findall("symbols") if "numberSystem" not in element.attrib))
    return {"numberingSystemSymbols": records, "defaultSymbolsSource": default_alias, "currencyFormatsSource": formats, "numberFormatsSource": number_formats}, metadata
