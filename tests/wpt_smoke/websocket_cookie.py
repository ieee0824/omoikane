"""Fixture-only TLS WebSocket endpoint for the pinned storage-access test."""

import base64
import hashlib
import socketserver
import ssl
import struct
import threading


class CookieHandler(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.settimeout(10)
        header = bytearray()
        while not header.endswith(b"\r\n\r\n"):
            chunk = self.request.recv(1)
            if not chunk or len(header) >= 32768:
                return
            header.extend(chunk)
        lines = header.decode("latin-1").split("\r\n")
        if lines[0] != "GET /echo-cookie HTTP/1.1":
            return
        fields = {}
        for line in lines[1:]:
            if ":" in line:
                name, value = line.split(":", 1)
                fields[name.lower()] = value.strip()
        key = fields.get("sec-websocket-key", "")
        if len(base64.b64decode(key, validate=True)) != 16:
            return
        accept = base64.b64encode(hashlib.sha1(
            (key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode("ascii")
        ).digest())
        self.request.sendall(b"HTTP/1.1 101 Switching Protocols\r\n"
                             b"Upgrade: websocket\r\nConnection: Upgrade\r\n"
                             b"Sec-WebSocket-Accept: " + accept + b"\r\n\r\n")
        payload = fields.get("cookie", "").encode("utf-8")
        if len(payload) < 126:
            frame = bytes((0x81, len(payload)))
        elif len(payload) < 65536:
            frame = b"\x81\x7e" + struct.pack("!H", len(payload))
        else:
            frame = b"\x81\x7f" + struct.pack("!Q", len(payload))
        self.request.sendall(frame + payload)
        try:
            self.request.recv(4096)
        except TimeoutError:
            pass


class CookieServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True

    def __init__(self, port, certificate, key):
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(certificate, key)
        self.context = context
        super().__init__(("127.0.0.1", port), CookieHandler)

    def get_request(self):
        connection, address = super().get_request()
        return self.context.wrap_socket(connection, server_side=True), address


def start(port, certificate, key):
    server = CookieServer(port, certificate, key)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server
