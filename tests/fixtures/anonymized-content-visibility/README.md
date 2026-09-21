# Content visibility rendering fixture (#702)

These fixed inputs cover the paint states of `content-visibility: visible`,
`hidden`, and `auto`, the hidden element's intrinsic-size placeholder, and an
offscreen automatic subtree after `scrollIntoView()` makes it relevant. Solid
colors avoid font rasterization differences. The hidden card must retain its
principal background and border while omitting its red child and generated
content. The generated content remains a paint sentinel; its independent block
flow geometry is tracked by
[#798](https://github.com/ieee0824/omoikane/issues/798) and is not used as a
geometry probe here.

Firefox 155.0.1 / geckodriver 0.37.1 and Omoikane were captured twice on
Linux ARM64 on 2026-09-20, at 500x280 and 500x240 CSS pixels. Each browser's
repeat images were byte-identical after RGB decoding. Both fixtures had zero
changed pixels, zero geometry delta for every probed element, and no reported
state difference between Firefox and Omoikane. Visual review confirmed that the
visible and automatic payloads paint, the hidden principal background and
border remain, and the hidden red child and `::before` do not paint. Raw images,
DOM measurements, hashes, the numeric comparison, and the losslessly compressed
review image are retained under
`/workspace/.artifacts/issue702-firefox-comparison-final4/` in the development
record.

Capture Firefox and Omoikane twice with
`scripts/compare-firefox-rendering.py`; write generated output outside this
fixture directory. `manifest.json` records the exact input hashes. Rust unit
and WPT regressions separately cover skip decisions, focus/selection,
geometry queries, DOM updates, and CSSOM grammar.
Omoikane does not yet expose a browser find-in-page surface; connecting search
matches to automatic-content relevance is tracked independently in
[#799](https://github.com/ieee0824/omoikane/issues/799).

## Verification

```sh
python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/anonymized-content-visibility --output /tmp/content-visibility-capture
OMOIKANE_LIBRARY=/absolute/path/to/libomoikane.so \
  python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/anonymized-content-visibility --output /tmp/content-visibility-capture
python3 scripts/summarize-firefox-rendering.py \
  tests/fixtures/anonymized-content-visibility --output /tmp/content-visibility-capture
```
