"""Launch the pinned upstream HTTP/HTTPS servers; keep original WPT handlers."""
import argparse
import json
import logging
from pathlib import Path
import ssl
import subprocess
import sys


def configuration(root, config):
    certificates = root / "tools" / "certs"
    return config.ConfigBuilder(
        logging.getLogger("omoikane-wpt"),
        browser_host="web-platform.test", server_host="127.0.0.1",
        doc_root=str(root), subdomains={"www", "www1", "www2"},
        alternate_hosts={"alt": "not-web-platform.test"},
        ports={"http": ["auto", "auto"], "https": ["auto", "auto"],
               "ws": [0], "wss": ["auto"]},
        ssl={"type": "pregenerated", "encrypt_after_connect": False,
             "pregenerated": {
                 "host_key_path": str(certificates / "web-platform.test.key"),
                 "host_cert_path": str(certificates / "web-platform.test.pem"),
                 "ca_cert_path": str(certificates / "cacert.pem")}},
    )


def check_revision(root, revision):
    command = ["git", "-c", "safe.directory=" + str(root), "-C", str(root)]
    actual = subprocess.check_output(command + ["rev-parse", "HEAD"], text=True).strip()
    if actual != revision:
        raise RuntimeError(f"WPT revision mismatch: expected {revision}, got {actual}")
    subprocess.run(command + ["diff", "--quiet", "HEAD", "--"], check=True)


def adapter_routes(handlers):
    directory = Path(__file__).parent
    routes = []
    for target, filename in [
        ("/resources/testharnessreport.js", "testharnessreport.js"),
        ("/resources/testdriver-vendor.js", "testdriver_vendor.js"),
        ("/common/sab.js", "sab.js"),
    ]:
        content = (directory / filename).read_bytes()

        @handlers.handler
        def adapter(request, response, body=content):
            return [(b"Content-Type", b"text/javascript; charset=utf-8")], body

        routes.append(("GET", target, adapter))
    return routes


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--ready-file", type=Path, required=True)
    args = parser.parse_args()
    root = args.root.resolve()
    check_revision(root, args.revision)
    sys.path[:0] = [str(root), str(root / "tools")]
    import localpaths  # noqa: F401: pinned vendored dependency paths
    from wptserve import config, handlers, routes, server, stash
    import websocket_cookie
    class ClosingHttpHandler(server.Http1WebTestRequestHandler):
        def finish(self):
            super().finish()
            if isinstance(self.connection, ssl.SSLSocket):
                # Streaming upstream files may be close-delimited. Authenticate
                # their end with TLS close_notify rather than accepting ragged EOF.
                self.connection.settimeout(1)
                try:
                    self.connection.unwrap()
                except OSError:
                    # The peer may close after receiving our close_notify.
                    pass

    with stash.StashServer(), configuration(root, config) as settings:
        servers = []
        websocket_server = None
        try:
            websocket_server = websocket_cookie.start(
                settings.ports["wss"][0],
                settings.ssl_config["cert_path"],
                settings.ssl_config["key_path"],
            )
            for scheme in ("http", "https"):
                for port in settings.ports[scheme]:
                    instance = server.WebTestHttpd(
                        host="127.0.0.1", port=port, doc_root=str(root),
                        handler_cls=ClosingHttpHandler,
                        routes=adapter_routes(handlers) + routes.routes,
                        rewrites=[("GET", "/resources/WebIDLParser.js", "/resources/webidl2/lib/webidl2.js")],
                        config=settings, use_ssl=scheme == "https",
                        key_file=settings.ssl_config["key_path"],
                        certificate=settings.ssl_config["cert_path"],
                    )
                    instance.start()
                    servers.append(instance)
            description = {
                "base_url": f"https://web-platform.test:{settings.ports['https'][0]}",
                "http_ports": settings.ports["http"],
                "https_ports": settings.ports["https"],
                "wss_ports": settings.ports["wss"],
                "hosts": sorted(settings.domains_set),
                "ca_certificate": settings.ssl_config["ca_cert_path"],
            }
            temporary = args.ready_file.with_suffix(".tmp")
            temporary.write_text(json.dumps(description), encoding="utf-8")
            temporary.replace(args.ready_file)
            # Endpoint diagnostics must never fill the metadata/control pipe.
            sys.stdout = sys.stderr
            for line in sys.stdin:
                if line.strip() == "stop":
                    break
        finally:
            for instance in reversed(servers):
                instance.stop()
            if websocket_server is not None:
                websocket_server.shutdown()
                websocket_server.server_close()


if __name__ == "__main__":
    main()
