"""Bind fixture ports atomically and publish their actual values before serving."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import socket
import sys
import tempfile
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    'wptserve_launcher', Path(__file__).resolve().parents[2] / 'tests/wpt_smoke/wptserve.py')
launcher = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(launcher)


class Settings(dict):
    def __getattr__(self, name):
        return self[name]


class BoundSocket:
    def __init__(self, port):
        self.socket = socket.socket()
        try:
            self.socket.bind(('127.0.0.1', port))
        except BaseException:
            self.socket.close()
            raise
        self.server_address = self.socket.getsockname()
        self.port = self.server_address[1]
        self.closed = False

    def server_close(self):
        self.socket.close()
        self.closed = True

    def shutdown(self):
        pass


class WptserveStartupTests(unittest.TestCase):
    def launch(self, directory, occupied, *, fail_bind=False, fail_start=False):
        initial = Settings(
            ports={'http': [occupied, occupied], 'https': [occupied, occupied],
                   'ws': [0], 'wss': [occupied]},
            ssl_config={'cert_path': 'cert', 'key_path': 'key', 'ca_cert_path': 'ca'},
            domains_set={'web-platform.test'})
        native = SimpleNamespace(config=None)
        sockets, http, requests, started_configs = [], [], [], []

        @contextlib.contextmanager
        def configuration(*_):
            yield initial

        def start_websocket(port, *_):
            requests.append(port)
            instance = BoundSocket(port)
            sockets.append(instance)
            return instance

        case = self

        class Http(BoundSocket):
            def __init__(self, **arguments):
                if fail_bind and len(http) == 2:
                    raise OSError('injected bind failure')
                requests.append(arguments['port'])
                super().__init__(arguments['port'])
                sockets.append(self)
                http.append(self)
                self.httpd = self
                self.started = False
                native.config = arguments['config']

            def start(self):
                if fail_start and http.index(self) == 1:
                    raise RuntimeError('injected start failure')
                ports = native.config.ports
                actual = [instance.port for instance in http]
                case.assertEqual(ports['http'] + ports['https'], actual)
                case.assertTrue(all(port > 0 for port in actual))
                started_configs.append(ports)
                self.started = True

            def stop(self):
                if self.started:
                    self.server_close()
                self.started = False

        wptserve = ModuleType('wptserve')
        wptserve.config = SimpleNamespace(Config=Settings)
        wptserve.handlers = SimpleNamespace(handler=lambda function: function)
        wptserve.routes = SimpleNamespace(routes=[])
        wptserve.server = SimpleNamespace(
            Http1WebTestRequestHandler=object, WebTestHttpd=Http, Server=native)
        wptserve.stash = SimpleNamespace(StashServer=contextlib.nullcontext)
        ready = Path(directory) / 'ready.json'
        argv = ['launcher', '--root', directory, '--revision', 'revision',
                '--ready-file', str(ready)]
        modules = {'localpaths': ModuleType('localpaths'), 'wptserve': wptserve,
                   'websocket_cookie': SimpleNamespace(start=start_websocket)}
        try:
            with patch.dict(sys.modules, modules), patch.object(sys, 'argv', argv), \
                    patch.object(sys, 'stdin', io.StringIO('stop\n')), \
                    patch.object(sys, 'stdout', io.StringIO()), \
                    patch.object(sys, 'path', list(sys.path)), \
                    patch.object(launcher, 'configuration', configuration), \
                    patch.object(launcher, 'check_revision'), \
                    patch.object(launcher, 'generated_worker_routes', return_value=[]), \
                    patch.object(launcher, 'adapter_routes', return_value=[]):
                launcher.main()
        finally:
            self.assertTrue(all(instance.closed for instance in sockets))
            self.assertFalse(any(instance.started for instance in http))
        return json.loads(ready.read_text()), initial, native.config, requests, started_configs

    def test_occupied_probed_ports_are_not_reused_and_templates_use_bound_ports(self):
        with tempfile.TemporaryDirectory() as directory, socket.socket() as occupied:
            occupied.bind(('127.0.0.1', 0))
            busy = occupied.getsockname()[1]
            data, original, configured, requests, starts = self.launch(directory, busy)
        ports = data['http_ports'] + data['https_ports'] + data['wss_ports']
        self.assertEqual(requests, [0] * 5)
        self.assertEqual(len(set(ports)), 5)
        self.assertNotIn(busy, ports)
        self.assertEqual(configured.ports['http'], data['http_ports'])
        self.assertEqual(configured.ports['https'], data['https_ports'])
        self.assertEqual(configured.ports['wss'], data['wss_ports'])
        self.assertEqual(configured.ports['ws'], [0])
        self.assertEqual(original.ports['http'], [busy, busy])
        self.assertEqual(len(starts), 4)
        self.assertEqual(data['base_url'], f"https://web-platform.test:{data['https_ports'][0]}")

    def test_bind_failure_closes_bound_listeners_without_publishing_ready(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(OSError, 'injected bind failure'):
                self.launch(directory, 0, fail_bind=True)
            self.assertFalse((Path(directory) / 'ready.json').exists())

    def test_start_failure_closes_started_and_unstarted_listeners(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(RuntimeError, 'injected start failure'):
                self.launch(directory, 0, fail_start=True)
            self.assertFalse((Path(directory) / 'ready.json').exists())


if __name__ == '__main__':
    unittest.main()
