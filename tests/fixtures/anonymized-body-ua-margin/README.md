# HTML body UA margin against Firefox (#797)

These fixed inputs isolate the HTML user-agent default `body { margin: 8px; }`
and an author `body { margin: 0; }` override. They use only solid colors, so
font selection and text rasterization do not affect the comparison.

Firefox 155.0.1 / geckodriver 0.37.1 and Omoikane were captured twice on
Linux ARM64 on 2026-09-21 at 500x160 CSS pixels and DPR 1. The default case
places both the body and marker at (8, 8); the author override places them at
(0, 0). Their rectangles match exactly in both browsers. Decoded RGB output
also matches at every pixel, and both browsers' repeat captures are identical.
The measurements, original PNGs, hashes, and numeric comparison are retained
under `/workspace/.artifacts/issue797/firefox-comparison-final/` in the
development record.

Capture Firefox and Omoikane twice with the repository comparison scripts;
write generated images and measurements outside this fixture directory.

```sh
GECKODRIVER=/absolute/path/to/geckodriver \
  python3 scripts/compare-firefox-rendering.py firefox \
  --fixtures tests/fixtures/anonymized-body-ua-margin --output /tmp/body-ua-margin
OMOIKANE_LIBRARY=/absolute/path/to/libomoikane.so \
  python3 scripts/compare-firefox-rendering.py omoikane \
  --fixtures tests/fixtures/anonymized-body-ua-margin --output /tmp/body-ua-margin
python3 scripts/summarize-firefox-rendering.py \
  tests/fixtures/anonymized-body-ua-margin --output /tmp/body-ua-margin
```
