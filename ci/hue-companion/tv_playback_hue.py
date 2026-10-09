"""Loopback playback lease: own only the Hue changes made for one native player.

The heartbeat is independent of capture and uses no bridge credentials. A missed
heartbeat or service restart restores the daemon state recorded before playback.
"""
import json
from pathlib import Path
import re
import threading
import time

LEASE_SECONDS = 20
STATE = Path('/var/home/root/lg-hue-sync/playback-lease.json')
TOKEN = re.compile(r'[0-9]{1,12}-[0-9]{1,30}\Z')

class PlaybackLease:
    def __init__(self, daemon, lock, test_busy, state=STATE, clock=time.monotonic):
        self.daemon = daemon
        self.lock = lock
        self.test_busy = test_busy
        self.state = state
        self.clock = clock
        self.owner = None
        self.previous = None
        self.deadline = 0
        self.latest = None

    def _persist(self):
        temporary = self.state.with_suffix('.tmp')
        temporary.write_text(json.dumps(self.previous), encoding='utf-8')
        temporary.chmod(0o600)
        temporary.replace(self.state)

    def recover(self):
        with self.lock:
            if self.state.exists():
                self.previous = json.loads(self.state.read_text(encoding='utf-8'))
                self._restore()

    def _restore(self):
        if not self.previous:
            self.owner = None
            return
        previous = self.previous
        # Read current settings so brightness, area and user edits remain intact.
        status = self.daemon('status')
        if previous['changed_hue'] and status['settings']['hue_sync_enabled']:
            settings = dict(status['settings'])
            settings['hue_sync_enabled'] = False
            self.daemon('settings', 'POST', settings)
        if previous['started_daemon']:
            self.daemon('stop', 'POST')
        # Preserve the recovery record on IO failure, allowing the watchdog to retry.
        self.state.unlink(missing_ok=True)
        self.previous = None
        self.owner = None

    def active(self):
        with self.lock:
            return bool(self.previous)

    def command(self, action, token):
        if not TOKEN.fullmatch(token):
            raise RuntimeError('Session Hue invalide')
        with self.lock:
            if action == 'stop':
                if token == self.owner:
                    self._restore()
                return {'active': self.owner is not None}
            if action == 'heartbeat':
                if token != self.owner:
                    raise RuntimeError('Session Hue expirée')
                self.deadline = self.clock() + LEASE_SECONDS
                return {'active': True}
            if action != 'start':
                raise RuntimeError('Commande Hue inconnue')
            if self.test_busy():
                raise RuntimeError('Terminez le test de couleurs avant la lecture avec Hue')
            # A cancelled old worker may reach the socket after a newer Play.
            # Refuse its older token from the same native process.
            if self.latest:
                pid, stamp = token.split('-')
                old_pid, old_stamp = self.latest.split('-')
                if pid == old_pid and int(stamp) < int(old_stamp):
                    raise RuntimeError('Ancienne session Hue refusée')
            if token == self.owner:
                self.deadline = self.clock() + LEASE_SECONDS
                return {'active': True}
            if self.previous:
                self._restore()
            status = self.daemon('status')
            if not status.get('hue_zones'):
                raise RuntimeError('Sélectionnez une zone Hue dans l’application de synchronisation')
            self.previous = {'changed_hue': not status['settings']['hue_sync_enabled'],
                             'started_daemon': not status['is_syncing']}
            # Save BEFORE changing the daemon, including the partially-started case.
            self._persist()
            self.owner = token
            self.latest = token
            self.deadline = self.clock() + LEASE_SECONDS
            try:
                if self.previous['changed_hue']:
                    settings = dict(status['settings'])
                    settings['hue_sync_enabled'] = True
                    self.daemon('settings', 'POST', settings)
                if self.previous['started_daemon']:
                    self.daemon('start', 'POST')
            except Exception:
                self._restore()
                raise RuntimeError('La synchronisation Hue ne démarre pas') from None
            return {'active': True, 'lease_seconds': LEASE_SECONDS}

    def expire(self):
        with self.lock:
            if self.previous and self.clock() >= self.deadline:
                self._restore()

    def close(self):
        with self.lock:
            self._restore()

    def watch(self, stop):
        while not stop.wait(1):
            try:
                self.expire()
            except Exception:
                # Keep retrying restoration; do not log configuration or credentials.
                pass

    def start_watchdog(self):
        stop = threading.Event()
        threading.Thread(target=self.watch, args=(stop,), daemon=True).start()
        return stop
