# OpenType font controls Firefox comparison

This fixture compares the OpenType shaping and deterministic synthesis paths
against Firefox using the repository's production-browser capture harness.  It
uses the checked-in Liberation Sans font so both engines receive identical font
bytes.  The rows cover normal text, small/all-small caps, super/subscript,
explicit `smcp` disablement, and disabled caps synthesis.

Capture and summarize it with:

```sh
GECKODRIVER=<geckodriver> python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/anonymized-opentype-fonts --serve-root tests/fixtures \
  --output <output>
OMOIKANE_LIBRARY=<libomoikane.so> python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/anonymized-opentype-fonts --serve-root tests/fixtures \
  --output <output>
python3 scripts/summarize-firefox-rendering.py \
  --fixtures tests/fixtures/anonymized-opentype-fonts --output <output>
```

The issue #1493 development capture is retained outside the repository under
`.artifacts/issue1269/issue1493-firefox-comparison/`.
