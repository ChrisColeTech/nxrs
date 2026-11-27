// The page side of the web build's GPU (docs/35 W4): the canvas the game is shown in, and the GPU worker that owns the
// WebGPU device and executes the engine's command ring (src/web/gpu-worker.js). The engine starts it through
// nxgfx_web_start (src/web/nxgfx.lib.js) once it creates its Vulkan device (src/graphics/src/web).
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
  const say = (text, red) => {
    console.log('[nxrs] [gpu] ' + text);
    const log = document.getElementById('log');
    if (log) {
      const d = document.createElement('div');
      d.textContent = '[gpu] ' + text;
      if (red) d.className = 'red';
      log.appendChild(d);
    }
  };
  const canvas = document.createElement('canvas');
  canvas.id = 'screen';
  canvas.width = W;
  canvas.height = H;
  canvas.style.cssText = 'display:block;width:960px;max-width:100%;aspect-ratio:16/9;background:#000;margin:8px 12px;';
  const place = () => {
    const status = document.getElementById('status');
    if (status && !canvas.isConnected) status.after(canvas);
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', place);
  else place();

  globalThis.NXRS_GPU = {
    canvas,
    stats: {},
    start(memory, ring) {
      if (started) return;
      started = true;
      place();
      const off = canvas.transferControlToOffscreen();
      const worker = new Worker('gpu-worker.js');
      worker.onmessage = (e) => {
        const m = e.data;
        if (m.log) say(m.log, m.red);
        if (m.stats) NXRS_GPU.stats = m.stats;
      };
      worker.onerror = (e) => say('worker: ' + e.message, true);
      // ?shots=N: every N-th presented image goes to the dev server (development)
      const shotEvery = Number(new URLSearchParams(location.search).get('shots') || 0);
      worker.postMessage({ memory, ring, canvas: off, width: W, height: H, shotEvery }, [off]);
      NXRS_GPU.worker = worker;
    },
  };
})();
