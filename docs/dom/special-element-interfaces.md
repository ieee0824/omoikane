# Special HTML and MathML element interfaces

Omoikane exposes dedicated DOM interfaces for `meter`, `progress`, `picture`,
`source`, `track`, `datalist`, `map`, and `embed`, and exposes MathML elements
through `MathMLElement` (with MathML `a` using `MathMLAnchorElement`). Interface
selection is based on both namespace and local name, so parser-created, cloned,
imported, and adopted nodes retain the same platform interface.

The current interface slice implements the reflected attributes and collection
members defined by these interfaces. `meter` and `progress` normalize their
numeric state, label lists are live, and `datalist.options` and `map.areas` are
live collections. URL-valued attributes resolve against the node document.
`HTMLTrackElement.track` is currently `null`; loading and `TextTrack` state are
part of the remaining media integration. Picture candidate selection and the
native appearance of meter and progress likewise remain tracked by issue
[#1286](https://github.com/ieee0824/omoikane/issues/1286).

The focused upstream contract is recorded in
`tests/wpt/special-element-interfaces.json`. Run it against the pinned WPT
revision with:

```sh
scripts/fetch-wpt.sh
WPT_REQUIRED=1 WPT_MANIFEST=tests/wpt/special-element-interfaces.json \
  cargo test --locked --test wpt_smoke -- --nocapture
```
