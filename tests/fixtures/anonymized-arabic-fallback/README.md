# Arabic fallback and embedded font controls (#675)

The system-font inputs reproduce Arabic missing glyphs with Liberation Sans as
the primary face. That face does not cover Arabic. Previously the default
fallback list contained only Liberation Sans and CJK faces on Linux. Adding
DejaVu Sans / Noto Arabic candidates lets the existing shaping path produce one
contextual RTL run. macOS candidates also include Geeza Pro. No shaper changes
are needed for the reproduced word.

`DejaVuSans.ttf` is DejaVu Sans 2.37, with SHA-256
`abdc775b21b1bc470d50c97e790d276f2054b7504e56e5bd3e64f48d68582322`.
The accompanying license covers this unmodified font. The controlled tests build
a font database from repository fixtures, so installed fonts cannot make the
fallback tests pass accidentally. They check nonzero contextual glyph IDs,
descending RTL clusters, bitmap coverage and Latin/Arabic/CJK run ownership.

The four HTML inputs are the original mixed paragraph, a smaller system case,
a data-URL second-family control, and a data-URL primary-family control. The last
control exposed an additional bug: font loading sent data URLs through HTTP URL
resolution and skipped them. In addition, unquoted URL payloads were tokenized
as CSS numbers, changing leading zeros and long digit runs inside base64 data.
The tokenizer now keeps unquoted URL arguments intact. The shared data-URL decoder feeds the font
loader directly, with the existing decoded font-size limit. A render test with
an empty system database requires both layout and paint to select the web face.
That test also exposed the `font` shorthand dropping its family name. Parsing
the shorthand before slash/comma boundaries are lost now retains the requested
family list, size, line height and variants, including the fixed textarea font.

The system and second-family control use the same DejaVu bytes in this host;
equal pixels alone cannot establish which source was selected. The primary
control and explicit face diagnostics make the embedded-font path observable.
Claims of white output from the earlier investigation were not reproduced:
saved and freshly captured PNGs contain nonwhite glyph pixels, and DOM rectangles
are nonzero. The diagnosed problem was the ignored font source.

Reproduce from the repository root (set `GECKODRIVER` if needed):

```sh
python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/anonymized-arabic-fallback --output tests/output/arabic
OMOIKANE_LIBRARY=/absolute/path/to/libomoikane.so \
  python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/anonymized-arabic-fallback --output tests/output/arabic
cargo test --locked --lib font::arabic_tests -- --nocapture
cargo test --locked --lib paint::font_data_url_tests -- --nocapture
OMOIKANE_ARABIC_FONT_REPORT=/tmp/arabic-host-fonts.json \
  cargo test --locked --lib default_fonts_shape_arabic_in_a_contextual_fallback_run
```

Reference environment: Linux ARM64, Firefox 155.0.1 / geckodriver 0.37.1,
800x600 CSS pixels, DPR 1, 2026-09-11. Both browsers navigate to the same local
HTML; Firefox waits for fonts and two animation frames. Each image is captured
twice. Before is main at `c6ab6e8409a0d215ed8e19a6892761bd968e6910`, via the
saved equivalent PR #679 library; after is this change.

The reference Firefox chooses DejaVu Sans for Arabic, Liberation Sans for Latin
in the system case, and IPAMincho for Japanese. Omoikane chooses the same Arabic
file but uses its CJK fallback policy (the observed host run selects Noto Sans CJK JP). The report
retains the file hashes and actual face diagnostics. Glyph rasterization and
baseline differences remain visible and are tracked in #677; this is not a
claim that arbitrary bidi text or every installed-font combination is equivalent.

`omoikane-after-runs.json` records the actual default fallback shaper's selected
files, hashes, glyph IDs and clusters. The Arabic run uses the DejaVu file above
and has no missing glyphs. `primary-before-diagnostics.txt` and
`primary-after-diagnostics.txt` separately show the primary embedded-font
control changing from system Liberation Sans to web AuditArabic in layout and paint.

All four after cases have stable repeat images and exact outer rectangles.
Full-image differences from Firefox are 2.713% (original paragraph), 0.542%
(system and second-family controls) and 0.532% (primary web-font control).
These descriptive numbers do not replace the glyph/face assertions or visual
inspection. `comparison.json` and `provenance.json` preserve measurements and
inputs. PNGs are lossless copies; `lossless-before.json` / `lossless-after.json`
record pixel identity with the original captures retained under
`/workspace/.artifacts/issues673-675/final-captures/arabic-fallback/`.
