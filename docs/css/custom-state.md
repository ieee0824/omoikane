# Custom element states

`ElementInternals.states` exposes the same Window-only `CustomStateSet` for each custom element.
It accepts DOMString values, preserves insertion order and exact UTF-16 units, and
supports live iterators and `forEach`. It does not require `formAssociated`.
`add` returns the receiver; `keys`, `values`, and `Symbol.iterator` share a function.
Public construction and calls with an unbranded receiver throw `TypeError`.
Dedicated, Shared and Service Worker globals do not expose the interface.
Structured cloning and messaging reject genuine state collections with
`DataCloneError`, even after prototype changes and across Window realms. The
private brand check runs before author property reads and transfer side effects,
as required by [structured serialization](https://html.spec.whatwg.org/multipage/structured-data.html#structuredserializeinternal).

Native DOM elements own optional state membership. JavaScript keeps the ordered
collection and its owner in private runtime state; matching does not invoke author
getters or `Set.prototype` methods. Only membership changes invalidate the actual
owner document's styles. Clone/import creates independent state collections.
Tests force collection for detached owners, connected elements and adopted elements
whose creating iframe has retired.

`:state(<ident>)` accepts one CSS identifier, including escaped, non-ASCII and
CSS-wide keyword names. The argument is case sensitive; the pseudo-class name is
ASCII case insensitive. CSS input normalizes CRLF, CR and form feed to LF, and replaces NUL and unpaired
surrogates with U+FFFD, following [CSS input preprocessing](https://drafts.csswg.org/css-syntax-3/#input-preprocessing);
this does not normalize the strings stored by `CustomStateSet`. DOM queries,
`matches`, `CSS.supports`, stylesheet matching and CSSOM serialization share the
same decoded name. Native matching requires a successfully customized element;
a retained collection on a failed upgrade does not make `:state()` match.

Selectors support relational/filtered sibling matching, `:host(:state(...))`, and
states on exposed shadow parts. Host and part state names refer to their respective
elements. Part suffixes also resolve existing before/after, first-line, first-letter,
marker and before/after marker styles. Structural pseudo-classes, `:has`, `:scope`
and `:host` never match a part target, including inside logical pseudo-classes.
The implementation follows [CSS Pseudo-Elements section 5](https://drafts.csswg.org/css-pseudo-4/#element-like).

The seven original WPT cases in `tests/wpt/custom-state.json` are also in the normal
manifest. Fetch them with `scripts/fetch-wpt.sh`; they use the repository's pinned
revision. Neither upstream assertions nor timeout limits are changed. The upstream
GC helper uses best-effort collection; native force-collection tests provide the
separate GC regression coverage.

```sh
cargo test --locked --test css_custom_states
WPT_REQUIRED=1 WPT_MANIFEST=tests/wpt/custom-state.json cargo test --locked --test wpt_smoke -- --nocapture
```

Firefox 158.0.1 matched 9 of 11 strict comparison contracts, including the
separate input, serialization, transfer and property descriptor contracts. It differed on
`add`'s return value and the `keys`/`values` alias. A separate diagnostic also found
that Firefox matches state names during precustomized and failed upgrade phases.
These differences are preserved rather than counted as successful comparisons.
The implementation uses the current [HTML CustomStateSet definition](https://html.spec.whatwg.org/multipage/custom-elements.html#customstateset),
[Web IDL setlike rules](https://webidl.spec.whatwg.org/#idl-setlike), and
[DOM custom element states](https://dom.spec.whatwg.org/#concept-element-custom).

This change does not complete Issue #1278 or #1269. Other selector families and
broader pseudo-element chains, including file-selector-button/details-content and
nested element-backed pseudos, remain part of those issues. CSSOM's omission of
some unset initial values (observed with opacity) also remains; the part structural
regression uses an explicit shadow stylesheet baseline so forbidden selectors must
still fail to override it.
