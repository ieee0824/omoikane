# Sanitizer configuration and filtering

`constants.json` contains owned data derived from the WHATWG HTML Standard,
last updated 2026-10-07 and retrieved on 2026-10-08:

- [HTML element definitions](https://html.spec.whatwg.org/) provide each
  element's sanitization category, local allowed attributes, and navigating URLs.
- [§8.6.5 Sanitization constants](https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#sanitization-constants)
  provides global allowed attributes and the MathML/SVG default elements.
- [HTML event handler definitions](https://html.spec.whatwg.org/multipage/webappapis.html#event-handlers)
  provide `on*` content attributes; SVG adds `onbegin`, `onend`, `onrepeat`.

The complete source HTML has SHA256
`089cedd161195f010ce9d384e39e8bf579162332d2d07ab03f866f85a5cc467a`.
The data is generated from the specification, independently of WPT expectations:

```sh
python3 src/js/html_sanitizer/generate_constants.py \
  --standard-file /path/to/html-standard.html \
  --output /tmp/sanitizer-constants.json
```

Compare parsed JSON values when reviewing an update; key order is irrelevant.
The current snapshot contains 122 default elements, 58 global allowed attributes,
8 unsafe elements, and 7 navigating element/attribute pairs. Configuration
canonicalization supplies per-element defaults and validates invariants before use.
Filtering visits parsed children, template contents, and both open and closed shadow
trees before registration, resource scheduling, or custom element reactions.
