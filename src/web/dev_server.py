"""Serves one web build to a local browser (docs/35, development; the launcher serves builds from W5 on).

    py -I src/web/dev_server.py <web build folder> [--port 8770]

  /                 the page (src/web/index.html; its scripts beside this file: boot.js, input.js, saves.js,
                    io-worker.js, audio-worklet.js, gpu.js, gpu-worker.js)
  /game.js, /game.wasm, /data.json, /bootset.json   the build folder (nxrs-build build --target web)
  /data/<path>      the build's <name>_data folder, with HTTP range requests (exact 206 + Content-Range)
  POST /log         the page's engine log lines, appended to --log (development)

Every response carries COOP/COEP (shared memory and threads need a cross-origin isolated page). Binds 127.0.0.1 only.
"""
import argparse
import http.server
import re
import sys
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
PAGE = {'boot.js', 'input.js', 'saves.js', 'io-worker.js', 'audio-worklet.js', 'gpu.js', 'gpu-worker.js'}
TYPES = {'.wasm': 'application/wasm', '.js': 'text/javascript', '.html': 'text/html', '.json': 'application/json'}


def make_handler(build: Path, data: Path, log_file=None):
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
            if p in ('/', '/index.html'):
                return HERE / 'index.html'
            if p.lstrip('/') in PAGE:
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
            # /log: the page's engine log lines (gpu.js), appended to --log when given (development: read by tools)
            n = int(self.headers.get('Content-Length', '0'))
            body = self.rfile.read(n)
            if self.path == '/log' and log_file is not None:
                with open(log_file, 'ab') as fh:
                    fh.write(body)
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
    ap.add_argument('--log', help='append the page\'s engine log to this file')
    a = ap.parse_args()
    build = Path(a.build).resolve()
    datas = [d for d in build.iterdir() if d.is_dir() and d.name.endswith('_data')]
    if not (build / 'game.js').is_file() or not (build / 'data.json').is_file() or len(datas) != 1:
        sys.exit(f'{build}: not a web build (game.js, data.json and one <name>_data folder)')
    srv = http.server.ThreadingHTTPServer(('127.0.0.1', a.port), make_handler(build, datas[0], a.log))
    print(f'serving {build} on http://127.0.0.1:{a.port}/', flush=True)
    srv.serve_forever()


if __name__ == '__main__':
    main()
