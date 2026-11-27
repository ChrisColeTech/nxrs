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

  // the one line the player sees over the game: the click for sound, a failure (which stays)
  function makeNotice() {
    const el = document.getElementById('notice');
    return {
      show(text, error) {
        if (failed && !error) return;
        if (error && loading.fail(text)) return;
        el.textContent = text;
        el.className = error ? 'error' : '';
      },
      clear(text) {
        if (!failed && (text === undefined || el.textContent === text)) el.textContent = '';
      },
    };
  }

  // ---- the loading screen (WEB-W9): the game's name and icon, one bar over the bytes still to come (game code and
  // the boot set), and the steps with their real measures; it leaves at the first picture. A failure replaces the
  // steps with its reason.
  const loading = (() => {
    const el = document.getElementById('nx-loading');
    const $ = (s) => el.querySelector(s);
    const steps = {};
    for (const li of el.querySelectorAll('.steps li')) steps[li.dataset.step] = li;
    let gone = false;
    const bytes = { code: 0, codeTotal: 0, data: 0, dataTotal: 0 };
    const mb = (n) => (n / 1e6).toFixed(1);
    const bar = () => {
      const total = bytes.codeTotal + bytes.dataTotal;
      const got = Math.min(bytes.code, bytes.codeTotal) + Math.min(bytes.data, bytes.dataTotal);
      $('.big i').style.width = total ? `${(100 * got / total).toFixed(1)}%` : '0';
      const codeLeft = bytes.code < bytes.codeTotal;
      $('.cap span').textContent = codeLeft ? 'Downloading game code' : bytes.data < bytes.dataTotal ? 'Downloading game data' : 'Starting';
      $('.cap .val').textContent = total ? `${mb(got)} of ${mb(total)} MB` : '';
    };
    return {
      step(name, state, measure) {
        const li = steps[name];
        if (!li || gone) return;
        li.className = state;
        if (measure !== undefined) li.querySelector('.m').textContent = measure;
      },
      bytes(kind, got, total) {
        bytes[kind] = got;
        bytes[kind + 'Total'] = total;
        if (!gone) bar();
      },
      head(name, sub, icon) {
        if (name) $('h1').textContent = name;
        if (sub !== undefined) $('.sub').textContent = sub;
        if (icon) $('.icon').style.backgroundImage = `url("${icon}")`;
      },
      // the first picture: the game is the page
      leave() {
        gone = true;
        el.hidden = true;
      },
      // a failure while loading shows here (selectable), in place of the steps; false once the game is shown
      fail(text) {
        if (gone) return false;
        $('.steps').hidden = true;
        $('.error').textContent = text;
        return true;
      },
    };
  })();

  // the picture replaces the loading screen; a lost GPU device is a failure the player must see
  addEventListener('nxrs-gpu', (e) => {
    if (e.detail.first) {
      loading.step('start', 'done');
      loading.leave();
      NxrsMenu.ready();
    }
    if (e.detail.fatal) fail(e.detail.fatal);
  });

  // the game's name from its control data (the first language with one), else build.json's
  function nacpName(bytes) {
    for (let i = 0; i < 16; i++) {
      const b = bytes.subarray(i * 0x300, i * 0x300 + 0x200);
      const end = b.indexOf(0);
      const s = new TextDecoder().decode(b.subarray(0, end < 0 ? b.length : end)).trim();
      if (s) return s;
    }
    return '';
  }

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
        if (m.saved) savedOk(m.saved);
        if (m.saveFailed) NxrsMenu.saved({ failed: m.saveFailed.reason });
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

  // A save stored by the I/O worker: the menu lists it; the first one asks the browser to keep this site's storage
  // (navigator.storage.persist), so Chrome does not clear the saves under storage pressure.
  let persistAsked = false;
  function savedOk(s) {
    NxrsMenu.saved({ at: Date.now(), name: s.name });
    if (persistAsked || !navigator.storage || !navigator.storage.persist) return;
    persistAsked = true;
    navigator.storage.persisted()
      .then((p) => p || navigator.storage.persist())
      .then((p) => log(`[web] saves: storage ${p ? 'persistent' : 'not persistent (Chrome may clear it when the disk is low)'}`))
      .catch(() => {});
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
  // WEB-W9: a GainNode between the worklet and the speakers (the menu's volume and mute, page-side), and the sound
  // suspended while the menu pauses the game
  function audioHost() {
    let ring = null;
    let ctx = null;
    let gain = null;
    let gesture = false;
    let paused = false;
    const level = () => (NxrsSettings.get('mute') === '1' ? 0 : Number(NxrsSettings.get('volume')) / 100);
    NxrsSettings.onChange((k) => {
      if ((k === 'volume' || k === 'mute') && gain) gain.gain.value = level();
    });
    window.NxrsAudio = {
      suspend() {
        paused = true;
        if (ctx) ctx.suspend().catch(() => {});
      },
      resume() {
        paused = false;
        if (ctx && gesture) ctx.resume().catch(() => {});
      },
      // the sound has started (it is only suspended while the menu is open)
      state: () => ({ running: !!ctx && (ctx.state === 'running' || paused) }),
    };
    const start = async () => {
      if (!ring || !gesture) return;
      if (ctx) {
        if (ctx.state !== 'running' && !paused) ctx.resume().catch(() => {});
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
        gain = ctx.createGain();
        gain.gain.value = level();
        node.connect(gain).connect(ctx.destination);
        const state = () => {
          Atomics.store(h, 7, ctx.sampleRate);
          Atomics.store(h, 6, ctx.state === 'running' ? 1 : 2);
          log(`[web] audio: AudioContext ${ctx.state} at ${ctx.sampleRate} Hz`);
        };
        ctx.onstatechange = state;
        if (!paused) await ctx.resume();
        else await ctx.suspend();
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

  // Module.instantiateWasm: game.wasm through the code cache, compiled while it streams. The loading screen counts the
  // body's bytes as they arrive (the stream yields the module's own bytes, so its total is build.json's size).
  let wasmSize = 0;
  function counted(response, from) {
    if (!response.body) return response;
    let got = 0;
    const kept = !from.includes('fetched');
    loading.step('code', 'now', kept ? 'kept copy (checked with the server)' : '');
    if (kept) loading.bytes('code', 0, 0);
    const stream = response.body.pipeThrough(new TransformStream({
      transform(chunk, c) {
        got += chunk.length;
        if (!kept) {
          loading.bytes('code', got, wasmSize);
          loading.step('code', 'now', `${(got / 1e6).toFixed(1)} of ${(wasmSize / 1e6).toFixed(1)} MB`);
        }
        c.enqueue(chunk);
      },
    }));
    return new Response(stream, { status: 200, headers: { 'Content-Type': 'application/wasm' } });
  }
  function instantiateWasm(imports, done) {
    const t0 = performance.now();
    // WEB-W8c: the GPU worker gets the memory now, while it is small (gpu.js early: why)
    if (globalThis.NXRS_GPU && imports.env) NXRS_GPU.early(imports.env.memory);
    codeResponse('game.wasm')
      .then(({ response, from }) => WebAssembly.instantiateStreaming(counted(response, from), imports).then((r) => {
        const s = ((performance.now() - t0) / 1000).toFixed(2);
        log(`[web] game.wasm: ${from}, compiled and instantiated in ${s} s`);
        if (from.includes('fetched')) loading.bytes('code', wasmSize, wasmSize);
        loading.step('code', 'done');
        loading.step('compile', 'done', `${s} s`);
        loading.step('data', 'now');
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
    // the loading screen: the boot set's bytes present (fetched or cached) every 250 ms until the first picture
    let started = 0;
    const progress = setInterval(() => io.stats().then((s) => {
      if (!s.bootBytes) return;
      loading.bytes('data', s.bootDoneBytes, s.bootBytes);
      const left = !s.bootDone;
      loading.step('data', left ? 'now' : 'done', `${(Math.min(s.bootDoneBytes, s.bootBytes) / 1e6).toFixed(1)} of ${(s.bootBytes / 1e6).toFixed(1)} MB${s.boot ? '' : ' · from this browser\'s cache'}`);
      if (started) loading.step('start', 'now', `${((performance.now() - started) / 1000).toFixed(0)} s`);
    }), 250);
    addEventListener('nxrs-gpu', (e) => { if (e.detail.first) clearInterval(progress); });
    watchers.push((t) => {
      if (t.startsWith('[web] saves:') && !started) started = performance.now();
    });
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

  // build.json (beside this page): the game's title id, its name, the module's size
  async function describe(manifest) {
    const b = await fetch('build.json', { cache: 'no-store' }).then((r) => (r.ok ? r.json() : {}), () => ({}));
    const files = b.files || [];
    wasmSize = (files.find((f) => f.path === 'game.wasm') || {}).size || 0;
    const id = b.titleId || '';
    let name = b.title || manifest.title || 'Game';
    let icon = '';
    const nacp = manifest.files.find(([p]) => p === 'control.nacp');
    const ico = manifest.files.find(([p]) => /^icon_.*\.dat$/.test(p));
    try {
      if (nacp) name = nacpName(new Uint8Array(await (await fetch('data/control.nacp')).arrayBuffer())) || name;
      if (ico) icon = URL.createObjectURL(new Blob([await (await fetch('data/' + encodeURIComponent(ico[0]))).arrayBuffer()], { type: 'image/jpeg' }));
    } catch { /* the name and icon are a nicety; the start goes on */ }
    document.title = name;
    return { id, name, icon };
  }

  async function main() {
    const text = await (await fetch('data.json', { cache: 'no-store' })).text();
    const manifest = JSON.parse(text);
    const game = await describe(manifest);
    NxrsSettings.load(game.id);
    NxrsMenu.init({ title: game.name, titleId: game.id, saveTitle: manifest.title, log });
    let cached = false;
    try {
      cached = !!(await (await caches.open(CODE_CACHE)).match(new URL('game.wasm', location.href).href));
    } catch { /* no Cache Storage */ }
    loading.head(game.name, cached ? 'Loading · from this browser\'s cache' : 'Loading · first start on this browser', game.icon);
    loading.step('checks', 'done', 'WebGPU, threads, 64-bit memory');
    loading.step('code', 'now');
    loading.bytes('code', 0, cached ? 0 : wasmSize);
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
        // WEB-W9: the settings kept in this browser, read by the engine as its ini (os/src/boot.rs nxweb_main)
        FS.mkdirTree('/data');
        FS.writeFile('/data/settings.ini', NxrsSettings.iniText());
        (M.ENV || globalThis.ENV).NXRS_TITLE = game.name;
        M.addRunDependency('host');
        loading.step('saves', 'now');
        Promise.all([
          io.ready,
          hostFiles(FS, manifest, text),
          NxrsSaves.restore(FS, '/data/save', manifest.title, log),
        ]).then(([cache, [n, bytes], saves]) => {
          loading.step('saves', 'done', saves ? `${saves} restored` : 'none yet');
          loading.step('start', 'now');
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
