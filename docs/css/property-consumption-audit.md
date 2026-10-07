# CSS property consumption audit

Tracking: [#1272](https://github.com/ieee0824/omoikane/issues/1272).
Source inspected: `3de13b20`, 2026-10-07. The registry contains 237 longhand
entries, including vendor aliases. Recognition and valid computed values do
not by themselves establish a layout, paint, or native-input implementation.

The audit searches tracked production source for each CSS name and its
`PropertyId` variant, then follows dynamic property names, canonical aliases,
logical-to-physical conversion and intermediate layout/paint structures.
Tests, registry declarations, grammar validation and CSSOM serialization alone
are not evidence of a rendering consumer. The following gaps have no consumer
in the inspected layout, paint, font or platform-input paths.

| Recognized property | Missing behavior / source evidence | Follow-up |
| --- | --- | --- |
| `word-spacing` | Inline text shaping and advance accumulation never read this property. | [#1273](https://github.com/ieee0824/omoikane/issues/1273) |
| `text-indent` | Inline line construction never reads the paragraph indent. | [#1273](https://github.com/ieee0824/omoikane/issues/1273) |
| `text-decoration-style` | `PropagatedTextDecoration` carries line, color, thickness and underline geometry, but no decoration style; text paint draws the captured geometry without dashed/dotted/double/wavy selection. | [#1273](https://github.com/ieee0824/omoikane/issues/1273) |
| `list-style-image` | Marker construction reads list style type and position, but does not resolve or paint the marker image. | [#1274](https://github.com/ieee0824/omoikane/issues/1274) |
| `outline-width`, `outline-style`, `outline-offset`, `outline-color` | Paint has border and text-decoration paths, but no outline geometry/paint pass. The visited-color handling of outline-color is only a value restriction, not an outline renderer. | [#1274](https://github.com/ieee0824/omoikane/issues/1274) |
| `cursor` | The GUI routes cursor position and Pointer Lock visibility/grab, but does not map computed cursor values to a native cursor icon or custom cursor image. | [#1274](https://github.com/ieee0824/omoikane/issues/1274) |

These nine entries retain their existing syntax/serialization support. #1272
does not claim to implement their visual effects or remove supported grammar
to hide the gaps. The linked implementation issues own the missing consumers.

## Indirect consumers checked

Several apparent gaps from literal-name searches are implemented indirectly:

| Property group | Consumption path |
| --- | --- |
| `-webkit-mask-position-x/y`, `-webkit-mask-repeat/size` | `canonical_property_name` maps aliases to mask longhands; `src/paint/mod.rs` and `src/paint/image.rs` consume canonical mask geometry. |
| `border-{top,right,bottom,left}-style` | `src/paint/border.rs` builds the property name from the side. |
| `grid-column-start/end`, `grid-row-start/end` | `src/layout/grid.rs::axis_request` builds `{axis}-start/end`. |
| `margin-block/inline-start/end`, `padding-block/inline-start/end` | `src/css/logical.rs` maps the logical side before physical box layout. |
| `animation-*` | Style animation sampling consumes typed `PropertyId` values and produces the computed style read by layout/paint; runtime animation state lives in `src/css/style/animation.rs` and `src/css/animation.rs`. |

A consumer reference establishes reachability only, not support for every
standard value. In particular, advanced animation behavior remains tracked by
[#1281](https://github.com/ieee0824/omoikane/issues/1281), and grid coverage by
[#1270](https://github.com/ieee0824/omoikane/issues/1270). This audit is not a
conformance claim for the whole registry.

The full regression run also identified an implemented property missing from
the registry: `table-layout`, consumed by `src/layout/table.rs`. The shared
registry now includes it with the `auto` / `fixed` grammar and `auto` initial
value. Existing fixed-table geometry tests cover its actual layout consumer;
an additional test fixes feature detection and invalid-value rejection.
`page-orientation`, found by the same consumer search, is an `@page` descriptor
validated separately by `src/css/style/page.rs`, not an element longhand.

The integration with main `fffface3` also checks the P1 additions `order`,
`grid-auto-columns`, `grid-auto-rows` and `grid-auto-flow`: flex layout reads
order when constructing the item sequence, and grid layout consumes the three
auto-placement properties. These additions do not introduce another missing
consumer in the table above.

Implemented shorthands containing `var()` remain pending until substitution,
so their original names must also be recognized. The registry now includes
the implemented deferred shorthands, including physical border sides, margin,
padding, background, font and flex. The fixed-revision WPT
`css/css-variables/variable-substitution-shorthands.html` exposed this gap when
a CSSOM write to `border-left` was incorrectly rejected. A direct regression
test checks the border width, style and color after that write.

Retained shorthand metadata is validated through its expanded longhands before
the cascade uses it. This preserves internal consumers of simple border and
animation values while preventing malformed aggregate values from falling
through to the generic first-token conversion. Regression cases cover excess
border-radius/background-position components and a third animation time value.

CSSOM rule declaration views cache their parsed and validated declarations by
the source block. Writes replace that source and invalidate the cache, so
repeated reads avoid revalidating an unchanged block. A mutation regression
checks cssText replacement, valid and invalid property writes, and removal.
