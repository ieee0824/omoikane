"""Detect drift between the Window and Worker bootstrap implementations.

The explicit replacements document the reviewed differences; they do not prove
those differences semantically equivalent. Runtime contracts test behavior.
This checker handles trusted section comments, not arbitrary JavaScript syntax.
Run: python3 -m unittest discover -s scripts/tests -p 'test_bootstrap_section_pairs.py'
"""
from pathlib import Path
import re
import json
import subprocess
import tempfile
import unittest
W = Path(__file__).resolve().parents[2] / 'src/js/dom_bootstrap.js'
PAT = re.compile(
    r'/\* @omoikane-bootstrap (window-only|worker-only) ([\w-]+) \*/'
    r'(.*?)/\* @omoikane-bootstrap end \*/', re.S)
PREFIX = "/* @omoikane-bootstrap "
# New regions require explicit review. This is a maintenance ratchet, not a
# specification limit or proof that marker-like text is a JavaScript comment.
SECTION_COUNTS = {
    "dom_bootstrap.js": 156,
    "font_loading.js": 1,
    "window_members.js": 1,
    "xpath.js": 1,
    "highlight.js": 1,
    "find_in_page.js": 1,
}
RULES = {'callListener-674': [('target instanceof Node && nodeRoot(target) instanceof ShadowRoot',
                       'workerIsNode(target) && isEventShadowRoot(nodeRoot(target))')],
 'isEventNode-729': [('node instanceof Node', 'workerIsNode(node)')],
 'isEventShadowRoot-730': [('function isEventShadowRoot(node) {\n'
                            '    if (node instanceof Node) return node instanceof ShadowRoot;\n'
                            '    return isEventNode(node) && internalNodeType(node) === 11 &&\n'
                            '      nativeShadowHost(internalNodeId(node)) !== null;\n'
                            '  }',
                            'function isEventShadowRoot(node) { return workerIsNode(node) && '
                            'internalNodeType(node) === 11 && '
                            'nativeShadowHost(internalNodeId(node)) !== null; }')],
 'dispatchEventOnTarget-739': [('finalEntry.target instanceof Node',
                                'workerIsNode(finalEntry.target)'),
                               ('finalEntry.relatedTarget instanceof Node',
                                'workerIsNode(finalEntry.relatedTarget)')],
 'nearestInclusiveAutoPopover-851': [('node instanceof Node', 'workerIsNode(node)')],
 'dom-initialization-2473': [('globalThis.localStorage = storageForDocument("local", document, '
                              'globalThis);',
                              '  const workerStorageDocument = Object.freeze({__id: '
                              '__omoikane_document_id, get URL() { return '
                              'String(nativeDocumentURL(__omoikane_document_id)); }});\n'
                              'globalThis.localStorage = storageForDocument("local", '
                              'workerStorageDocument, globalThis);')],
 'dom-initialization-2474': [('storageForDocument("session", document, globalThis)',
                              'storageForDocument("session", workerStorageDocument, globalThis)')],
 'serializeStructuredValue-3087': [('value instanceof Node', 'workerIsNode(value)')],
 'dom-initialization-3457': [('[HTMLElement.prototype, Document.prototype, SVGElement.prototype, '
                              'globalThis]',
                              '[globalThis]'),
                             ('[globalThis, HTMLBodyElement.prototype, '
                              'HTMLFrameSetElement.prototype]',
                              '[globalThis]'),
                             ('    wireInlineHandlers(globalThis.document);', '')],
 'window-focus-bridge': [('focus() {\n'
                          '      if (this !== focusDocument.defaultView && '
                          '!windowObjects.has(this))',
                          'focus() {\n      if (!windowObjects.has(this))'),
                         ('blur() {\n'
                          '      if (this !== focusDocument.defaultView && '
                          '!windowObjects.has(this))',
                          'blur() {\n      if (!windowObjects.has(this))')],
 'focus-window-id': [('focusAcrossDocuments(focusDocument, null, true);',
                      'focusAcrossDocumentIds(__omoikane_document_id, null, true);')],
 'focus-command-bridge': [('    if (id === null) return null;',
                           '    if (id === null) return null;\n'
                           '    // Child globals have no DOM elements; their viewport snapshot has '
                           'no element ID.\n'
                           '    if (id === __omoikane_document_id && kind === "snapshot") return '
                           'null;')]}


def sections(source):
    """Read trusted directives, rejecting unmatched, malformed and nested ones."""
    result = {}
    cursor = 0
    active = None
    while (start := source.find(PREFIX, cursor)) != -1:
        end = source.find("*/", start + len(PREFIX))
        if end == -1:
            raise ValueError("unclosed directive")
        fields = source[start + len(PREFIX):end].split()
        if fields == ["end"]:
            if active is None:
                raise ValueError("unmatched end")
            key, body_start = active
            result[key] = source[body_start:start]
            active = None
        else:
            if active is not None:
                raise ValueError("nested section")
            if (len(fields) != 2 or fields[0] not in ("window-only", "worker-only")
                    or re.fullmatch(r"[A-Za-z0-9_-]+", fields[1]) is None):
                raise ValueError("unknown directive")
            key = tuple(fields)
            if key in result:
                raise ValueError("duplicate " + fields[1])
            active = (key, end + 2)
        cursor = end + 2
    if active is not None:
        raise ValueError("unclosed section")
    return result


def check(source):
    bodies = sections(source)
    worker_names = {name for kind, name in bodies if kind == "worker-only"}
    if worker_names != set(RULES) | {"node-brand"}:
        raise ValueError("worker pair inventory drift")
    for name, rules in RULES.items():
        if ("window-only", name) not in bodies:
            raise ValueError("missing counterpart " + name)
        expected = bodies["window-only", name]
        for old, new in rules:
            if expected.count(old) != 1:
                raise ValueError("expected substitution count " + name)
            expected = expected.replace(old, new, 1)
        if expected != bodies["worker-only", name]:
            raise ValueError("unapproved pair difference " + name)
    return len(RULES)


def mutate_body(source, kind, name, mutate):
    def replace(match):
        if match.group(1) == kind and match.group(2) == name:
            return match.group(0).replace(match.group(3), mutate(match.group(3)), 1)
        return match.group(0)
    return PAT.sub(replace, source)


def check_inventory(sources):
    """Discover all marked files so newly introduced regions cannot bypass review."""
    marked = {name: sections(text) for name, text in sources.items() if PREFIX in text}
    if set(marked) != set(SECTION_COUNTS):
        raise ValueError("marked file inventory drift")
    for name, bodies in marked.items():
        if len(bodies) != SECTION_COUNTS[name]:
            raise ValueError("section count drift " + name)
        expected = (set(RULES) | {"node-brand"} if name == "dom_bootstrap.js"
                    else {"window-brand"} if name == "window_members.js" else set())
        if {label for kind, label in bodies if kind == "worker-only"} != expected:
            raise ValueError("worker region inventory drift " + name)
    return marked


def select_sections(source, worker):
    """Render the validated sections for comparison with the product selector."""
    sections(source)
    result = []
    cursor = 0
    active = None
    while (start := source.find(PREFIX, cursor)) != -1:
        if active is None or active == worker:
            result.append(source[cursor:start])
        end = source.index("*/", start + len(PREFIX))
        fields = source[start + len(PREFIX):end].split()
        active = None if fields == ["end"] else fields[0] == "worker-only"
        cursor = end + 2
    result.append(source[cursor:])
    return "".join(result)


class ProductSelectorAgreementTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.directory.cleanup)
        root = Path(cls.directory.name)
        cls.binary = root / "selector"
        source = root / "selector.rs"
        # Compile the actual product source, without cfg(test) or Cargo dependencies.
        source.write_text(
            "#[path = " + json.dumps(str(W.parent / "bootstrap_sections.rs")) + "]\n"
            "mod selector;\n"
            "fn main() {\n"
            "  use std::io::{Read, Write};\n"
            "  let mut source = String::new();\n"
            "  std::io::stdin().read_to_string(&mut source).unwrap();\n"
            "  let worker = std::env::args().nth(1).unwrap() == \"worker\";\n"
            "  match selector::select(&source, worker) {\n"
            "    Ok(value) => std::io::stdout().write_all(value.as_bytes()).unwrap(),\n"
            "    Err(_) => std::process::exit(2),\n"
            "  }\n}\n", encoding="utf-8")
        subprocess.run(["rustc", "--edition=2024", str(source), "-o", str(cls.binary)],
                       check=True, capture_output=True)

    def assert_agreement(self, source):
        for worker in (False, True):
            result = subprocess.run([str(self.binary), "worker" if worker else "window"],
                                    input=source.encode("utf-8"), capture_output=True)
            try:
                expected = select_sections(source, worker)
            except ValueError:
                self.assertEqual(result.returncode, 2)
            else:
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, expected.encode("utf-8"))

    def test_all_embedded_sources_match_product_selector(self):
        for path in W.parent.glob("*.js"):
            with self.subTest(file=path.name):
                self.assert_agreement(path.read_text(encoding="utf-8"))

    def test_malformed_and_utf8_fixtures_match_product_selector(self):
        for source in (
            "λ/* @omoikane-bootstrap worker-only a */中/* @omoikane-bootstrap end */尾",
            "/* @omoikane-bootstrap end */",
            "/* @omoikane-bootstrap worker-only",
            "/* @omoikane-bootstrap worker-only missing */",
            "/* @omoikane-bootstrap unknown a */",
            "/* @omoikane-bootstrap worker-only bad! */",
            "/* @omoikane-bootstrap worker-only a *//* @omoikane-bootstrap worker-only b */",
            "/* @omoikane-bootstrap worker-only a *//* @omoikane-bootstrap end */"
            "/* @omoikane-bootstrap worker-only a *//* @omoikane-bootstrap end */",
        ):
            with self.subTest(source=source):
                self.assert_agreement(source)


class BootstrapSectionPairTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = W.read_text(encoding="utf-8")

    @staticmethod
    def sources():
        return {path.name: path.read_text(encoding="utf-8")
                for path in W.parent.glob("*.js")}

    def test_region_inventory_requires_explicit_review(self):
        self.assertEqual(len(check_inventory(self.sources())), 6)

    def test_new_marked_file_cannot_bypass_inventory(self):
        sources = self.sources()
        sources["new_bootstrap.js"] = (
            "/* @omoikane-bootstrap worker-only surprise */"
            "code/* @omoikane-bootstrap end */")
        with self.assertRaisesRegex(ValueError, "marked file inventory drift"):
            check_inventory(sources)

    def test_adapter_cannot_acquire_unreviewed_sections(self):
        sources = self.sources()
        sources["worker_dom_surface.js"] += (
            "/* @omoikane-bootstrap worker-only surprise */"
            "code/* @omoikane-bootstrap end */")
        with self.assertRaisesRegex(ValueError, "marked file inventory drift"):
            check_inventory(sources)

    def test_worker_mode_change_outside_core_is_rejected(self):
        sources = self.sources()
        sources["font_loading.js"] = sources["font_loading.js"].replace(
            "window-only document-fonts", "worker-only document-fonts")
        with self.assertRaisesRegex(ValueError, "worker region inventory drift"):
            check_inventory(sources)

    def test_malformed_directives_are_rejected(self):
        for text in (
            "/* @omoikane-bootstrap end */",
            "/* @omoikane-bootstrap worker-only missing */text",
            "/* @omoikane-bootstrap worker-only",
            "/* @omoikane-bootstrap unknown mode */",
            "/* @omoikane-bootstrap worker-only bad! */",
            "/* @omoikane-bootstrap worker-only a */"
            "/* @omoikane-bootstrap window-only b */",
        ):
            with self.subTest(text=text), self.assertRaises(ValueError):
                sections(text)

    def test_embedded_pairs_have_only_reviewed_differences(self):
        self.assertEqual(check(self.source), 12)

    def test_worker_only_change_is_rejected(self):
        changed = mutate_body(self.source, "worker-only", "callListener-674",
                              lambda body: body + "throw new Error();")
        with self.assertRaisesRegex(ValueError, "unapproved pair difference"):
            check(changed)

    def test_missing_counterpart_is_rejected(self):
        changed = self.source.replace("window-only callListener-674",
                                      "window-only renamed-pair")
        with self.assertRaisesRegex(ValueError, "missing counterpart"):
            check(changed)

    def test_missing_worker_section_is_rejected(self):
        changed = PAT.sub(lambda match: "" if match.group(1) == "worker-only"
                          and match.group(2) == "callListener-674"
                          else match.group(0), self.source)
        with self.assertRaisesRegex(ValueError, "inventory drift"):
            check(changed)

    def test_constant_brand_substitution_is_rejected(self):
        changed = mutate_body(self.source, "worker-only", "isEventNode-729",
                              lambda body: body.replace("workerIsNode(node)", "false"))
        with self.assertRaisesRegex(ValueError, "unapproved pair difference"):
            check(changed)

    def test_common_maintenance_edit_is_allowed(self):
        changed = self.source
        for kind in ("window-only", "worker-only"):
            changed = mutate_body(changed, kind, "callListener-674",
                                  lambda body: body + "// common maintenance edit\n")
        self.assertEqual(check(changed), 12)

    def test_changed_substitution_site_requires_review(self):
        changed = self.source
        for kind in ("window-only", "worker-only"):
            changed = mutate_body(changed, kind, "isEventNode-729",
                                  lambda body: body.replace("node", "candidate"))
        with self.assertRaisesRegex(ValueError, "expected substitution count"):
            check(changed)

    def test_duplicate_section_is_rejected(self):
        section = next(match.group(0) for match in PAT.finditer(self.source)
                       if match.group(1) == "worker-only")
        with self.assertRaisesRegex(ValueError, "duplicate"):
            check(self.source + section)


if __name__ == "__main__":
    unittest.main()
