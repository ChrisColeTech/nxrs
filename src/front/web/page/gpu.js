// The page side of the web build's GPU (docs/35 W4): the canvas the game is shown in, and the GPU worker that owns the
// WebGPU device and executes the engine's command ring (gpu-worker.js). The engine starts it through
// nxgfx_web_start (../link/nxgfx.lib.js) once it creates its Vulkan device (src/graphics/src/web).
//
// The canvas is the page (WEB-WPAGE): as large as the window allows at the game's aspect, centred on black. What the
// page must tell the player (the first picture, a lost device) goes to boot.js as an `nxrs-gpu` event.
'use strict';
(() => {
  // the engine's log lines also go to the server (`dev_server.py --log`), readable while the page is busy
  const pending = [];
  const consoleLog = console.log.bind(console);
  console.log = (...args) => {
    consoleLog(...args);
    if (typeof args[0] === 'string' && args[0].startsWith('[nxrs]')) pending.push(args[0]);
  };
  setInterval(() => {
    if (!pending.length) return;
    const body = pending.splice(0).join('\n') + '\n';
    fetch('log', { method: 'POST', body }).catch(() => {});
  }, 500);
  // the game's aspect (the canvas's shape on the page); its backing store is the displayed size in device pixels
  const W = 1280;
  const H = 720;
  let started = false;
  const tell = (detail) => dispatchEvent(new CustomEvent('nxrs-gpu', { detail }));
  const say = (text) => console.log('[nxrs] [gpu] ' + text);
  const canvas = document.createElement('canvas');
  canvas.id = 'screen';
  // letterboxed: the whole width when the window is taller than the game, the whole height when it is wider
  canvas.style.width = `min(100vw, calc(100vh * ${W} / ${H}))`;
  canvas.style.height = `min(100vh, calc(100vw * ${H} / ${W}))`;
  // WEB-W8: the backing store follows the canvas's displayed size x devicePixelRatio (exact device pixels where the
  // browser reports them), so the engine's swap chain is the screen's pixels and the picture is scaled once, by the
  // engine's present (sharp upscale or its filtered blit), never again by the page. Before the canvas goes to the
  // GPU worker its size is set here; after, the worker resizes it at the present of an image of the new size.
  let size = [W, H];
  const device = (w, h) => [Math.max(1, Math.round(w)), Math.max(1, Math.round(h))];
  const resized = (s) => {
    if (s[0] === size[0] && s[1] === size[1]) return;
    size = s;
    // the worker exists from instantiation (early), but owns the canvas only once started
    const worker = started && globalThis.NXRS_GPU && NXRS_GPU.worker;
    if (worker) worker.postMessage({ resize: s });
    else {
      canvas.width = s[0];
      canvas.height = s[1];
    }
  };
  const observer = new ResizeObserver((entries) => {
    const e = entries[entries.length - 1];
    const dp = e.devicePixelContentBoxSize && e.devicePixelContentBoxSize[0];
    if (dp) resized(device(dp.inlineSize, dp.blockSize));
    else {
      const c = e.contentBoxSize[0];
      resized(device(c.inlineSize * devicePixelRatio, c.blockSize * devicePixelRatio));
    }
  });
  try {
    observer.observe(canvas, { box: 'device-pixel-content-box' });
  } catch {
    observer.observe(canvas);
  }
  canvas.width = W;
  canvas.height = H;
  const place = () => {
    if (!canvas.isConnected) document.body.prepend(canvas);
    const r = canvas.getBoundingClientRect();
    if (r.width > 0) resized(device(r.width * devicePixelRatio, r.height * devicePixelRatio));
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', place);
  else place();

  globalThis.NXRS_GPU = {
    canvas,
    stats: {},
    // development: asks the worker for its ring header words (into NXRS_GPU.stats)
    ask() {
      if (NXRS_GPU.worker) NXRS_GPU.worker.postMessage({ ask: 1 });
    },
    // WEB-W8c: the worker gets the wasm memory at instantiation (boot.js instantiateWasm), while it is still small.
    // V8 counts a shared memory's length at the time an isolate receives it as that isolate's external memory (later
    // growth is not counted): handed over at nxgfx_web_start, after the guest window was claimed (~9.4 GB), the
    // worker's GC limit sat below that figure and it ran ~220 full collections a second (build/WEB/W8c/gcbench: a
    // worker handed >= 8 GiB collects continuously, one handed it small and grown later does not).
    early(memory) {
      if (handed || !memory) return;
      handed = true;
      worker().postMessage({ memory });
    },
    start(memory, ring) {
      if (started) return;
      started = true;
      place();
      const off = canvas.transferControlToOffscreen();
      const w = worker();
      // ?shots=N: every N-th presented image goes to the dev server (development)
      const q = new URLSearchParams(location.search);
      const shotEvery = Number(q.get('shots') || 0);
      // ?subtrace=1: the worker logs every submission to the dev server (gpu-worker.js traceSubmit)
      const subTrace = q.get('subtrace') === '1';
      // the memory only when early() did not hand it over (a page without the instantiate hook)
      const m = { ring, canvas: off, width: size[0], height: size[1], shotEvery, subTrace };
      if (!handed) m.memory = memory;
      w.postMessage(m, [off]);
    },
  };
  let handed = false;
  function worker() {
    if (NXRS_GPU.worker) return NXRS_GPU.worker;
    const worker = new Worker('gpu-worker.js');
    worker.onmessage = (e) => {
      const m = e.data;
      if (m.log) say(m.log);
      if (m.stats) NXRS_GPU.stats = m.stats;
      if (m.first) tell({ first: true });
      if (m.fatal) tell({ fatal: m.fatal });
    };
    worker.onerror = (e) => {
      say('worker: ' + e.message);
      tell({ fatal: 'the GPU worker stopped: ' + e.message });
    };
    NXRS_GPU.worker = worker;
    return worker;
  }
})();
