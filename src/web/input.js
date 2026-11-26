// The page's keyboard and gamepads into the engine's input block (docs/35 W3; os/src/input/web.rs reads it).
// Words: [0] sequence (bumped at every change), [1] focus, [2..10) 256 key bits by Windows virtual-key code, [10]
// gamepads connected (a bit each), gamepad i at [16 + 8i]: [0] connected, [1] buttons (bit = standard mapping index),
// [2..6) axes 0-3 scaled to ±32767; [40..48) key presses latched until the engine reads them. Keys are released when
// the page loses the focus.
'use strict';
const NxrsInput = (() => {
  const VK = {
    Backspace: 0x08, Tab: 0x09, Enter: 0x0d, NumpadEnter: 0x0d, ShiftLeft: 0xa0, ShiftRight: 0xa1,
    ControlLeft: 0xa2, ControlRight: 0xa3, AltLeft: 0xa4, AltRight: 0xa5, Escape: 0x1b, Space: 0x20,
    PageUp: 0x21, PageDown: 0x22, End: 0x23, Home: 0x24, ArrowLeft: 0x25, ArrowUp: 0x26, ArrowRight: 0x27,
    ArrowDown: 0x28, Insert: 0x2d, Delete: 0x2e, Minus: 0xbd, Equal: 0xbb, BracketLeft: 0xdb, BracketRight: 0xdd,
    Semicolon: 0xba, Quote: 0xde, Comma: 0xbc, Period: 0xbe, Slash: 0xbf, Backslash: 0xdc, Backquote: 0xc0,
  };
  for (let i = 0; i < 26; i++) VK['Key' + String.fromCharCode(65 + i)] = 0x41 + i;
  for (let i = 0; i < 10; i++) {
    VK['Digit' + i] = 0x30 + i;
    VK['Numpad' + i] = 0x60 + i;
  }
  // the browser's own keys pass through
  const PASS = new Set(['F5', 'F11', 'F12']);
  let w = null;
  const pads = new Map(); // index -> last encoding (string)

  function bump() {
    Atomics.add(w, 0, 1);
  }
  function setFocus() {
    if (!w) return;
    const f = document.hasFocus() && document.visibilityState === 'visible' ? 1 : 0;
    if (Atomics.exchange(w, 1, f) !== f) bump();
    if (!f) releaseAll();
  }
  function releaseAll() {
    let any = false;
    for (let i = 2; i < 10; i++) any = Atomics.exchange(w, i, 0) !== 0 || any;
    if (any) bump();
  }
  function key(e, down) {
    if (!w || PASS.has(e.code) || e.ctrlKey && e.shiftKey) return;
    const vk = VK[e.code];
    if (vk === undefined) return;
    e.preventDefault();
    const word = 2 + (vk >> 5);
    const bit = 1 << (vk & 31);
    const old = down ? Atomics.or(w, word, bit) : Atomics.and(w, word, ~bit);
    // a press latched until the engine reads it: one shorter than its 4 ms tick still reaches the game
    if (down) Atomics.or(w, 40 + (vk >> 5), bit);
    if (((old & bit) !== 0) !== down) bump();
  }
  function pollPads() {
    if (w) {
      const list = navigator.getGamepads ? navigator.getGamepads() : [];
      let mask = 0;
      for (let i = 0; i < 4; i++) {
        const g = list[i];
        const base = 16 + 8 * i;
        let enc = '';
        if (g && g.connected) {
          mask |= 1 << i;
          let b = 0;
          g.buttons.forEach((x, j) => { if (j < 32 && (x.pressed || x.value > 0.5)) b |= 1 << j; });
          const ax = [0, 1, 2, 3].map((j) => Math.round((g.axes[j] || 0) * 32767));
          enc = b + ',' + ax.join(',');
          if (pads.get(i) !== enc) {
            Atomics.store(w, base, 1);
            Atomics.store(w, base + 1, b);
            ax.forEach((v, j) => Atomics.store(w, base + 2 + j, v));
          }
        } else if (pads.get(i)) {
          for (let j = 0; j < 8; j++) Atomics.store(w, base + j, 0);
        }
        if (pads.get(i) !== enc) {
          pads.set(i, enc);
          bump();
        }
      }
      Atomics.store(w, 10, mask);
    }
    requestAnimationFrame(pollPads);
  }

  // the engine published its block: from now on the page writes into it
  function publish(memory, block, words, log) {
    w = new Int32Array(memory.buffer, block, words);
    setFocus();
    log(`[web] input: keyboard and gamepads into the engine's block at ${block}`);
  }
  addEventListener('keydown', (e) => key(e, true), true);
  addEventListener('keyup', (e) => key(e, false), true);
  addEventListener('blur', setFocus);
  addEventListener('focus', setFocus);
  document.addEventListener('visibilitychange', setFocus);
  addEventListener('gamepadconnected', (e) => console.log(`[nxrs] [web] gamepad ${e.gamepad.index}: ${e.gamepad.id} (${e.gamepad.mapping || 'no standard mapping'})`));
  requestAnimationFrame(pollPads);
  return { publish };
})();
