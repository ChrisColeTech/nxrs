// The page side of a web build (docs/35): checks what the build needs, logs the engine to the console (the page itself
// is only the game's canvas and a one-line notice, WEB-WPAGE), starts the I/O worker
// and the boot-set prefetch, restores the saves, puts the small host files in the in-memory file system, and starts
// game.js (Emscripten; main runs on a worker thread of its own, PROXY_TO_PTHREAD). The engine then hands the page its
// shared blocks (src/web/nxrs-lib.js -> Module.nxrsHost): the I/O slots (to the I/O worker), the audio ring (to the
// AudioWorklet), the input block (to input.js). One function per subsystem below; the page never blocks.
//
// Game data (RomFS, ExeFS) is never copied into the in-memory file system: the engine reads it through the I/O worker
// (io-worker.js), which fetches it by HTTP range into an OPFS cache. URL options: ?env=NAME=value,... sets engine
// variables (e.g. NXRS_CONTROLLER_TRACE=1); ?fresh=1 empties the data cache first; ?bootset=0 skips the prefetch.
'use strict';
(() => {
  const T0 = performance.now();
  const secs = () => ((performance.now() - T0) / 1000).toFixed(2);
  const watchers = [];
  const log = makeLog();
  const notice = makeNotice();
  let failed = false;
  const fail = (why) => {
    failed = true;
    notice.show(why, true);
    log('[RED] ' + why);
  };

  // WEB-WPAGE: the engine's log goes to the console only (and to the server, gpu.js). The page keeps the last
  // LOG_KEEP lines in window.NXRS_LOG for drivers (newest last) and calls the watchers with every line.
  function makeLog() {
    const LOG_KEEP = 5000;
    const lines = [];
    window.NXRS_LOG = lines;
    return (text) => {
      console.log('[nxrs] ' + text);
      for (const w of watchers) w(text);
      lines.push(text);
      if (lines.length > LOG_KEEP + 1000) lines.splice(0, lines.length - LOG_KEEP);
    };
  }

  // the one line the player sees: loading, the click for sound, a failure (which stays)
  function makeNotice() {
    const el = document.getElementById('notice');
    return {
      show(text, error) {
        if (failed && !error) return;
        el.textContent = text;
        el.className = error ? 'error' : '';
      },
      clear(text) {
        if (!failed && (text === undefined || el.textContent === text)) el.textContent = '';
      },
    };
  }

  // the picture replaces the loading line; a lost GPU device is a failure the player must see
  addEventListener('nxrs-gpu', (e) => {
    if (e.detail.first) notice.clear('Loading');
    if (e.detail.fatal) fail(e.detail.fatal);
  });

  // the page's title is the game's name (build.json, beside this page)
  fetch('build.json', { cache: 'no-store' })
    .then((r) => (r.ok ? r.json() : null))
    .then((b) => { if (b && b.title) document.title = b.title; })
    .catch(() => {});

  function checks() {
    if (!self.crossOriginIsolated) {
      fail('This page is not cross-origin isolated (COOP/COEP): threads and shared memory are unavailable. Open it from the nxrs launcher or a server that sends those headers.');
      return false;
    }
    if (!navigator.gpu) {
      fail('This browser has no WebGPU. Use a current Chrome or Edge.');
      return false;
    }
    try {
      new WebAssembly.Memory({ initial: 1n, maximum: 2n, address: 'i64', shared: true });
    } catch (e) {
      fail('This browser has no shared 64-bit WebAssembly memory (memory64): ' + e);
      return false;
    }
    // Chrome on Windows rounds short Atomics.wait timeouts up to the 15.6 ms system tick unless a short timer is
    // pending somewhere in the renderer
    setInterval(() => {}, 1);
    return true;
  }

  // ---- data: the I/O worker
  function startIo(manifest, bootset) {
    const worker = new Worker('io-worker.js');
    const stats = []; // waiting for the worker's answers, in order
    const ready = new Promise((resolve, reject) => {
      worker.onerror = (e) => reject(new Error('io-worker.js: ' + (e.message || e)));
      worker.onmessage = (e) => {
        const m = e.data;
        if (m.log) log('[web] ' + m.log);
        if (m.ready) resolve(m.ready);
        if (m.stats) stats.shift()?.(m.stats);
      };
    });
    worker.postMessage({ init: { files: manifest.files, version: manifest.version, title: manifest.title, bootset, fresh: manifest.fresh } });
    addEventListener('pagehide', () => worker.postMessage({ close: true }));
    return {
      ready,
      start: (memory, table) => worker.postMessage({ start: { memory, table } }),
      stats: () => new Promise((r) => { stats.push(r); worker.postMessage({ stats: true }); }),
    };
  }

  // the data folder's small files (control data, icon, tables) for the engine's host-file readers; game data excluded
  async function hostFiles(FS, manifest, text) {
    const small = manifest.files.filter(([p, n]) => !/^(romfs|exefs)\//.test(p) && n <= 4 << 20);
    let bytes = 0;
    await Promise.all(small.map(async ([p]) => {
      const r = await fetch('data/' + p.split('/').map(encodeURIComponent).join('/'));
      if (!r.ok) throw new Error(`data/${p}: HTTP ${r.status}`);
      const b = new Uint8Array(await r.arrayBuffer());
      FS.mkdirTree(('/data/' + p).replace(/\/[^/]*$/, ''));
      FS.writeFile('/data/' + p, b);
      bytes += b.length;
    }));
    FS.mkdirTree('/data');
    FS.writeFile('/data/data.json', text);
    return [small.length, bytes];
  }

  // ---- audio: the AudioWorklet, started at the first click or key (the browser's autoplay rule)
  function audioHost() {
    let ring = null;
    let ctx = null;
    let gesture = false;
    const start = async () => {
      if (!ring || !gesture) return;
      if (ctx) {
        if (ctx.state !== 'running') ctx.resume().catch(() => {});
        return;
      }
      const h = new Int32Array(ring.memory.buffer, ring.at, 16);
      try {
        ctx = new AudioContext({ sampleRate: 48000, latencyHint: 'interactive' });
        await ctx.audioWorklet.addModule('audio-worklet.js');
        const node = new AudioWorkletNode(ctx, 'nxrs-ring', {
          numberOfInputs: 0,
          outputChannelCount: [2],
          processorOptions: { memory: ring.memory, ring: ring.at, frames: ring.frames },
        });
        node.connect(ctx.destination);
        const state = () => {
          Atomics.store(h, 7, ctx.sampleRate);
          Atomics.store(h, 6, ctx.state === 'running' ? 1 : 2);
          log(`[web] audio: AudioContext ${ctx.state} at ${ctx.sampleRate} Hz`);
        };
        ctx.onstatechange = state;
        await ctx.resume();
        state();
      } catch (e) {
        log(`[RED] [web] audio: the AudioWorklet did not start: ${e}`);
        ctx = null;
      }
    };
    const HINT = 'Click or press a key for sound';
    const onGesture = () => {
      gesture = true;
      notice.clear(HINT);
      start();
    };
    addEventListener('pointerdown', onGesture, true);
    addEventListener('keydown', onGesture, true);
    return (memory, at, frames) => {
      ring = { memory, at, frames };
      log(`[web] audio: the engine's ring at ${at} (${frames} frames)${gesture ? '' : '; sound starts at the first click or key'}`);
      // the hint replaces the loading line; the first picture does not remove it, the first click or key does
      if (!gesture) notice.show(HINT);
      start();
    };
  }

  // ---- code: the modules kept in Cache Storage. Chrome's HTTP cache does not store an entry this large (its disk
  // cache refuses one over an eighth of its size; game.wasm.gz is 107 MB), so the server's ETag was never asked
  // about and every launch downloaded the module again (WEB-W6). The page keeps the response itself and asks the
  // server with If-None-Match: a 304 (no body) uses the kept copy, a 200 (a changed build: its ETag is the file's
  // size and time) replaces it. Without Cache Storage (a private window, storage blocked) it is a plain fetch.
  const CODE_CACHE = 'nxrs-code-1';
  async function codeResponse(file) {
    const url = new URL(file, location.href).href;
    let cache = null;
    let kept = null;
    try {
      cache = await caches.open(CODE_CACHE);
      kept = await cache.match(url);
    } catch {
      cache = null;
    }
    const tag = kept && kept.headers.get('ETag');
    const r = await fetch(url, { cache: 'no-store', headers: tag ? { 'If-None-Match': tag } : {} });
    if (r.status === 304 && kept) return { response: kept, from: 'kept copy (304)' };
    if (!r.ok) throw new Error(`${file}: HTTP ${r.status}`);
    if (cache && r.headers.get('ETag')) {
      const copy = r.clone();
      cache.put(url, copy).catch((e) => log(`[web] code cache: ${file} not kept (${e})`));
    }
    return { response: r, from: kept ? 'changed on the server, fetched' : 'fetched' };
  }

  // Module.instantiateWasm: game.wasm through the code cache, compiled while it streams
  function instantiateWasm(imports, done) {
    const t0 = performance.now();
    codeResponse('game.wasm')
      .then(({ response, from }) => WebAssembly.instantiateStreaming(response, imports).then((r) => {
        log(`[web] game.wasm: ${from}, compiled and instantiated in ${((performance.now() - t0) / 1000).toFixed(2)} s`);
        done(r.instance, r.module);
      }))
      .catch((e) => fail('game.wasm: ' + (e.message || e)));
    return {};
  }

  // ---- the engine
  function engineArgs(M) {
    const env = new URLSearchParams(location.search).get('env');
    if (!env) return;
    const ENV = M.ENV || globalThis.ENV;
    for (const kv of env.split(',')) {
      const i = kv.indexOf('=');
      if (i > 0) ENV[kv.slice(0, i)] = kv.slice(i + 1);
    }
    log('[web] engine variables: ' + env);
  }

  function timeToMain(io, manifest) {
    const total = manifest.files.reduce((n, f) => n + f[1], 0);
    // then every 30 s while the engine reads: what came from the network and what from the cache
    let last = '';
    setInterval(() => io.stats().then((s) => {
      const now = `${s.reads}/${s.boot + s.demand}`;
      if (now === last) return;
      last = now;
      const mib = (n) => (n / 2 ** 20).toFixed(1);
      log(`[web] data at ${secs()} s: fetched ${mib(s.boot + s.demand)} MiB of ${mib(total)} MiB (boot set ${mib(s.boot)}, on demand ${mib(s.demand)}, ${s.fetches} requests); ${s.reads} engine reads, ${mib(s.hit)} MiB of them wholly from the cache`);
    }), 30000);
    watchers.push((t) => {
      if (!t.includes('>>> nnMain')) return;
      const at = secs();
      io.stats().then((s) => {
        const mib = (n) => (n / 2 ** 20).toFixed(1);
        log(`[web] nnMain ${at} s after the page opened; data fetched by then: ${mib(s.boot + s.demand)} MiB of ${mib(total)} MiB (boot set ${mib(s.boot)}${s.bootDone ? '' : ', still running'}, on demand ${mib(s.demand)}, ${s.fetches} requests); ${s.reads} engine reads, ${mib(s.hit)} MiB of them from the cache`);
      });
    });
  }

  async function main() {
    const text = await (await fetch('data.json', { cache: 'no-store' })).text();
    const manifest = JSON.parse(text);
    // ?bootset=0: no prefetch (every read on demand); ?fresh=1: the cache emptied first (a cold start)
    const q = new URLSearchParams(location.search);
    const bootset = q.get('bootset') === '0'
      ? []
      : await fetch('bootset.json', { cache: 'no-store' }).then((r) => (r.ok ? r.json() : []), () => []);
    manifest.fresh = q.get('fresh') === '1';
    const io = startIo(manifest, bootset);
    timeToMain(io, manifest);
    const audio = audioHost();
    window.Module = {
      arguments: ['/data'],
      instantiateWasm,
      nxrsHost: {
        io: (memory, table) => io.start(memory, table),
        audio,
        input: (memory, block, words) => NxrsInput.publish(memory, block, words, log),
        // WEB-W4: the GPU worker (gpu.js) gets the memory and the engine's command ring
        gpu: (memory, ring) => NXRS_GPU.start(memory, ring),
      },
      print: (t) => log(t),
      printErr: (t) => log(t),
      preRun: [() => {
        const M = window.Module;
        const FS = M.FS || globalThis.FS;
        engineArgs(M);
        M.addRunDependency('host');
        Promise.all([
          io.ready,
          hostFiles(FS, manifest, text),
          NxrsSaves.restore(FS, '/data/save', manifest.title, log),
        ]).then(([cache, [n, bytes]]) => {
          log(`[web] ${n} host files (${(bytes / 1024).toFixed(0)} KiB) in /data; game data streamed by the I/O worker (${cache} cache); ${secs()} s`);
          M.removeRunDependency('host');
        }, (e) => fail('The game did not start: ' + (e.message || e)));
      }],
      onAbort: (what) => fail('The game stopped: ' + what),
      onExit: (code) => fail(`The game exited (${code}) after ${secs()} s.`),
      // Emscripten's own progress words ("Running...") are log lines, not something the page shows
      setStatus: (t) => { if (t) log('[web] ' + t); },
    };
    const s = document.createElement('script');
    s.src = 'game.js';
    s.onerror = () => fail('game.js failed to load.');
    document.body.appendChild(s);
  }

  if (checks()) main().catch((e) => fail('The page did not start: ' + (e.message || e)));
})();
