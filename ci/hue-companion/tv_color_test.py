#!/usr/bin/env python3
"""Owner-local Hue calibration, independent of the TV video capture plane.

Uses the existing paired configuration, pinned HTTPS and HueStream v2 DTLS.
The daemon is paused during a test and its prior running state is restored.
The loopback-only API accepts an explicit header; every session expires in 120s.
"""
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import signal
import ssl
import struct
import subprocess
import threading
import time
from tv_playback_hue import PlaybackLease

CONFIG = Path('/var/home/root/lg-hue-sync/config.json')
PATTERNS = {'quadrants', 'red', 'green', 'blue', 'white', 'warm-white'}
UNIFORM = {'red': (255, 0, 0), 'green': (0, 255, 0), 'blue': (0, 0, 255),
           'white': (255, 255, 255), 'warm-white': (255, 180, 107)}

def daemon(path, method='GET', body=None):
    connection = http.client.HTTPConnection('127.0.0.1', 8088, timeout=2)
    try:
        connection.request(method, '/api/' + path,
                           body=json.dumps(body) if body is not None else None,
                           headers={'Content-Type': 'application/json'})
        response = connection.getresponse()
        result = json.loads(response.read())
        if response.status != 200:
            raise RuntimeError('Le service de synchronisation ne répond pas')
        return result
    finally:
        connection.close()

def bridge(config, method, path, body=None):
    pin = config.get('hue_bridge_certificate_sha256')
    if not pin:
        raise RuntimeError('Certificat du Bridge non configuré')
    # Hue's local certificate is authenticated by the existing SHA-256 pin.
    connection = http.client.HTTPSConnection(config['bridge_ip'], timeout=6,
                                             context=ssl._create_unverified_context())
    try:
        connection.connect()
        fingerprint = hashlib.sha256(connection.sock.getpeercert(binary_form=True)).hexdigest()
        if fingerprint.lower() != pin.lower():
            raise RuntimeError('Le certificat du Bridge a changé')
        connection.request(method, path, body=json.dumps(body) if body else None,
                           headers={'hue-application-key': config['username'],
                                    'Content-Type': 'application/json'})
        response = connection.getresponse()
        result = json.loads(response.read())
        if response.status not in (200, 201) or result.get('errors'):
            raise RuntimeError('Le Bridge refuse le test de couleurs')
        return result
    finally:
        connection.close()

def color(pattern, x, y):
    if pattern != 'quadrants':
        return UNIFORM[pattern]
    if y < 0.33:
        return (255, 0, 0)
    if x > 0.66:
        return (0, 255, 0)
    if y > 0.66:
        return (0, 0, 255)
    return (255, 255, 0)

def packet(area_id, channels, pattern, sequence):
    data = b'HueStream' + bytes((2, 0, sequence % 256, 0, 0, 0, 0)) + area_id.lower().encode('ascii')
    for channel in channels:
        # Reuse the daemon's authoritative sampling zones and real v2 channel IDs.
        x, y = channel['screen_position']
        rgb = color(pattern, x, y)
        data += struct.pack('>BHHH', channel['channel_id'], *(component * 257 for component in rgb))
    return data

class Session:
    def __init__(self, pattern):
        self.pattern = pattern
        self.frames = 0
        self.channels = []
        self.stop_event = threading.Event()
        self.ready = threading.Event()
        self.error = None
        self.active = False
        self.thread = threading.Thread(target=self.run, daemon=True)

    def run(self):
        process = None
        config = None
        area_path = None
        acquired = False
        was_running = False
        paused = False
        try:
            config = json.loads(CONFIG.read_text())
            area_id = config.get('entertainment_configuration_id') or config['entertainment_area_id']
            if len(area_id) != 36:
                raise RuntimeError('Une zone Hue Entertainment v2 est nécessaire')
            area_path = '/clip/v2/resource/entertainment_configuration/' + area_id
            status = daemon('status')
            was_running = status['is_syncing']
            daemon('stop', 'POST')
            paused = True
            deadline = time.monotonic() + 8
            while daemon('status')['hue_connected']:
                if time.monotonic() > deadline:
                    raise RuntimeError('La synchronisation ne libère pas les lumières')
                if self.stop_event.wait(0.1):
                    return
            area = bridge(config, 'GET', area_path)['data'][0]
            if area.get('status') == 'active':
                raise RuntimeError('Une autre application utilise déjà cette zone Hue')
            self.channels = area['channels']
            zones = {zone['channel_id']: zone for zone in status['hue_zones']}
            if set(zones) != {channel['channel_id'] for channel in self.channels}:
                raise RuntimeError('Actualisez la zone Hue avant de lancer le test')
            for channel in self.channels:
                zone = zones[channel['channel_id']]
                channel['screen_position'] = ((zone['x_min'] + zone['x_max']) / 2,
                                              (zone['y_min'] + zone['y_max']) / 2)
            bridge(config, 'PUT', area_path, {'action': 'start'})
            acquired = True
            process = subprocess.Popen([
                '/usr/bin/openssl', 's_client', '-dtls1_2', '-brief', '-nocommands', '-ign_eof',
                '-connect', config['bridge_ip'] + ':2100',
                '-cipher', 'PSK-AES128-GCM-SHA256', '-psk_identity', config['username'],
                '-psk', config['clientkey']], stdin=subprocess.PIPE,
                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, bufsize=0)
            connected = threading.Event()
            def observe_handshake():
                for line in iter(process.stderr.readline, b''):
                    if b'CONNECTION ESTABLISHED' in line:
                        connected.set()
            threading.Thread(target=observe_handshake, daemon=True).start()
            deadline = time.monotonic() + 7
            while not connected.wait(0.05):
                if process.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError('La connexion chiffrée Hue ne démarre pas')
                if self.stop_event.is_set():
                    return
            self.active = True
            self.ready.set()
            deadline = time.monotonic() + 120
            while not self.stop_event.is_set() and time.monotonic() < deadline:
                process.stdin.write(packet(area_id, self.channels, self.pattern, self.frames))
                self.frames += 1
                if process.poll() is not None:
                    raise RuntimeError('La connexion Hue a été interrompue')
                self.stop_event.wait(0.05)
        except Exception as error:
            # Never return arbitrary exceptions containing configuration or PSK arguments.
            self.error = str(error) if isinstance(error, RuntimeError) else 'Le test Hue a échoué'
        finally:
            self.active = False
            if process:
                process.terminate()
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
                process.stdin.close()
                process.stderr.close()
            try:
                if acquired:
                    bridge(config, 'PUT', area_path, {'action': 'stop'})
            except Exception:
                self.error = 'Impossible de rendre le contrôle au Bridge'
            try:
                if paused and was_running:
                    daemon('start', 'POST')
            except Exception:
                self.error = 'Relancez la synchronisation après le test'
            self.ready.set()

class Controller:
    def __init__(self):
        self.session = None
        self.lock = threading.RLock()

    def status(self):
        session = self.session
        return {'active': bool(session and session.active),
                'pattern': session.pattern if session and session.active else None,
                'frames': session.frames if session else 0,
                'channels': len(session.channels) if session else 0,
                'error': session.error if session else None}

    def command(self, pattern):
        with self.lock:
            if pattern == 'off':
                if self.session:
                    self.session.stop_event.set()
                    self.session.thread.join(timeout=18)
                    if self.session.thread.is_alive():
                        raise RuntimeError('Arrêt du test en cours, réessayez')
                return self.status()
            if pattern not in PATTERNS:
                raise RuntimeError('Mire inconnue')
            if playback.active():
                raise RuntimeError('Quittez la lecture avec Hue avant le test de couleurs')
            if self.session and self.session.active:
                self.session.pattern = pattern
            else:
                if self.session and self.session.thread.is_alive():
                    raise RuntimeError('Le test précédent se termine')
                self.session = Session(pattern)
                self.session.thread.start()
                self.session.ready.wait(25)
                if not self.session.active:
                    self.session.stop_event.set()
                    self.session.thread.join(timeout=18)
                    raise RuntimeError(self.session.error or 'Le test ne démarre pas')
            return self.status()

controller = Controller()
playback = PlaybackLease(daemon, controller.lock,
                         lambda: bool(controller.session and controller.session.thread.is_alive()))

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def response(self, status, result):
        content = json.dumps(result).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(content)))
        self.send_header('Access-Control-Allow-Origin', 'null')
        self.send_header('Cache-Control', 'no-store')
        self.end_headers()
        self.wfile.write(content)

    def permitted(self):
        return self.headers.get('Origin') in (None, 'null') and self.headers.get('X-LG-Hue-Test') == '1'

    def do_OPTIONS(self):
        if self.headers.get('Origin') != 'null':
            self.response(403, {'error': 'Origin refused'})
            return
        self.send_response(204)
        self.send_header('Access-Control-Allow-Origin', 'null')
        self.send_header('Access-Control-Allow-Methods', 'GET, POST')
        self.send_header('Access-Control-Allow-Headers', 'X-LG-Hue-Test')
        self.send_header('Access-Control-Max-Age', '3600')
        self.end_headers()

    def do_GET(self):
        if not self.permitted() or self.path != '/api/test/status':
            self.response(403, {'error': 'Request refused'})
            return
        self.response(200, controller.status())

    def do_POST(self):
        if not self.permitted() or not self.path.startswith(('/api/test/', '/api/playback/')):
            self.response(403, {'error': 'Request refused'})
            return
        try:
            if self.path.startswith('/api/playback/'):
                result = playback.command(self.path.removeprefix('/api/playback/'),
                                          self.headers.get('X-LG-Hue-Session', ''))
            else:
                result = controller.command(self.path.removeprefix('/api/test/'))
            self.response(200, result)
        except RuntimeError as error:
            self.response(409, {'error': str(error)})
        except Exception:
            self.response(503, {'error': 'Service Hue temporairement indisponible'})

if __name__ == '__main__':
    try:
        playback.recover()
    except Exception:
        # The daemon may still be booting. The watchdog retries the saved restoration.
        pass
    playback_watchdog = playback.start_watchdog()
    server = ThreadingHTTPServer(('127.0.0.1', 8976), Handler)
    def shutdown(*_):
        playback_watchdog.set()
        playback.close()
        controller.command('off')
        raise SystemExit(0)
    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT, shutdown)
    server.serve_forever()
