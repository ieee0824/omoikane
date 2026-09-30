# GUI navigation fixture

Two self-contained pages for the native GUI URL-entry E2E (#1131, #1132).
Each page fills the whole page area with one solid colour, so a screenshot can
tell them apart without text rendering. `expectations.json` lists each served
path, its colour and the page-area region to sample.

Serve them with `scripts/gui_navigation_fixture.py`:

```python
from gui_navigation_fixture import NavigationFixture

with NavigationFixture() as fixture:
    start = fixture.url("/start.html")          # pass to the GUI
    destination = fixture.url("/destination.html")  # type into the URL bar
    ...
    assert fixture.page_requests() == ["/start.html", "/destination.html"]
    fixture.unrelated_requests()  # e.g. /favicon.ico, recorded separately
```

Each `NavigationFixture` binds its own `127.0.0.1:0` server, so runs never
share request logs; leaving the `with` block stops the server and joins its
thread. The self-test is `python3 -m unittest discover -s scripts/tests -p
test_gui_navigation_fixture.py`.
