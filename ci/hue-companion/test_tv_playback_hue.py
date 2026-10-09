import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'pkg/hue-companion'))

import copy
import tempfile
import threading
import unittest

from tv_playback_hue import PlaybackLease

class PlaybackLeaseTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.state = Path(self.directory.name) / 'lease.json'
        self.now = 100
        self.busy = False
        self.status = {'is_syncing': False, 'hue_zones': [{'channel_id': 11}],
                       'settings': {'hue_sync_enabled': False, 'brightness_multiplier': 1.1,
                                    'nanoleaf_sync_enabled': True}}
        self.calls = []
        self.fail_start = False
        self.lease = self.new_lease()

    def tearDown(self):
        self.directory.cleanup()

    def new_lease(self):
        return PlaybackLease(self.daemon, threading.RLock(), lambda: self.busy,
                             self.state, lambda: self.now)

    def daemon(self, path, method='GET', body=None):
        self.calls.append((path, method))
        if path == 'status':
            return copy.deepcopy(self.status)
        if path == 'settings':
            self.status['settings'] = copy.deepcopy(body)
        elif path == 'start':
            if self.fail_start:
                raise OSError('simulated network error')
            self.status['is_syncing'] = True
        elif path == 'stop':
            self.status['is_syncing'] = False
        return {}

    def test_start_stop_restores_state_and_preserves_user_edits(self):
        self.lease.command('start', '42-1000')
        self.assertTrue(self.status['is_syncing'])
        self.assertTrue(self.status['settings']['hue_sync_enabled'])
        self.status['settings']['brightness_multiplier'] = 1.7
        self.lease.command('stop', '42-1000')
        self.assertFalse(self.status['is_syncing'])
        self.assertFalse(self.status['settings']['hue_sync_enabled'])
        self.assertEqual(self.status['settings']['brightness_multiplier'], 1.7)
        self.assertTrue(self.status['settings']['nanoleaf_sync_enabled'])
        self.assertFalse(self.state.exists())

    def test_prior_manual_sync_is_left_running(self):
        self.status['is_syncing'] = True
        self.status['settings']['hue_sync_enabled'] = True
        self.lease.command('start', '42-1000')
        self.lease.command('stop', '42-1000')
        self.assertTrue(self.status['is_syncing'])
        self.assertNotIn(('stop', 'POST'), self.calls)
        self.assertNotIn(('settings', 'POST'), self.calls)

    def test_nanoleaf_running_without_hue_is_restored(self):
        self.status['is_syncing'] = True
        self.lease.command('start', '42-1000')
        self.lease.command('stop', '42-1000')
        self.assertTrue(self.status['is_syncing'])
        self.assertFalse(self.status['settings']['hue_sync_enabled'])

    def test_crashed_client_expires(self):
        self.lease.command('start', '42-1000')
        self.now += 20
        self.lease.expire()
        self.assertFalse(self.status['is_syncing'])
        self.assertFalse(self.lease.active())

    def test_heartbeat_renews_without_restarting_capture(self):
        self.lease.command('start', '42-1000')
        self.now += 19
        self.lease.command('heartbeat', '42-1000')
        self.now += 19
        self.lease.expire()
        self.assertTrue(self.lease.active())
        self.assertEqual(self.calls.count(('start', 'POST')), 1)

    def test_old_stop_cannot_stop_new_playback(self):
        self.lease.command('start', '42-1000')
        self.lease.command('start', '42-1001')
        self.lease.command('stop', '42-1000')
        self.assertEqual(self.lease.owner, '42-1001')
        self.assertTrue(self.status['is_syncing'])

    def test_delayed_old_start_cannot_steal_new_playback(self):
        self.lease.command('start', '42-1001')
        with self.assertRaises(RuntimeError):
            self.lease.command('start', '42-1000')
        self.assertEqual(self.lease.owner, '42-1001')

    def test_wrong_heartbeat_cannot_renew(self):
        self.lease.command('start', '42-1000')
        with self.assertRaises(RuntimeError):
            self.lease.command('heartbeat', '41-1000')

    def test_color_test_and_unconfigured_area_refuse_playback(self):
        self.busy = True
        with self.assertRaises(RuntimeError):
            self.lease.command('start', '42-1000')
        self.busy = False
        self.status['hue_zones'] = []
        with self.assertRaises(RuntimeError):
            self.lease.command('start', '42-1000')
        self.assertNotIn(('start', 'POST'), self.calls)

    def test_partial_start_failure_restores_changes(self):
        self.fail_start = True
        with self.assertRaises(RuntimeError):
            self.lease.command('start', '42-1000')
        self.assertFalse(self.status['settings']['hue_sync_enabled'])
        self.assertFalse(self.lease.active())

    def test_service_restart_recovers_record(self):
        self.lease.command('start', '42-1000')
        recovered = self.new_lease()
        recovered.recover()
        self.assertFalse(self.status['is_syncing'])
        self.assertFalse(self.status['settings']['hue_sync_enabled'])
        self.assertFalse(self.state.exists())

    def test_malformed_token_and_unknown_action_are_refused(self):
        for action, token in [('start', '42\r\nInjected: 1'), ('oops', '42-1')]:
            with self.assertRaises(RuntimeError):
                self.lease.command(action, token)

if __name__ == '__main__':
    unittest.main()
