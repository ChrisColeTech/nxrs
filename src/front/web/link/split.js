// The cold module's loader (docs/35 §4.6, WEB-W6), a --pre-js of a split web build (src/cli/src/websplit.rs): runs in
// the page and in every engine thread. A guest function outside the recorded hot set is a placeholder in its thread's
// function table; its first call on a thread lands here through Emscripten's split-module proxy, which then calls the
// real function from the table. The cold module is fetched and compiled synchronously on that thread (an engine
// pthread: synchronous XHR and compile are allowed in workers), instantiated against this thread's primary instance
// (which patches this thread's table), and the call goes on: a cold call waits, it never traps. Each thread does this
// once; the server's revalidation answers the later fetches from the browser cache.
//
// Emscripten's proxy names the cold file from `wasmBinaryFile`, which only the page's thread sets (an engine thread
// gets the compiled module, not the file): every thread is given it here, from this script's own location (game.js
// beside game.wasm), before the module is instantiated.
if (typeof WorkerGlobalScope !== 'undefined' && self.location && /^https?:/.test(self.location.href)) {
  wasmBinaryFile = new URL('game.wasm', self.location.href).href;
}
Module['loadSplitModule'] = (file, imports, base) => {
  const say = (t) => (typeof err === 'function' ? err(t) : console.log(t));
  if (typeof WorkerGlobalScope === 'undefined') {
    // the page's thread runs no guest code, and may not compile a large module synchronously
    say(`[RED] [web] split: a cold function (table slot ${base}) was called on the page's thread`);
  }
  const t0 = performance.now();
  const x = new XMLHttpRequest();
  x.open('GET', file, false);
  x.responseType = 'arraybuffer';
  x.send();
  if (x.status !== 200) throw new Error(`${file}: HTTP ${x.status}`);
  const t1 = performance.now();
  const module = new WebAssembly.Module(x.response);
  const instance = new WebAssembly.Instance(module, imports);
  const t2 = performance.now();
  say(`[web] split: cold module loaded on this thread at its first cold call (table slot ${base}): ` +
    `${(x.response.byteLength / 2 ** 20).toFixed(1)} MiB fetched in ${(t1 - t0).toFixed(0)} ms, ` +
    `compiled and instantiated in ${(t2 - t1).toFixed(0)} ms`);
  return [instance, module];
};
