#!/usr/bin/env python3
"""Isolated HTTP fixture and request log for the native GUI navigation E2E.

The fixture serves the pages in tests/fixtures/gui-navigation from a fresh
127.0.0.1:0 server and records every request, so a test can confirm that the
GUI really fetched a page independently of what the browser reports.
"""

from dataclasses import dataclass
import http.server
import json
from pathlib import Path
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
FIXTURE_DIR = ROOT / "tests/fixtures/gui-navigation"


def load_expectations(fixture_dir=FIXTURE_DIR):
    """Return the parsed expectations.json describing each page."""
    return json.loads((fixture_dir / "expectations.json").read_text())


@dataclass(frozen=True)
class RecordedRequest:
    """One request received by the fixture, in arrival order."""

    sequence: int
    method: str
    path: str
    known_page: bool


class NavigationFixture:
    """Context manager owning one server, its thread and its request log."""

    def __init__(self, fixture_dir=FIXTURE_DIR):
        self.expectations = load_expectations(fixture_dir)
        self._bodies = {
            path: (fixture_dir / page["file"]).read_bytes()
            for path, page in self.expectations["pages"].items()
        }
        self._requests = []
        self._lock = threading.Condition()
        self._server = None
        self._thread = None

    def __enter__(self):
        fixture = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                path = self.path.split("?", 1)[0]
                body = fixture._record(self.command, path)
                if body is None:
                    self.send_error(404)
                    return
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(body)))
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_):
                pass

        self._server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._server.daemon_threads = True
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()
        return self

    def __exit__(self, *_):
        self._server.shutdown()
        self._server.server_close()
        self._thread.join(timeout=5)

    def _record(self, method, path):
        body = self._bodies.get(path)
        with self._lock:
            self._requests.append(RecordedRequest(len(self._requests), method, path, body is not None))
            self._lock.notify_all()
        return body

    @property
    def port(self):
        return self._server.server_port

    def url(self, path):
        """Absolute URL of `path` on this fixture's server."""
        return f"http://127.0.0.1:{self.port}{path}"

    def requests(self):
        """All requests so far, in arrival order."""
        with self._lock:
            return list(self._requests)

    def page_requests(self):
        """Paths of requests for known fixture pages, in arrival order."""
        return [r.path for r in self.requests() if r.known_page]

    def unrelated_requests(self):
        """Requests for paths that are not fixture pages (e.g. favicons)."""
        return [r for r in self.requests() if not r.known_page]

    def wait_for_request(self, path, timeout=20):
        """Block until `path` has been requested; return it or raise TimeoutError."""
        deadline = time.monotonic() + timeout
        with self._lock:
            while True:
                for request in self._requests:
                    if request.path == path:
                        return request
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError(f"{path} was not requested within {timeout}s")
                self._lock.wait(remaining)
