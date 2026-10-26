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
The layout pipeline carries normalized determinate or indeterminate progress
state and normalized meter ranges into paint. Their default native appearance
draws a track plus a value bar; meter bars distinguish optimum, suboptimal, and
even-less-good regions.

`picture` image resolution walks `source` children in tree order, filters them
by supported image type and the current layout viewport's media environment,
then selects `srcset` density or width candidates using `sizes`. A failed
candidate still uses the child `img` fallback, matching Omoikane's established
resource-failure behavior.

`HTMLTrackElement.track` is currently `null`; loading and `TextTrack` state are
part of the broader timed-text media integration tracked by issue
[#1299](https://github.com/ieee0824/omoikane/issues/1299).

The focused upstream contract is recorded in
`tests/wpt/special-element-interfaces.json`. Run it against the pinned WPT
revision with:

```sh
scripts/fetch-wpt.sh
WPT_REQUIRED=1 WPT_MANIFEST=tests/wpt/special-element-interfaces.json \
  cargo test --locked --test wpt_smoke -- --nocapture
```
