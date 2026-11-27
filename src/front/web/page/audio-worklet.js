// The game's sound (docs/35 W3): plays the engine's ring (os/src/web/audio.rs) of 48 kHz float stereo frames in shared
// memory. Header words: [0] written, [1] read, [2] capacity (a power of two), [3] heartbeat (calls here), [4] underrun
// frames, [5] underrun events. A short ring plays the missing tail as silence and counts it; nothing is counted
// before the first frames arrive.
'use strict';
class NxrsRing extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const { memory, ring, frames } = options.processorOptions;
    this.h = new Int32Array(memory.buffer, ring, 16);
    this.d = new Float32Array(memory.buffer, ring + 64, frames * 2);
    this.mask = frames - 1;
    this.cap = frames;
    this.started = false;
  }
  process(_inputs, outputs) {
    const out = outputs[0];
    const L = out[0];
    const R = out[1] || out[0];
    const n = L.length;
    const h = this.h;
    Atomics.add(h, 3, 1);
    const w = Atomics.load(h, 0);
    const r = Atomics.load(h, 1);
    const avail = Math.min((w - r) >>> 0, this.cap);
    const k = Math.min(n, avail);
    for (let j = 0; j < k; j++) {
      const i = ((r + j) & this.mask) * 2;
      L[j] = this.d[i];
      R[j] = this.d[i + 1];
    }
    for (let j = k; j < n; j++) {
      L[j] = 0;
      R[j] = 0;
    }
    if (avail > 0) this.started = true;
    if (k < n && this.started) {
      Atomics.add(h, 4, n - k);
      Atomics.add(h, 5, 1);
    }
    Atomics.store(h, 1, (r + k) | 0);
    return true;
  }
}
registerProcessor('nxrs-ring', NxrsRing);
