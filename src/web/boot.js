// The page side of a web build (docs/35 W2): checks what the build needs, shows the engine's log, gives the game its
// data folder as /data, and starts game.js (Emscripten; main runs on a worker thread of its own, PROXY_TO_PTHREAD).
// W2: the data folder is fetched whole into the in-memory file system before main (Emscripten's file system runs on
// this page's thread, where synchronous range reads are not allowed); the I/O worker of docs/35 W3 replaces this with
// reads on demand.
'use strict';
(() => {
  const status = document.getElementById('status');
  const logEl = document.getElementById('log');
  const lines = [];
  let flush = 0;
  const log = (text, red) => {
    // also in the console: readable while this thread is busy serving the game's file system
    console.log('[nxrs] ' + text);
    lines.push([text, red || /\[RED\]|FATAL|panicked|Aborted/.test(text)]);
    if (!flush) {
      flush = requestAnimationFrame(() => {
        flush = 0;
        const frag = document.createDocumentFragment();
        for (const [t, r] of lines.splice(0)) {
          const d = document.createElement('div');
          d.textContent = t;
          if (r) d.className = 'red';
          frag.appendChild(d);
        }
        logEl.appendChild(frag);
        logEl.scrollTop = logEl.scrollHeight;
      });
    }
  };
  window.NXRS_LOG = lines;
  const fail = (why) => {
    status.textContent = why;
    status.className = 'red';
    log(why, true);
  };

  // what a web build needs from the browser
  if (!self.crossOriginIsolated) {
    return fail('this page is not cross-origin isolated (COOP/COEP): threads and shared memory are unavailable');
  }
  try {
    new WebAssembly.Memory({ initial: 1n, maximum: 2n, address: 'i64' });
  } catch (e) {
    return fail('this browser has no 64-bit WebAssembly memory (memory64): ' + e);
  }

  (async () => {
    status.textContent = 'reading the file list';
    const files = await (await fetch('files.json')).json();
    const t0 = performance.now();
    window.Module = {
      arguments: ['/data'],
      print: (t) => log(t),
      printErr: (t) => log(t),
      preRun: [() => {
        const M = window.Module;
        const FS = M.FS || globalThis.FS;
        M.addRunDependency('data');
        const total = files.reduce((n, f) => n + f[1], 0);
        let done = 0;
        let next = 0;
        const one = async () => {
          while (next < files.length) {
            const [path] = files[next++];
            const r = await fetch('data/' + path.split('/').map(encodeURIComponent).join('/'));
            if (!r.ok) throw new Error(`data/${path}: HTTP ${r.status}`);
            const bytes = new Uint8Array(await r.arrayBuffer());
            const dir = ('/data/' + path).replace(/\/[^/]*$/, '');
            FS.mkdirTree(dir);
            FS.writeFile('/data/' + path, bytes);
            done += bytes.length;
            status.textContent = `loading data ${(done / 2 ** 20).toFixed(0)} / ${(total / 2 ** 20).toFixed(0)} MiB`;
          }
        };
        Promise.all(Array.from({ length: 6 }, one)).then(() => {
          FS.mkdirTree('/data/save');
          log(`[web] ${files.length} data files (${(total / 2 ** 20).toFixed(0)} MiB) in /data, ${((performance.now() - t0) / 1000).toFixed(1)} s`);
          status.textContent = 'running';
          M.removeRunDependency('data');
        }, (e) => fail('data: ' + e));
      }],
      onAbort: (what) => fail('aborted: ' + what),
      onExit: (code) => {
        status.textContent = `exited (${code}) after ${((performance.now() - t0) / 1000).toFixed(1)} s`;
      },
      setStatus: (t) => { if (t) status.textContent = t; },
    };
    status.textContent = 'loading game.wasm';
    const s = document.createElement('script');
    s.src = 'game.js';
    s.onerror = () => fail('game.js failed to load');
    document.body.appendChild(s);
  })().catch((e) => fail('boot: ' + e));
})();
