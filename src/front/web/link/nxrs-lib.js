// The engine's calls into the page (docs/35 W3), linked into game.js with --js-library. Each runs on the page's
// thread (proxied from the engine thread that calls it, which waits for it) and hands a shared block of linear memory
// to the page: the page (boot.js, Module.nxrsHost) gives it to its worker, worklet or event handlers. After that the
// engine and the page meet only through atomics on the block.
addToLibrary({
  nxweb_io_start__proxy: 'sync',
  nxweb_io_start__sig: 'vp',
  nxweb_io_start: (table) => {
    Module['nxrsHost'].io(wasmMemory, Number(table));
  },
  nxweb_audio_publish__proxy: 'sync',
  nxweb_audio_publish__sig: 'vpi',
  nxweb_audio_publish: (ring, frames) => {
    Module['nxrsHost'].audio(wasmMemory, Number(ring), frames);
  },
  nxweb_input_publish__proxy: 'sync',
  nxweb_input_publish__sig: 'vpi',
  nxweb_input_publish: (block, words) => {
    Module['nxrsHost'].input(wasmMemory, Number(block), words);
  },
});
