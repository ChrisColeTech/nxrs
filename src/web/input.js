// The page's keyboard and gamepads into the engine's input block (docs/35 W3; os/src/input/web.rs reads it).
// Words: [0] sequence (bumped at every change), [1] focus (1 visible and focused, 2 visible behind another window,
// 3 hidden), [2..10) 256 key bits by Windows virtual-key code, [10] gamepads connected (a bit each), [11] the in-game
// menu is open (WEB-W9: the engine pauses the game), gamepad i at [16 + 8i]: [0] connected, [1] buttons (bit =
// standard mapping index), [2..6) axes 0-3 scaled to ±32767; [48..56) key presses latched until the engine reads
// them. From [64] the settings mailbox (writeSettings). Keys are released when the page loses the focus.
//
// The engine writes the game's vibration back (WEB-RUMBLE): [56 + 2i] low motor, [57 + 2i] high motor of player i
// (0..65535; 0 while the game does not own input). Each poll plays them on gamepad i through its vibrationActuator
// ('dual-rumble': strong = low motor, weak = high motor), re-issued before an effect ends while the level holds,
// reset at zero; nothing while the menu is open, on blur or while the page is hidden.
//
// The menu (menu.js) opens with Esc, a gamepad's Home (standard button 16) or Minus + Plus held 1 s. While it is open
// nothing reaches the game: keys go to the menu, the block's keys and pads are released, and a button still held when
// it closes stays released until it is let go.
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
  const MAIL = 64;
  const MAIL_BYTES = 4096;
  const HOME = 16;
  const MINUS = 8;
  const PLUS = 9;
  let w = null;
  let menuOpen = false;
  let suppress = 0; // pad buttons held when the menu closed: released until let go
  let comboSince = 0;
  let homeWas = false;
  let pendingMail = null;
  const pads = new Map(); // index -> last encoding (string)
  const hooks = { open: () => {}, key: () => {} };
  const RUMBLE = 56;
  const EFFECT_MS = 200; // one effect's length; re-issued while the level holds
  const REISSUE_MS = 80; // re-issued this long before the effect would end
  const rumbling = new Map(); // gamepad index -> { low, high, until } of the effect playing
  const rumbleSeen = new Set(); // gamepad ids whose actuator was logged
  let rumbleLogged = 0;

  function bump() {
    Atomics.add(w, 0, 1);
  }
  function focusState() {
    if (document.visibilityState !== 'visible') return 3;
    return document.hasFocus() ? 1 : 2;
  }
  function setFocus() {
    if (!w) return;
    const f = focusState();
    if (Atomics.exchange(w, 1, f) !== f) bump();
    if (f !== 1) {
      releaseAll();
      stopAllRumble(f === 3 ? 'page hidden' : 'page not focused');
    }
  }
  function releaseAll() {
    if (!w) return;
    let any = false;
    for (let i = 2; i < 10; i++) any = Atomics.exchange(w, i, 0) !== 0 || any;
    if (any) bump();
  }
  function releasePads() {
    for (let i = 0; i < 4; i++) {
      const base = 16 + 8 * i;
      for (let j = 1; j < 6; j++) Atomics.store(w, base + j, 0);
    }
    pads.clear();
    bump();
  }
  function key(e, down) {
    if (PASS.has(e.code) || e.ctrlKey && e.shiftKey) return;
    if (menuOpen) {
      if (down) hooks.key(e);
      return;
    }
    if (e.code === 'Escape') {
      e.preventDefault();
      if (down && !e.repeat) hooks.open();
      return;
    }
    if (!w) return;
    const vk = VK[e.code];
    if (vk === undefined) return;
    e.preventDefault();
    const word = 2 + (vk >> 5);
    const bit = 1 << (vk & 31);
    const old = down ? Atomics.or(w, word, bit) : Atomics.and(w, word, ~bit);
    // a press latched until the engine reads it: one shorter than its 4 ms tick still reaches the game
    if (down) Atomics.or(w, 48 + (vk >> 5), bit);
    if (((old & bit) !== 0) !== down) bump();
  }
  function padBits(g) {
    let b = 0;
    g.buttons.forEach((x, j) => { if (j < 32 && (x.pressed || x.value > 0.5)) b |= 1 << j; });
    return b;
  }
  // the first connected gamepad (the menu reads it too)
  function firstPad() {
    const list = navigator.getGamepads ? navigator.getGamepads() : [];
    for (const g of list) if (g && g.connected) return g;
    return null;
  }
  function menuShortcut(b) {
    const home = (b >> HOME & 1) !== 0;
    const homePressed = home && !homeWas;
    homeWas = home;
    const combo = (b >> MINUS & 1) && (b >> PLUS & 1);
    if (!combo) comboSince = 0;
    else if (!comboSince) comboSince = performance.now();
    if (homePressed || (comboSince && performance.now() - comboSince >= 1000)) {
      comboSince = 0;
      return true;
    }
    return false;
  }
  // A gamepad's dual-rumble actuator (Chrome: vibrationActuator, its effects list where it has one), logged once.
  function actuator(g, i) {
    const a = g.vibrationActuator || null;
    if (!rumbleSeen.has(g.id)) {
      rumbleSeen.add(g.id);
      const effects = a ? (a.effects ? [...a.effects].join(',') : `type ${a.type}`) : 'none';
      console.log(`[nxrs] [web] gamepad ${i} rumble: vibrationActuator ${a ? 'present' : 'absent'}, effects ${effects || 'none'}`);
    }
    if (!a) return null;
    if (a.effects ? ![...a.effects].includes('dual-rumble') : a.type && a.type !== 'dual-rumble') return null;
    return a;
  }
  function stopRumble(i, a, why) {
    rumbling.delete(i);
    if (a) a.reset().catch(() => {});
    if (rumbleLogged < 200) {
      rumbleLogged++;
      console.log(`[nxrs] [web] rumble gamepad ${i}: reset (${why})`);
    }
  }
  // every gamepad still rumbling stops (menu open, blur, hidden: the next frame may not come while hidden)
  function stopAllRumble(why) {
    const list = navigator.getGamepads ? navigator.getGamepads() : [];
    for (const i of [...rumbling.keys()]) stopRumble(i, list[i] && list[i].vibrationActuator, why);
  }
  // The engine's motor levels to the gamepads.
  function pollRumble(list) {
    const on = w && !menuOpen && focusState() === 1;
    for (let i = 0; i < 4; i++) {
      const g = list[i];
      const a = g && g.connected ? actuator(g, i) : null;
      if (!a) {
        rumbling.delete(i);
        continue;
      }
      const low = on ? Atomics.load(w, RUMBLE + 2 * i) / 65535 : 0;
      const high = on ? Atomics.load(w, RUMBLE + 2 * i + 1) / 65535 : 0;
      const last = rumbling.get(i);
      if (low <= 0 && high <= 0) {
        if (last) stopRumble(i, a, on ? 'the game stopped' : menuOpen ? 'menu open' : 'page not focused');
        continue;
      }
      const now = performance.now();
      if (last && last.low === low && last.high === high && now < last.until - REISSUE_MS) continue;
      a.playEffect('dual-rumble', { startDelay: 0, duration: EFFECT_MS, strongMagnitude: low, weakMagnitude: high })
        .catch((e) => console.log(`[nxrs] [web] rumble gamepad ${i}: playEffect failed: ${e}`));
      if ((!last || last.low !== low || last.high !== high) && rumbleLogged < 200) {
        rumbleLogged++;
        console.log(`[nxrs] [web] rumble gamepad ${i}: playEffect dual-rumble strong ${low.toFixed(3)} weak ${high.toFixed(3)} ${EFFECT_MS} ms`);
      }
      rumbling.set(i, { low, high, until: now + EFFECT_MS });
    }
  }
  function pollPads() {
    pollRumble(navigator.getGamepads ? navigator.getGamepads() : []);
    if (w && !menuOpen) {
      const list = navigator.getGamepads ? navigator.getGamepads() : [];
      let mask = 0;
      for (let i = 0; i < 4; i++) {
        const g = list[i];
        const base = 16 + 8 * i;
        let enc = '';
        if (g && g.connected) {
          mask |= 1 << i;
          let b = padBits(g);
          if (i === 0 || !(mask & ((1 << i) - 1))) {
            if (menuShortcut(b)) {
              suppress = b;
              hooks.open();
              break;
            }
          }
          suppress &= b;
          b &= ~suppress;
          b &= ~(1 << HOME);
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
      if (!menuOpen) Atomics.store(w, 10, mask);
    }
    requestAnimationFrame(pollPads);
  }

  // The settings mailbox: `text` (key=value lines) for the engine's next tick, under a sequence lock (odd while
  // written). Before the engine published its block, kept and written when it does.
  function writeSettings(text) {
    if (!w) {
      pendingMail = text;
      return;
    }
    const bytes = new TextEncoder().encode(text).subarray(0, MAIL_BYTES);
    Atomics.add(w, MAIL, 1);
    Atomics.store(w, MAIL + 1, bytes.length);
    for (let i = 0; i < bytes.length; i += 4) {
      const v = bytes[i] | (bytes[i + 1] || 0) << 8 | (bytes[i + 2] || 0) << 16 | (bytes[i + 3] || 0) << 24;
      Atomics.store(w, MAIL + 4 + (i >> 2), v);
    }
    Atomics.add(w, MAIL, 1);
  }

  // The menu opens (the game pauses: keys and pads released) or closes (buttons still held stay released).
  function setMenu(open) {
    menuOpen = open;
    if (open) stopAllRumble('menu open');
    if (!w) return;
    releaseAll();
    if (open) releasePads();
    else {
      const g = firstPad();
      suppress = g ? padBits(g) : 0;
      homeWas = g ? (padBits(g) >> HOME & 1) !== 0 : false;
    }
    Atomics.store(w, 11, open ? 1 : 0);
    bump();
  }

  // the engine published its block: from now on the page writes into it
  function publish(memory, block, words, log) {
    w = new Int32Array(memory.buffer, block, words);
    setFocus();
    if (menuOpen) setMenu(true);
    if (pendingMail !== null) writeSettings(pendingMail);
    pendingMail = null;
    log(`[web] input: keyboard and gamepads into the engine's block at ${block}`);
  }
  addEventListener('keydown', (e) => key(e, true), true);
  addEventListener('keyup', (e) => key(e, false), true);
  addEventListener('blur', setFocus);
  addEventListener('focus', setFocus);
  document.addEventListener('visibilitychange', setFocus);
  addEventListener('gamepadconnected', (e) => console.log(`[nxrs] [web] gamepad ${e.gamepad.index}: ${e.gamepad.id} (${e.gamepad.mapping || 'no standard mapping'})`));
  requestAnimationFrame(pollPads);
  return {
    publish,
    setMenu,
    writeSettings,
    firstPad,
    padBits,
    // menu.js: what opens it and where its keys go while it is open
    hook(open, keyHandler) {
      hooks.open = open;
      hooks.key = keyHandler;
    },
  };
})();
