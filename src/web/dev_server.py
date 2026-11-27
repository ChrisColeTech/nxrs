"""Serves one web build to a local browser (docs/35, for engine developers; players use the launcher's Play in
browser, or `nxrs --serve-web <folder>`, which serve the same folder the same way).

    py -I src/web/dev_server.py <web build folder> [--port 8770] [--live]

  /                 the page (the build's index.html)
  /<file>           the build folder: the page and its workers, game.js, game.wasm, game.cold.wasm, data.json,
                    bootset.json, zdata/ (compressed data extents, read by range); a whole-file GET that accepts gzip
                    gets the build's <file>.gz with Content-Encoding: gzip; files carry an ETag and are revalidated
  /data/<path>      the data folder build.json names (<name>_data), with HTTP range requests (exact 206 +
                    Content-Range); the GPU worker's shader files (data/shaders/spv/<hex>.spv|.json) included
  POST /log         the page's engine log lines, appended to --log (development)
  POST /shot?n=N    a presented image (?shots=N on the page), saved beside --log (development)
  POST /shader-xlate/<16 hex key>
                    a shader the game made at run time (WEB-W7b; body: u32 LE ctl length, ctl, code, at most 4 MiB):
                    translated by `nxrs-build shader-xlate <data>/shaders` (--engine, else NXRS_BUILD, else this
                    tree's target/release/nxrs-build) into <data>/shaders/live/<key>/, answered with
                    u32 LE spv length, spv, json; 503 without an nxrs-build, 422 with its reason. With --log, the
                    request is also kept beside the log as xlate-<key>.bin (a recorded input for the launcher's test)

--live serves the page and its scripts (index.html, boot.js, input.js, saves.js, io-worker.js, audio-worklet.js,
gpu.js, gpu-worker.js, menu.js, menu.css) from src/web beside this file instead of the build's copies, to try a JS change without
building again. A build from before W5 (no build.json) is served that way too.

Every response carries COOP/COEP (shared memory and threads need a cross-origin isolated page). Binds 127.0.0.1 only.
"""
import argparse
import http.server
import json
import os
import re
import subprocess
import sys
import threading
import time
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
PAGE = {'index.html', 'boot.js', 'input.js', 'saves.js', 'io-worker.js', 'audio-worklet.js', 'gpu.js',
        'gpu-worker.js', 'menu.js', 'menu.css'}
TYPES = {'.wasm': 'application/wasm', '.js': 'text/javascript', '.html': 'text/html', '.json': 'application/json',
         '.css': 'text/css'}
MAX_XLATE = 4 << 20
XLATE_LOCK = threading.Lock()


def xlate(engine, data: Path, key: str, body: bytes):
    """(status, answer) of one shader translation (the launcher's services/web_xlate.rs, the same rules)."""
    n = int.from_bytes(body[:4], 'little') if len(body) >= 4 else 0
    if n == 0 or 4 + n >= len(body):
        return 400, b'not a shader request (u32 ctl length, ctl, code)'
    shaders = data / 'shaders'
    spv = shaders / 'live' / key / 'spv'

    def answer():
        s, j = spv / f'{key}.spv', spv / f'{key}.json'
        if s.is_file() and j.is_file():
            b = s.read_bytes()
            return len(b).to_bytes(4, 'little') + b + j.read_bytes()
        return None

    t0 = time.time()
    with XLATE_LOCK:
        got = answer()
        what = 'already translated'
        if got is None and engine is None:
            return 503, b'this server has no nxrs-build to translate shaders with'
        if got is None:
            p = subprocess.run([str(engine), 'shader-xlate', str(shaders)], input=body, capture_output=True,
                               timeout=120)
            printed = [l.strip() for l in p.stdout.decode(errors='replace').splitlines() if l.strip()]
            if p.returncode != 0:
                return 422, p.stderr.strip()
            if not printed or printed[-1].lower() != key:
                return 422, f'nxrs-build translated key {printed[-1] if printed else "?"} for {key}'.encode()
            got, what = answer(), 'translated'
            if got is None:
                return 422, p.stdout.strip()
    print(f'shader-xlate {key}: {what} in {time.time() - t0:.2f} s', flush=True)
    return 200, got


def make_handler(build: Path, data: Path, live: bool, log_file=None, engine=None):
    class H(http.server.BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'

        def log_message(self, *a):
            pass

        def headers_common(self, ctype, length, etag=None):
            self.send_header('Content-Type', ctype)
            self.send_header('Content-Length', str(length))
            self.send_header('Accept-Ranges', 'bytes')
            self.send_header('Cross-Origin-Opener-Policy', 'same-origin')
            self.send_header('Cross-Origin-Embedder-Policy', 'require-corp')
            # files: kept by the browser and revalidated (a warm start is 304s, and Chrome keeps the compiled wasm)
            if etag:
                self.send_header('Cache-Control', 'no-cache')
                self.send_header('ETag', etag)
                self.send_header('Vary', 'Accept-Encoding')
            else:
                self.send_header('Cache-Control', 'no-store')

        def resolve(self):
            p = self.path.split('?', 1)[0]
            if p == '/':
                p = '/index.html'
            if live and p.lstrip('/') in PAGE:
                return HERE / p.lstrip('/')
            if p.startswith('/data/'):
                rel = p[len('/data/'):]
                base = data
            else:
                rel = p.lstrip('/')
                base = build
            rel = urllib.parse.unquote(rel)
            f = (base / rel).resolve()
            return f if f.is_file() and base.resolve() in f.parents else None

        def do_HEAD(self):
            self.send(head=True)

        def do_POST(self):
            n = int(self.headers.get('Content-Length', '0'))
            # /shader-xlate/<key>: a run-time shader to translate (WEB-W7b); a body past the limit is refused unread
            m = re.match(r'/shader-xlate/([^/?]*)$', self.path)
            if m:
                key = m.group(1).lower()
                if n > MAX_XLATE:
                    status, out = 413, b'not a shader request (u32 ctl length, ctl, code)'
                    self.close_connection = True
                elif not re.fullmatch(r'[0-9a-f]{16}', key):
                    self.rfile.read(n)
                    status, out = 400, b'not a shader key'
                else:
                    body = self.rfile.read(n)
                    if log_file is not None:
                        (Path(log_file).parent / f'xlate-{key}.bin').write_bytes(body)
                    status, out = xlate(engine, data, key, body)
                self.send_response(status)
                self.headers_common('application/octet-stream' if status == 200 else 'text/plain', len(out))
                self.end_headers()
                self.wfile.write(out)
                return
            # /log: the page's engine log lines (gpu.js), appended to --log when given (development: read by tools)
            body = self.rfile.read(n)
            if self.path == '/log' and log_file is not None:
                with open(log_file, 'ab') as fh:
                    fh.write(body)
            # /shot?n=N: a presented image from the GPU worker (?shots=N on the page), saved beside --log
            m = re.match(r'/shot\?n=(\d+)$', self.path)
            if m and log_file is not None:
                (Path(log_file).parent / f'shot-{int(m.group(1)):06d}.png').write_bytes(body)
            self.send_response(204)
            self.headers_common('text/plain', 0)
            self.end_headers()

        def do_GET(self):
            self.send(head=False)

        def send(self, head):
            f = self.resolve()
            if f is None:
                self.send_response(404)
                self.headers_common('text/plain', 0)
                self.end_headers()
                return
            ctype = TYPES.get(f.suffix, 'application/octet-stream')
            rng = self.headers.get('Range', '')
            # the build's gzip copy (<file>.gz, src/cli/src/webzip.rs) for a whole-file GET that accepts gzip; a
            # range always gets the identity bytes
            gz = f.with_name(f.name + '.gz')
            enc = None
            if not rng and 'gzip' in self.headers.get('Accept-Encoding', '') and gz.is_file() \
                    and gz.stat().st_mtime >= f.stat().st_mtime:
                f, enc = gz, 'gzip'
            st = f.stat()
            size = st.st_size
            etag = f'"{size:x}-{st.st_mtime_ns:x}{"-gz" if enc else ""}"'
            if not rng and self.headers.get('If-None-Match') == etag:
                self.send_response(304)
                self.send_header('ETag', etag)
                self.send_header('Cache-Control', 'no-cache')
                self.send_header('Vary', 'Accept-Encoding')
                self.send_header('Cross-Origin-Opener-Policy', 'same-origin')
                self.send_header('Cross-Origin-Embedder-Policy', 'require-corp')
                self.send_header('Content-Length', '0')
                self.end_headers()
                return
            m = re.match(r'bytes=(\d*)-(\d*)$', rng)
            lo, hi = 0, size - 1
            if m and (m.group(1) or m.group(2)):
                if m.group(1):
                    lo = int(m.group(1))
                    hi = int(m.group(2)) if m.group(2) else size - 1
                else:
                    lo = max(0, size - int(m.group(2)))
                hi = min(hi, size - 1)
                if lo > hi:
                    self.send_response(416)
                    self.send_header('Content-Range', f'bytes */{size}')
                    self.headers_common('text/plain', 0)
                    self.end_headers()
                    return
                self.send_response(206)
                self.send_header('Content-Range', f'bytes {lo}-{hi}/{size}')
            else:
                self.send_response(200)
            n = hi - lo + 1
            self.headers_common(ctype, n, etag)
            if enc:
                self.send_header('Content-Encoding', enc)
            self.end_headers()
            if head:
                return
            with open(f, 'rb') as fh:
                fh.seek(lo)
                while n > 0:
                    chunk = fh.read(min(n, 1 << 20))
                    if not chunk:
                        break
                    self.wfile.write(chunk)
                    n -= len(chunk)

    return H


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('build')
    ap.add_argument('--port', type=int, default=8770)
    ap.add_argument('--live', action='store_true', help='the page and its scripts from src/web')
    ap.add_argument('--log', help='append the page\'s engine log to this file')
    ap.add_argument('--engine', help='the nxrs-build that translates run-time shaders (default: NXRS_BUILD, else '
                                     'this tree\'s target/release/nxrs-build)')
    a = ap.parse_args()
    exe = 'nxrs-build.exe' if os.name == 'nt' else 'nxrs-build'
    engine = next((Path(p) for p in [a.engine, os.environ.get('NXRS_BUILD'), HERE.parent.parent / 'target' / 'release'
                                     / exe] if p and Path(p).is_file()), None)
    build = Path(a.build).resolve()
    described = build / 'build.json'
    if described.is_file():
        # a W5 build: complete, its data folder named by build.json
        data = build / json.loads(described.read_text(encoding='utf-8'))['data']
        if not data.is_dir():
            sys.exit(f'{build}: build.json names {data.name}, which is missing')
        live = a.live
    else:
        datas = [d for d in build.iterdir() if d.is_dir() and d.name.endswith('_data')]
        if not (build / 'game.js').is_file() or not (build / 'data.json').is_file() or len(datas) != 1:
            sys.exit(f'{build}: not a web build (build.json, or game.js, data.json and one <name>_data folder)')
        data, live = datas[0], True
    srv = http.server.ThreadingHTTPServer(('127.0.0.1', a.port), make_handler(build, data, live, a.log, engine))
    print(f'serving {build} on http://127.0.0.1:{a.port}/' + (' (page from src/web)' if live else ''), flush=True)
    print(f'run-time shaders translated with {engine}' if engine else 'no nxrs-build: run-time shaders are not '
          'translated', flush=True)
    srv.serve_forever()


if __name__ == '__main__':
    main()
