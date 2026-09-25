# Find-in-page reference fixture

`basic.html` has unique words in normal text, offscreen
`content-visibility:auto`, `content-visibility:hidden`, `display:none`, an
open shadow root, and a same-document iframe. It also has two normal matches
for order/count checks.
`inline.html` and `block.html` distinguish a match across adjacent inline
elements from text separated by block boundaries.
`slot.html` fixes the order of two assigned text nodes in an open shadow root.

Run `GECKODRIVER=/path/to/geckodriver python3 tests/fixtures/find-in-page/firefox_probe.py`
with Firefox and Selenium. The probe uses Firefox's `window.find` and records
selection and scroll after a short delay. This is a reference for content
search behavior; it does not exercise Firefox's browser chrome search bar.

On Firefox 155.0.1 headless (2026-09-25), `ordinarytoken`, `autotoken`,
`shadowtoken`, and `frametoken` were found. `hiddentoken` and `nonetoken` were
not. Finding `autotoken` scrolled the window from `0` to `1242` CSS pixels and
selected the word. `frametoken` required `searchInFrames=true` and selected
text in the child frame. The machine-readable output is committed as
[firefox-baseline.json](firefox-baseline.json); run the probe to refresh it.
The inline phrase `helloworld` was found, while the same phrase split across
two paragraphs was not.
The assigned slot words were found once each in first-then-second order.
An inserted `addedtoken` was found, then disappeared from search after its
paragraph was removed. Clearing the selection left no selected text.
