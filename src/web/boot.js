// The page side of a web build (docs/35): checks what the build needs, shows the engine's log, starts the I/O worker
// and the boot-set prefetch, restores the saves, puts the small host files in the in-memory file system, and starts
// game.js (Emscripten; main runs on a worker thread of its own, PROXY_TO_PTHREAD). The engine then hands the page its
// shared blocks (src/web/nxrs-lib.js -> Module.nxrsHost): the I/O slots (to the I/O worker), the audio ring (to the
// AudioWorklet), the input block (to input.js). One function per subsystem below; the page never blocks.
//
// Game data (RomFS, ExeFS) is never copied into the in-memory file system: the engine reads it through the I/O worker
// (io-worker.js), which fetches it by HTTP range into an OPFS cache. URL options: ?env=NAME=value,... sets engine
// variables (e.g. NXRS_CONTROLLER_TRACE=1).
'use strict';
(() => {
  const status = document.getElementById('status');
  const T0 = performance.now();
  const secs = () => ((performance.now() - T0) / 1000).toFixed(2);
  const watchers = [];
  const log = makeLog();
  const fail = (why) => {
    status.textContent = why;
    status.className = 'red';
    log(why, true);
  };

  function makeLog() {
    const logEl = document.getElementById('log');
    const lines = [];
    let flush = 0;
    window.NXRS_LOG = lines;
    return (text, red) => {
      console.log('[nxrs] ' + text);
      for (const w of watchers) w(text);
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
  }

  function checks() {
    if (!self.crossOriginIsolated) {
      fail('this page is not cross-origin isolated (COOP/COEP): threads and shared memory are unavailable');
      return false;
    }
    try {
      new WebAssembly.Memory({ initial: 1n, maximum: 2n, address: 'i64', shared: true });
    } catch (e) {
      fail('this browser has no shared 64-bit WebAssembly memory (memory64): ' + e);
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
    let stats = null;
    const ready = new Promise((resolve, reject) => {
      worker.onerror = (e) => reject(new Error('io-worker.js: ' + (e.message || e)));
      worker.onmessage = (e) => {
        const m = e.data;
        if (m.log) log('[web] ' + m.log);
        if (m.ready) resolve(m.ready);
        if (m.stats && stats) stats(m.stats);
      };
    });
    worker.postMessage({ init: { files: manifest.files, version: manifest.version, title: manifest.title, bootset } });
    addEventListener('pagehide', () => worker.postMessage({ close: true }));
    return {
      ready,
      start: (memory, table) => worker.postMessage({ start: { memory, table } }),
      stats: () => new Promise((r) => { stats = r; worker.postMessage({ stats: true }); }),
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
        log(`[RED] [web] audio: the AudioWorklet did not start: ${e}`, true);
        ctx = null;
      }
    };
    const onGesture = () => {
      gesture = true;
      start();
    };
    addEventListener('pointerdown', onGesture, true);
    addEventListener('keydown', onGesture, true);
    return (memory, at, frames) => {
      ring = { memory, at, frames };
      log(`[web] audio: the engine's ring at ${at} (${frames} frames)${gesture ? '' : '; sound starts at the first click or key'}`);
      start();
    };
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
    status.textContent = 'reading the data manifest';
    const text = await (await fetch('data.json', { cache: 'no-store' })).text();
    const manifest = JSON.parse(text);
    const bootset = await fetch('bootset.json', { cache: 'no-store' }).then((r) => (r.ok ? r.json() : []), () => []);
    const io = startIo(manifest, bootset);
    timeToMain(io, manifest);
    const audio = audioHost();
    window.Module = {
      arguments: ['/data'],
      nxrsHost: {
        io: (memory, table) => io.start(memory, table),
        audio,
        input: (memory, block, words) => NxrsInput.publish(memory, block, words, log),
        // WEB-W4: the GPU worker's start goes here
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
          status.textContent = 'running';
          M.removeRunDependency('host');
        }, (e) => fail('not started: ' + (e.message || e)));
      }],
      onAbort: (what) => fail('aborted: ' + what),
      onExit: (code) => { status.textContent = `exited (${code}) after ${secs()} s`; },
      setStatus: (t) => { if (t) status.textContent = t; },
    };
    status.textContent = 'loading game.wasm';
    const s = document.createElement('script');
    s.src = 'game.js';
    s.onerror = () => fail('game.js failed to load');
    document.body.appendChild(s);
  }

  if (checks()) main().catch((e) => fail('boot: ' + (e.message || e)));
})();
