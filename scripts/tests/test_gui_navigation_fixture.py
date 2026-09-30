"""Verify the GUI navigation fixture serves distinct pages and logs requests."""

from html.parser import HTMLParser
from pathlib import Path
import re
import sys
import threading
import unittest
import urllib.error
import urllib.request

sys.path.insert(0, str(Path(__file__).parents[1]))
from gui_navigation_fixture import NavigationFixture, load_expectations  # noqa: E402


def fetch(url):
    with urllib.request.urlopen(url, timeout=5) as response:
        return response.status, response.read().decode()


class TitleParser(HTMLParser):
    def __init__(self):
        super().__init__()
        self.title = ""
        self._in_title = False

    def handle_starttag(self, tag, attrs):
        self._in_title = tag == "title"

    def handle_endtag(self, tag):
        self._in_title = False

    def handle_data(self, data):
        if self._in_title:
            self.title += data


def background_rgb(html):
    match = re.search(r"background:\s*rgb\((\d+),\s*(\d+),\s*(\d+)\)", html)
    return [int(v) for v in match.groups()]


class NavigationFixtureTests(unittest.TestCase):
    def test_pages_match_expectations_and_are_distinct(self):
        expectations = load_expectations()
        colours = []
        with NavigationFixture() as fixture:
            for path, page in expectations["pages"].items():
                status, html = fetch(fixture.url(path))
                self.assertEqual(status, 200)
                parser = TitleParser()
                parser.feed(html)
                self.assertEqual(parser.title, page["title"])
                self.assertEqual(background_rgb(html), page["background_rgb"])
                self.assertNotRegex(html, r"(?i)(src|href)=|<script|url\(")
                colours.append(tuple(page["background_rgb"]))
        self.assertEqual(len(set(colours)), len(colours))

    def test_requests_are_recorded_in_order_and_unrelated_ones_separated(self):
        with NavigationFixture() as fixture:
            fetch(fixture.url("/start.html"))
            with self.assertRaises(urllib.error.HTTPError) as error:
                fetch(fixture.url("/favicon.ico"))
            self.assertEqual(error.exception.code, 404)
            fetch(fixture.url("/destination.html?x=1"))
            self.assertEqual(fixture.page_requests(), ["/start.html", "/destination.html"])
            self.assertEqual([r.path for r in fixture.unrelated_requests()], ["/favicon.ico"])
            self.assertEqual([r.sequence for r in fixture.requests()], [0, 1, 2])
            self.assertTrue(all(r.method == "GET" for r in fixture.requests()))

    def test_runs_are_isolated_and_release_resources(self):
        threads_before = threading.active_count()
        with NavigationFixture() as first, NavigationFixture() as second:
            self.assertNotEqual(first.port, second.port)
            fetch(first.url("/start.html"))
            self.assertEqual(second.requests(), [])
            port = first.port
        with self.assertRaises(urllib.error.URLError):
            fetch(f"http://127.0.0.1:{port}/start.html")
        self.assertEqual(threading.active_count(), threads_before)

    def test_wait_for_request_observes_arrival_or_times_out(self):
        with NavigationFixture() as fixture:
            threading.Timer(0.1, fetch, [fixture.url("/destination.html")]).start()
            self.assertEqual(fixture.wait_for_request("/destination.html", 5).path, "/destination.html")
            with self.assertRaises(TimeoutError):
                fixture.wait_for_request("/start.html", 0.2)


if __name__ == "__main__":
    unittest.main()
