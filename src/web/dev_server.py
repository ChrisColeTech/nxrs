"""Serves one web build to a local browser (docs/35, for engine developers; players use the launcher's Play in
browser, or `nxrs --serve-web <folder>`, which serve the same folder the same way).

    py -I src/web/dev_server.py <web build folder> [--port 8770] [--live]

  /                 the page (the build's index.html)
  /<file>           the build folder: the page and its workers, game.js, game.wasm, data.json, bootset.json
  /data/<path>      the data folder build.json names (<name>_data), with HTTP range requests (exact 206 +
                    Content-Range)

--live serves the page and its scripts (index.html, boot.js, input.js, saves.js, io-worker.js, audio-worklet.js)
from src/web beside this file instead of the build's copies, to try a JS change without building again. A build
from before W5 (no build.json) is served that way too.

Every response carries COOP/COEP (shared memory and threads need a cross-origin isolated page). Binds 127.0.0.1 only.
"""
import argparse
import http.server
import json
import re
import sys
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
PAGE = {'index.html', 'boot.js', 'input.js', 'saves.js', 'io-worker.js', 'audio-worklet.js'}
TYPES = {'.wasm': 'application/wasm', '.js': 'text/javascript', '.html': 'text/html', '.json': 'application/json'}


def make_handler(build: Path, data: Path, live: bool):
    class H(http.server.BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'

        def log_message(self, *a):
            pass

        def headers_common(self, ctype, length):
            self.send_header('Content-Type', ctype)
            self.send_header('Content-Length', str(length))
            self.send_header('Accept-Ranges', 'bytes')
            self.send_header('Cross-Origin-Opener-Policy', 'same-origin')
            self.send_header('Cross-Origin-Embedder-Policy', 'require-corp')
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

        def do_GET(self):
            self.send(head=False)

        def send(self, head):
            f = self.resolve()
            if f is None:
                self.send_response(404)
                self.headers_common('text/plain', 0)
                self.end_headers()
                return
            size = f.stat().st_size
            ctype = TYPES.get(f.suffix, 'application/octet-stream')
            m = re.match(r'bytes=(\d*)-(\d*)$', self.headers.get('Range', ''))
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
            self.headers_common(ctype, n)
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
    a = ap.parse_args()
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
    srv = http.server.ThreadingHTTPServer(('127.0.0.1', a.port), make_handler(build, data, live))
    print(f'serving {build} on http://127.0.0.1:{a.port}/' + (' (page from src/web)' if live else ''), flush=True)
    srv.serve_forever()


if __name__ == '__main__':
    main()
