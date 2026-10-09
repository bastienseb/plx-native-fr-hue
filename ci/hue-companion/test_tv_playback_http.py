import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'pkg/hue-companion'))

import http.client
from http.server import ThreadingHTTPServer
import threading
import unittest
from unittest.mock import Mock

import tv_color_test

class PlaybackHttpTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(('127.0.0.1', 0), tv_color_test.Handler)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.thread.join(timeout=2)

    def setUp(self):
        self.original = tv_color_test.playback
        tv_color_test.playback = self.lease = Mock()
        self.lease.command.return_value = {'active': True}

    def tearDown(self):
        tv_color_test.playback = self.original

    def request(self, method='POST', path='/api/playback/start', headers=None):
        connection = http.client.HTTPConnection(*self.server.server_address, timeout=2)
        try:
            connection.request(method, path, headers=headers or {})
            response = connection.getresponse()
            response.read()
            return response.status
        finally:
            connection.close()

    def test_local_native_request_forwards_only_its_session(self):
        headers = {'X-LG-Hue-Test': '1', 'X-LG-Hue-Session': '42-1000'}
        self.assertEqual(self.request(headers=headers), 200)
        self.lease.command.assert_called_once_with('start', '42-1000')

    def test_missing_header_is_refused_before_any_command(self):
        self.assertEqual(self.request(), 403)
        self.lease.command.assert_not_called()

    def test_remote_web_origin_is_refused_even_with_headers(self):
        headers = {'X-LG-Hue-Test': '1', 'X-LG-Hue-Session': '42-1000', 'Origin': 'https://example.invalid'}
        self.assertEqual(self.request(headers=headers), 403)
        self.lease.command.assert_not_called()

    def test_get_cannot_start_playback(self):
        self.assertEqual(self.request(method='GET', headers={'X-LG-Hue-Test': '1'}), 403)
        self.lease.command.assert_not_called()

    def test_expired_session_is_a_conflict(self):
        self.lease.command.side_effect = RuntimeError('Session Hue expirée')
        self.assertEqual(self.request(path='/api/playback/heartbeat', headers={'X-LG-Hue-Test': '1'}), 409)

    def test_existing_color_status_remains_available(self):
        self.assertEqual(self.request(method='GET', path='/api/test/status', headers={'X-LG-Hue-Test': '1'}), 200)
        self.lease.command.assert_not_called()

if __name__ == '__main__':
    unittest.main()
