// Emscripten JS library of the web build's GPU (docs/35 W4; linked by nxrs-build --target web, src/cli/src/cc.rs).
//
// nxgfx_web_start(ring): on the page's thread (proxied), hands the wasm memory and the command ring's address to the
//   page's GPU worker (src/web/gpu.js starts it); 1 when a worker exists.
// nxgfx_web_fetch(url, len, out_len) / nxgfx_web_fetch_take(dst): a synchronous GET on the calling thread (an engine
//   pthread: synchronous XHR is allowed in workers), the body kept until taken into wasm memory.
addToLibrary({
  nxgfx_web_start__proxy: 'sync',
  nxgfx_web_start__sig: 'ip',
  nxgfx_web_start: (ring) => {
    const host = Module['nxrsHost'];
    if (!host || !host.gpu) return 0;
    host.gpu(wasmMemory, Number(ring));
    return 1;
  },
  $nxgfxFetched: null,
  nxgfx_web_fetch__deps: ['$nxgfxFetched'],
  nxgfx_web_fetch__sig: 'ipp',
  nxgfx_web_fetch: (url, len) => {
    // relative to the page's folder (a pthread's own location may be a blob: URL)
    const u = new URL(UTF8ToString(Number(url), Number(len)), scriptDirectory || location.href).href;
    try {
      const x = new XMLHttpRequest();
      x.open('GET', u, false);
      x.responseType = 'arraybuffer';
      x.send();
      if (x.status !== 200 && x.status !== 206) return -1;
      nxgfxFetched = new Uint8Array(x.response);
      return nxgfxFetched.length;
    } catch (e) {
      return -1;
    }
  },
  nxgfx_web_fetch_take__deps: ['$nxgfxFetched'],
  nxgfx_web_fetch_take__sig: 'vp',
  nxgfx_web_fetch_take: (dst) => {
    if (nxgfxFetched) HEAPU8.set(nxgfxFetched, Number(dst));
    nxgfxFetched = null;
  },
});
