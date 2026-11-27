// The page side of the web build's GPU (docs/35 W4): the canvas the game is shown in, and the GPU worker that owns the
// WebGPU device and executes the engine's command ring (src/web/gpu-worker.js). The engine starts it through
// nxgfx_web_start (src/web/nxgfx.lib.js) once it creates its Vulkan device (src/graphics/src/web).
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
  const W = 1280;
  const H = 720;
  let started = false;
  const tell = (detail) => dispatchEvent(new CustomEvent('nxrs-gpu', { detail }));
  const say = (text) => console.log('[nxrs] [gpu] ' + text);
  const canvas = document.createElement('canvas');
  canvas.id = 'screen';
  canvas.width = W;
  canvas.height = H;
  // letterboxed: the whole width when the window is taller than the game, the whole height when it is wider
  canvas.style.width = `min(100vw, calc(100vh * ${W} / ${H}))`;
  canvas.style.height = `min(100vh, calc(100vw * ${H} / ${W}))`;
  const place = () => {
    if (!canvas.isConnected) document.body.prepend(canvas);
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
    start(memory, ring) {
      if (started) return;
      started = true;
      place();
      const off = canvas.transferControlToOffscreen();
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
      // ?shots=N: every N-th presented image goes to the dev server (development)
      const q = new URLSearchParams(location.search);
      const shotEvery = Number(q.get('shots') || 0);
      // ?subtrace=1: the worker logs every submission to the dev server (gpu-worker.js traceSubmit)
      const subTrace = q.get('subtrace') === '1';
      worker.postMessage({ memory, ring, canvas: off, width: W, height: H, shotEvery, subTrace }, [off]);
      NXRS_GPU.worker = worker;
    },
  };
})();
