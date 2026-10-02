# Japanese HTML encoding fixture

These original, local pages reproduce #1224 without external network requests.
`*.sjis.html` contain actual Shift_JIS bytes; `*.utf8.html` are the same markup
and text in UTF-8. Each declares its encoding with `meta http-equiv`, while the
test server sends only `Content-Type: text/html`. The frame and its link resolve
relative URLs so both encodings follow the same nested navigation path.

`header.sjis.html` deliberately has a conflicting UTF-8 meta declaration. Its
HTTP response supplies `charset=Shift_JIS`, which must take precedence.

To regenerate the Shift_JIS fixtures after editing the UTF-8 references:

```python
from pathlib import Path
root = Path('tests/fixtures/legacy-encoding')
for name in ('index', 'page', 'next'):
    source = (root / f'{name}.utf8.html').read_text(encoding='utf-8')
    (root / f'{name}.sjis.html').write_bytes(
        source.replace('charset=UTF-8', 'charset=Shift_JIS').encode('shift_jis'))
(root / 'header.sjis.html').write_bytes(
    (root / 'page.utf8.html').read_text(encoding='utf-8').encode('shift_jis'))
```

The regression compares decoded body/title values and complete rendered RGBA
buffers against the UTF-8 references, including frame-link navigation and direct
navigation. Japanese system fallback glyphs must exist and rasterize visibly;
Linux test hosts need `fonts-noto-cjk`, while macOS uses installed Hiragino fonts.
With `OMOIKANE_BROWSER_REPORT_DIR`, the test saves PNGs and the selected Japanese
font metadata. The saved buffers are those used by the native GUI rendering path;
the test does not inspect OS window decorations.
