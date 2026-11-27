// WEB-W9: the in-game menu (docs/waves/WEB-W9.md): saves, graphics, audio and the controller, over the paused game,
// and the settings it keeps in the browser.
//
// Settings are the engine's own ini keys (src/config/src/options.rs) in its ini text: Graphics per game
// (localStorage nxrs-settings/<titleId>), Audio and Controller shared by every web game on this browser
// (nxrs-settings/shared); the page-only volume and mute in a [Web] section the engine keeps under [Other] and ignores.
// At start the page writes both texts to /data/settings.ini, which the engine opens as a native run opens its ini;
// a change in the menu goes to the engine at once through the input block's settings mailbox (input.js
// writeSettings), applied with nxconfig::set, and is stored here.
'use strict';
const NxrsSettings = (() => {
  const VERSION = '4'; // nxconfig SETTINGS_VERSION: no migration runs over the player's own values
  const GAME = {
    Display: { docked: '1' },
    Graphics: { resolution: '1', dynamic_resolution: '0', shadows: '1' },
    Performance: { fps_limit: '1' },
    Other: { upscale: 'sharp' },
  };
  const SHARED = {
    Controls: { button_layout: 'switch', button_map: '', key_map: '', stick_deadzone: '10', mute_on_focus_loss: '0' },
    Audio: { audio_slowdown: '1' },
    Web: { volume: '100', mute: '0' },
  };
  const PAGE_ONLY = new Set(['volume', 'mute']);
  let titleId = 'game';
  let kept = true;
  const values = {};
  const listeners = [];
  const keyOf = (part) => (part === SHARED ? 'nxrs-settings/shared' : `nxrs-settings/${titleId}`);

  function parse(text) {
    const out = {};
    for (const line of String(text || '').split(/\r?\n/)) {
      const l = line.trim();
      if (!l || l[0] === ';' || l[0] === '#' || l[0] === '[') continue;
      const i = l.indexOf('=');
      if (i > 0) out[l.slice(0, i).trim()] = l.slice(i + 1).trim();
    }
    return out;
  }
  function render(part, extra) {
    let t = '';
    for (const [sec, keys] of Object.entries(part)) {
      t += `[${sec}]\n`;
      for (const k of Object.keys(keys)) t += `${k}=${values[k]}\n`;
      if (sec === 'Other' && extra) t += extra;
    }
    return t;
  }
  function load(id) {
    titleId = String(id || 'game').toLowerCase();
    for (const part of [GAME, SHARED]) {
      for (const keys of Object.values(part)) Object.assign(values, keys);
      let stored = '';
      try {
        stored = localStorage.getItem(keyOf(part)) || '';
      } catch {
        kept = false;
      }
      const got = parse(stored);
      for (const keys of Object.values(part)) for (const k of Object.keys(keys)) if (k in got) values[k] = got[k];
    }
  }
  function save() {
    try {
      localStorage.setItem(keyOf(GAME), render(GAME, `settings_version=${VERSION}\n`));
      localStorage.setItem(keyOf(SHARED), render(SHARED));
      kept = true;
    } catch {
      kept = false;
    }
  }
  // the engine's keys as ini text (the page-only ones left out)
  function engineText() {
    let t = '';
    for (const part of [GAME, SHARED]) {
      for (const keys of Object.values(part)) for (const k of Object.keys(keys)) if (!PAGE_ONLY.has(k)) t += `${k}=${values[k]}\n`;
    }
    return t;
  }
  // /data/settings.ini: both texts, as the engine reads a native ini
  function iniText() {
    return render(GAME, `settings_version=${VERSION}\n`) + render(SHARED);
  }
  function set(k, v) {
    v = String(v);
    if (values[k] === v) return;
    values[k] = v;
    save();
    if (!PAGE_ONLY.has(k)) NxrsInput.writeSettings(engineText());
    for (const l of listeners) l(k, v);
  }
  return {
    load,
    get: (k) => values[k],
    set,
    iniText,
    engineText,
    kept: () => kept,
    onChange: (f) => listeners.push(f),
  };
})();

const NxrsMenu = (() => {
  const $ = (tag, attrs = {}, ...kids) => {
    const e = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (k === 'class') e.className = v;
      else if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
      else if (v !== undefined && v !== null && v !== false) e.setAttribute(k, v === true ? '' : v);
    }
    for (const k of kids.flat()) if (k !== null && k !== undefined && k !== false) e.append(k);
    return e;
  };
  const svg = (d) => {
    const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    s.setAttribute('viewBox', '0 0 24 24');
    s.innerHTML = d;
    return s;
  };
  const ICON = {
    saves: '<path d="M5 3h11l3 3v15H5z M8 3v6h8V3 M8 14h8v7H8z"/>',
    graphics: '<rect x="3" y="4" width="18" height="13" rx="1.5"/><path d="M8 21h8M12 17v4"/>',
    audio: '<path d="M4 9h4l5-4v14l-5-4H4z M16 9a4 4 0 010 6 M18.5 6.5a7.5 7.5 0 010 11"/>',
    controller: '<path d="M7 8h10a4 4 0 014 4l1 4a2.5 2.5 0 01-4.5 1.5L15.5 15h-7l-2 2.5A2.5 2.5 0 012 16l1-4a4 4 0 014-4z M7 11v3M5.5 12.5h3"/>',
    game: '<path d="M4 12a8 8 0 1014-5.3 M18 3v4h-4"/>',
    rate: '<circle cx="12" cy="13" r="8"/><path d="M12 13l4-4M10 2h4"/>',
    buttons: '<rect x="3" y="7" width="18" height="11" rx="1.5"/><path d="M6 10h1M9 10h1M12 10h1M15 10h1M7 14h10"/>',
    question: '<path d="M9.5 9a2.5 2.5 0 115 0c0 2-2.5 2-2.5 4M12 17v.5"/>',
    error: '<path d="M12 7v6M12 16.5v.5"/>',
  };
  const X_SVG = '<svg viewBox="0 0 12 12" stroke="currentColor" stroke-width="1.6"><path d="M2 2l8 8M10 2l-8 8"/></svg>';
  const SECTIONS = [['saves', 'Saves'], ['graphics', 'Graphics'], ['audio', 'Audio'], ['controller', 'Controller']];

  let ctx = null; // {title, titleId, saveTitle, log}
  let root = null;
  let form = null;
  let rail = null;
  let section = 'saves';
  let open = false;
  let ready = false;
  let question = null; // the open question's element
  let capture = null; // {kind: 'key'|'pad', guest, el}
  let lastSave = null; // {at} of the last stored save, or {failed: reason}
  let padPrev = 0;
  let padRepeat = 0;

  const info = (text) => $('span', { class: 'info', title: text, 'aria-label': text }, 'i');
  const group = (icon, title, text, err) => $('div', { class: 'group' },
    $('span', { class: 'tile' + (err ? ' err' : '') }, svg(ICON[icon])), $('h2', {}, title), text ? info(text) : null);
  const row = (label, text, control) => $('div', { class: 'row' }, $('span', { class: 'l' }, label, text ? info(text) : null), control);
  const rule = () => $('div', { class: 'rule' });
  const btn = (label, on, cls = '') => $('button', { class: 'btn ' + cls, 'data-nav': '', onclick: on }, label);

  function select(key, options) {
    const s = $('select', { 'data-nav': '', 'aria-label': key, onchange: () => NxrsSettings.set(key, s.value) },
      options.map(([v, l]) => $('option', { value: v }, l)));
    s.value = NxrsSettings.get(key);
    if (s.selectedIndex < 0) s.selectedIndex = options.findIndex(([v]) => Number(v) === Number(NxrsSettings.get(key)));
    s.nxStep = (d) => {
      const i = Math.max(0, Math.min(options.length - 1, s.selectedIndex + d));
      if (i !== s.selectedIndex) {
        s.selectedIndex = i;
        NxrsSettings.set(key, s.value);
      }
    };
    return s;
  }
  function toggle(key, label) {
    const t = $('button', { class: 'tog', role: 'switch', 'aria-label': label, 'data-nav': '' });
    const show = () => t.setAttribute('aria-checked', NxrsSettings.get(key) === '1' ? 'true' : 'false');
    t.onclick = () => { NxrsSettings.set(key, NxrsSettings.get(key) === '1' ? '0' : '1'); show(); };
    t.nxStep = (d) => { NxrsSettings.set(key, d > 0 ? '1' : '0'); show(); };
    show();
    return t;
  }
  function seg(key, options, label) {
    const box = $('div', { class: 'seg', role: 'group', 'aria-label': label, 'data-nav': '', tabindex: '0' });
    const show = () => {
      for (const b of box.children) b.setAttribute('aria-pressed', b.dataset.v === NxrsSettings.get(key) ? 'true' : 'false');
    };
    for (const [v, l] of options) {
      box.append($('button', { 'data-v': v, tabindex: '-1', onclick: () => { NxrsSettings.set(key, v); show(); } }, l));
    }
    box.nxStep = (d) => {
      const i = options.findIndex(([v]) => v === NxrsSettings.get(key));
      const j = Math.max(0, Math.min(options.length - 1, (i < 0 ? 0 : i) + d));
      NxrsSettings.set(key, options[j][0]);
      show();
    };
    show();
    return box;
  }
  function slider(key, min, max, step, label) {
    const val = $('span', { class: 'val' });
    const r = $('input', { type: 'range', min, max, step, 'aria-label': label, 'data-nav': '' });
    r.value = NxrsSettings.get(key);
    const show = () => { val.textContent = r.value + '%'; };
    r.oninput = () => { NxrsSettings.set(key, r.value); show(); };
    r.nxStep = (d) => {
      r.value = String(Math.max(min, Math.min(max, Number(r.value) + d * step)));
      NxrsSettings.set(key, r.value);
      show();
    };
    show();
    return $('span', { class: 'slider' }, r, val);
  }

  // ---------------------------------------------------------------- saves
  const playerName = (save) => {
    const m = /^user(\d+)$/.exec(save);
    return m ? `Player ${m[1]}` : save;
  };
  const when = (ms) => {
    const d = new Date(ms);
    const t = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    if (d.toDateString() === new Date().toDateString()) return `Today ${t}`;
    return `${d.toLocaleDateString([], { day: 'numeric', month: 'short', year: 'numeric' })} ${t}`;
  };
  const kb = (n) => (n < 1024 ? `${n} B` : `${Math.round(n / 1024)} KB`);
  const day = (ms) => new Date(ms).toISOString().slice(0, 10);

  async function download(entry) {
    const blob = await NxrsSaves.exportFile(ctx.saveTitle, entry, {
      titleId: ctx.titleId, title: ctx.title, engine: 'nxrs',
    });
    const a = $('a', { href: URL.createObjectURL(blob), download: `${ctx.title} - ${playerName(entry.save)} - ${day(Date.now())}.nxsave` });
    document.body.append(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(a.href), 10000);
  }
  async function exportAll(list) {
    if (window.showDirectoryPicker) {
      let dir;
      try {
        dir = await showDirectoryPicker({ mode: 'readwrite' });
      } catch {
        return; // the player closed the picker
      }
      for (const e of list) {
        const blob = await NxrsSaves.exportFile(ctx.saveTitle, e, { titleId: ctx.titleId, title: ctx.title, engine: 'nxrs' });
        const h = await dir.getFileHandle(`${ctx.title} - ${playerName(e.save)} - ${day(Date.now())}.nxsave`.replace(/[\\/:*?"<>|]/g, '_'), { create: true });
        const s = await h.createWritable();
        await s.write(blob);
        await s.close();
      }
    } else {
      for (const e of list) await download(e);
    }
  }
  function pickImport(list) {
    const input = $('input', { type: 'file', accept: '.nxsave' });
    input.onchange = async () => {
      const f = input.files && input.files[0];
      if (!f) return;
      let got;
      try {
        got = await NxrsSaves.readExport(f, ctx.titleId, ctx.title);
      } catch (e) {
        ask({ title: 'Import save', heading: 'Cannot import this save', text: e.message || String(e), error: true, verb: 'OK', act: () => close_q() });
        return;
      }
      const there = list.find((e) => e.save === got.desc.save);
      const who = playerName(got.desc.save);
      ask({
        title: 'Import save',
        heading: there ? `Replace ${who}'s save?` : `Add ${who}'s save?`,
        text: there
          ? 'The game restarts to load it. The save there now is replaced; export it first to keep a copy.'
          : 'The game restarts to load it.',
        ghost: there ? ['Export…', () => download(there)] : null,
        verb: 'Import and restart',
        act: async () => {
          await NxrsSaves.importSave(ctx.saveTitle, got.desc.save, got.pack);
          location.reload();
        },
      });
    };
    input.click();
  }
  async function savesSection() {
    const box = $('div');
    box.append(group('saves', 'Saves', 'Kept in this browser for this game. A save appears here when the game writes one. Export keeps a copy as a file; Import and Delete restart the game, because the running game holds its save in memory and would write the old one back.'));
    let list = [];
    let failed = null;
    try {
      list = await NxrsSaves.list(ctx.saveTitle);
    } catch (e) {
      failed = `The saves cannot be read in this browser: ${e.message || e}`;
    }
    if (list.length) {
      const t = $('table', {}, $('tr', {}, $('th', {}, 'Save'), $('th', {}, 'Last written'), $('th', { class: 'r' }, 'Size'), $('th', { class: 'r' })));
      for (const e of list) {
        const how = e.imported ? `imported ${new Date(e.imported).toLocaleDateString([], { day: 'numeric', month: 'short' })}` : 'written by the game';
        t.append($('tr', {},
          $('td', {}, playerName(e.save), $('div', { class: 'sub' }, `${e.save} · ${how}`)),
          $('td', {}, when(e.written)),
          $('td', { class: 'r' }, kb(e.size)),
          $('td', { class: 'r' },
            btn('Export…', () => download(e)),
            btn('Delete…', () => ask({
              title: 'Delete save',
              heading: `Delete ${playerName(e.save)}'s save?`,
              text: 'It is removed from this browser and the game restarts without it, so it cannot write it back. Export it first to keep a copy.',
              ghost: ['Export…', () => download(e)],
              verb: 'Delete and restart',
              act: async () => {
                await NxrsSaves.remove(ctx.saveTitle, e.save);
                location.reload();
              },
            })))));
      }
      box.append(t);
    } else if (!failed) {
      box.append($('div', { class: 'status' }, 'No saves yet: the game has not written one.'));
    }
    if (lastSave && lastSave.failed) failed = `The last save could not be stored: ${lastSave.failed}`;
    if (failed) box.append($('div', { class: 'status err' }, failed));
    box.append($('div', { class: 'bar' },
      btn('Import save…', () => pickImport(list)),
      list.length ? btn('Export all…', () => exportAll(list)) : null));
    box.append(rule());
    box.append(group('game', 'Game', 'Restart starts the game again from its title screen; the saves stay as stored.'));
    box.append($('div', { class: 'bar' }, btn('Restart game…', () => ask({
      title: 'Restart game',
      heading: 'Restart the game?',
      text: 'It starts again from the title screen; saves stay as stored.',
      verb: 'Restart',
      act: () => location.reload(),
    }))));
    return box;
  }

  // ---------------------------------------------------------------- graphics, audio
  function graphicsSection() {
    return $('div', {},
      group('graphics', 'Picture', 'Applied as you change them; you see them when the game resumes. Kept for this game.'),
      row('Screen mode', 'TV draws for a television (up to 1920×1080); Handheld for the console\'s own screen (1280×720), lighter on the GPU. The game is told, as when a console is docked.',
        select('docked', [['1', 'TV (1920×1080)'], ['0', 'Handheld (1280×720)']])),
      row('Internal resolution', 'The game\'s 3D resolution: below Native is faster, above is sharper.',
        select('resolution', [['0.5', 'Half (0.5×)'], ['0.75', 'Three quarters (0.75×)'], ['1', 'Native (1×)'], ['1.5', 'One and a half (1.5×)'], ['2', 'Double (2×)']])),
      row('Dynamic resolution', 'Lowers the internal resolution by itself when the GPU cannot keep up, down to 60%.', toggle('dynamic_resolution', 'Dynamic resolution')),
      row('Shadows', null, toggle('shadows', 'Shadows')),
      row('Upscaling', 'How the picture is enlarged to the window: Sharp keeps edges crisp; Smooth is a plain filtered blit.',
        seg('upscale', [['sharp', 'Sharp'], ['standard', 'Smooth']], 'Upscaling')),
      rule(),
      group('rate', 'Frame rate', 'Chrome always shows images in step with the display: there is no separate vsync switch on the web.'),
      row('FPS limit', '30 shows every other image while the game keeps its speed; Unlimited lets the game run as fast as it can (Chrome still shows at most one image per display refresh).',
        seg('fps_limit', [['30', '30'], ['1', '60'], ['0', 'Unlimited']], 'FPS limit')));
  }
  function audioSection() {
    const st = NxrsAudio.state();
    return $('div', {},
      group('audio', 'Sound', 'Applied as you change them. Kept for every game on this browser.'),
      row('Volume', null, slider('volume', 0, 100, 5, 'Volume')),
      row('Mute', null, toggle('mute', 'Mute')),
      row('Mute in the background', 'Silent while this tab or window is hidden or behind another window; the sound comes back with it.', toggle('mute_on_focus_loss', 'Mute in the background')),
      row('Smooth slowdown', 'When the game cannot keep up, the sound slows down smoothly instead of crackling.', toggle('audio_slowdown', 'Smooth slowdown')),
      $('div', { class: 'status' }, st.running ? 'Sound is on.' : 'Sound starts at the first click or key.'));
  }

  // ---------------------------------------------------------------- controller
  const KEY_DEFAULT = {
    x: 'a', space: 'a', z: 'b', c: 'x', v: 'y', q: 'l', e: 'r', 1: 'zl', 3: 'zr', enter: 'plus', backspace: 'minus',
    left: 'left', up: 'up', right: 'right', down: 'down', a: 'lsleft', w: 'lsup', d: 'lsright', s: 'lsdown',
    j: 'rsleft', i: 'rsup', l: 'rsright', k: 'rsdown',
  };
  const CODE_NAME = {
    Space: 'space', Enter: 'enter', NumpadEnter: 'enter', Backspace: 'backspace', Tab: 'tab', ShiftLeft: 'lshift',
    ShiftRight: 'rshift', ControlLeft: 'lctrl', ControlRight: 'rctrl', AltLeft: 'lalt', AltRight: 'ralt',
    ArrowLeft: 'left', ArrowUp: 'up', ArrowRight: 'right', ArrowDown: 'down', PageUp: 'pageup', PageDown: 'pagedown',
    End: 'end', Home: 'home', Insert: 'insert', Delete: 'delete', Minus: 'minus', Equal: 'equal', Comma: 'comma',
    Period: 'period', Slash: 'slash', Semicolon: 'semicolon', Quote: 'quote', BracketLeft: 'bracketleft',
    BracketRight: 'bracketright', Backslash: 'backslash', Backquote: 'backquote',
  };
  const codeName = (code) => {
    if (CODE_NAME[code]) return CODE_NAME[code];
    let m = /^Key([A-Z])$/.exec(code);
    if (m) return m[1].toLowerCase();
    m = /^Digit(\d)$/.exec(code);
    if (m) return m[1];
    m = /^Numpad(\d)$/.exec(code);
    return m ? 'num' + m[1] : null;
  };
  const keyLabel = (n) => ({ space: 'Space', enter: 'Enter', backspace: 'Backspace', left: '←', up: '↑', right: '→', down: '↓', lshift: 'Shift', rshift: 'Right Shift', lctrl: 'Ctrl', rctrl: 'Right Ctrl', lalt: 'Alt', ralt: 'Right Alt', tab: 'Tab' }[n] || n.toUpperCase());
  const parseMap = (text) => {
    const out = {};
    for (const t of String(text || '').toLowerCase().split(/[\s,;]+/)) {
      const i = t.indexOf(':');
      if (i > 0) out[t.slice(0, i)] = t.slice(i + 1);
    }
    return out;
  };
  const keyTable = () => ({ ...KEY_DEFAULT, ...parseMap(NxrsSettings.get('key_map')) });
  function bindKey(name, guest) {
    const t = keyTable();
    for (const k of Object.keys(t)) if (t[k] === guest) t[k] = 'none';
    t[name] = guest;
    const diff = [];
    for (const k of new Set([...Object.keys(t), ...Object.keys(KEY_DEFAULT)])) {
      const v = t[k] || 'none';
      if (v !== (KEY_DEFAULT[k] || 'none')) diff.push(`${k}:${v}`);
    }
    NxrsSettings.set('key_map', diff.join(' '));
  }
  // the standard mapping's buttons as layout.rs's physical names
  const PHYS = ['south', 'east', 'west', 'north', 'l', 'r', 'zl', 'zr', 'minus', 'plus', 'lstick', 'rstick', 'up', 'down', 'left', 'right'];
  const padBase = () => {
    const b = {};
    for (const p of PHYS) b[p] = p;
    Object.assign(b, NxrsSettings.get('button_layout') === 'xbox'
      ? { south: 'a', east: 'b', west: 'x', north: 'y' }
      : { east: 'a', south: 'b', north: 'x', west: 'y' });
    return b;
  };
  const ALIAS = { cross: 'south', circle: 'east', square: 'west', triangle: 'north' };
  const padTable = () => {
    const t = padBase();
    for (const [p, g] of Object.entries(parseMap(NxrsSettings.get('button_map')))) t[ALIAS[p] || p] = g;
    return t;
  };
  function bindPad(phys, guest) {
    const t = padTable();
    for (const p of PHYS) if (t[p] === guest) t[p] = 'none';
    t[phys] = guest;
    const base = padBase();
    NxrsSettings.set('button_map', PHYS.filter((p) => t[p] !== base[p]).map((p) => `${p}:${t[p]}`).join(' '));
  }
  function padNames() {
    const g = NxrsInput.firstPad();
    const id = g ? g.id : '';
    if (/054c|dualsense|dualshock|playstation/i.test(id)) {
      return { south: 'Cross', east: 'Circle', west: 'Square', north: 'Triangle', l: 'L1', r: 'R1', zl: 'L2', zr: 'R2', minus: 'Create', plus: 'Options', lstick: 'L3', rstick: 'R3' };
    }
    if (/057e|nintendo|pro controller|joy-con/i.test(id)) {
      return { south: 'B', east: 'A', west: 'Y', north: 'X', l: 'L', r: 'R', zl: 'ZL', zr: 'ZR', minus: 'Minus', plus: 'Plus', lstick: 'L-stick', rstick: 'R-stick' };
    }
    return { south: 'A', east: 'B', west: 'X', north: 'Y', l: 'LB', r: 'RB', zl: 'LT', zr: 'RT', minus: 'View', plus: 'Menu', lstick: 'LS', rstick: 'RS' };
  }
  const GUESTS = [
    ['a', 'A'], ['b', 'B'], ['x', 'X'], ['y', 'Y'], ['l', 'L'], ['r', 'R'], ['zl', 'ZL'], ['zr', 'ZR'], ['plus', 'Plus'],
    ['minus', 'Minus'], ['up', 'D-pad up'], ['down', 'D-pad down'], ['left', 'D-pad left'], ['right', 'D-pad right'],
    ['lstick', 'Left stick press'], ['rstick', 'Right stick press'],
    ['lsup', 'Left stick up', 'Left stick'], ['lsdown', 'Left stick down', 'Left stick'], ['lsleft', 'Left stick left', 'Left stick'], ['lsright', 'Left stick right', 'Left stick'],
    ['rsup', 'Right stick up', 'Right stick'], ['rsdown', 'Right stick down', 'Right stick'], ['rsleft', 'Right stick left', 'Right stick'], ['rsright', 'Right stick right', 'Right stick'],
  ];
  function mapRow([guest, label, stick]) {
    const keys = Object.entries(keyTable()).filter(([, g]) => g === guest).map(([k]) => k);
    const names = padNames();
    const pt = padTable();
    const phys = PHYS.filter((p) => pt[p] === guest);
    const padLabel = (p) => names[p] || ({ up: 'D-pad up', down: 'D-pad down', left: 'D-pad left', right: 'D-pad right' }[p]);
    const kcell = $('button', { class: 'key', 'data-nav': '', 'aria-label': `${label}: keyboard`, onclick: (ev) => startCapture('key', guest, ev.currentTarget) },
      keys.length ? keys.map(keyLabel).join(' ') : 'none');
    const pcell = stick
      ? $('span', { class: 'key fixed' }, stick)
      : $('button', { class: 'key', 'data-nav': '', 'aria-label': `${label}: gamepad`, onclick: (ev) => startCapture('pad', guest, ev.currentTarget) },
        phys.length ? phys.map(padLabel).join(' ') : 'none');
    return $('tr', {}, $('td', {}, label), $('td', {}, kcell), $('td', {}, pcell));
  }
  function controllerSection() {
    const g = NxrsInput.firstPad();
    const half = Math.ceil(GUESTS.length / 2);
    const table = (rows) => $('table', { class: 'map' }, $('tr', {}, $('th', {}, 'Switch'), $('th', {}, 'Keyboard'), $('th', {}, 'Gamepad')), rows.map(mapRow));
    return $('div', {},
      group('controller', 'Controller', 'The keyboard and the first gamepad play as player 1. Kept for every game on this browser.'),
      row('Gamepad', null, $('span', { class: 'readout' }, g ? g.id : 'No gamepad: press a button on one to connect it')),
      row('Face buttons', 'By position: the right button is A, the bottom one B, as on a Switch. By label: the button marked A on an Xbox pad is A.',
        seg('button_layout', [['switch', 'By position (Switch)'], ['xbox', 'By label (Xbox)']], 'Face buttons')),
      row('Stick dead zone', 'How far a stick must move before the game sees it.', slider('stick_deadzone', 0, 30, 1, 'Stick dead zone')),
      $('div', { class: 'rule', style: 'margin:12px 0' }),
      group('buttons', 'Buttons', 'Choose a key or a gamepad button, then press the one that should do it; a key or button already used elsewhere moves here. Esc (or B on a gamepad while choosing a key; Home while choosing a gamepad button) leaves it unchanged.'),
      $('div', { class: 'cols' }, table(GUESTS.slice(0, half)), table(GUESTS.slice(half))),
      $('div', { class: 'bar' }, btn('Restore default buttons', () => {
        NxrsSettings.set('key_map', '');
        NxrsSettings.set('button_map', '');
        show(section);
      })));
  }
  function startCapture(kind, guest, el) {
    capture = { kind, guest, el, prev: NxrsInput.firstPad() ? NxrsInput.padBits(NxrsInput.firstPad()) : 0 };
    el.classList.add('wait');
    el.textContent = kind === 'key' ? 'Press a key…' : 'Press a button…';
  }
  function endCapture(bind) {
    const c = capture;
    capture = null;
    if (bind) bind(c);
    show(section, c.el.getAttribute('aria-label'));
  }

  // ---------------------------------------------------------------- the window
  function close_q() {
    if (question) question.remove();
    question = null;
    focusFirst();
  }
  // A question over the menu (style guide: title names the action, heading the question, footer the dialog's own
  // ghost verb at the leading side and one filled verb last; X / Esc / B dismiss).
  function ask({ title, heading, text, ghost, verb, act, error }) {
    close_q();
    question = $('div', {},
      $('div', { class: 'dim' }),
      $('div', { class: 'q', role: 'dialog', 'aria-label': title },
        $('div', { class: 'head' }, $('span', { class: 't' }, title), $('span', { class: 'sp' }), xButton(`Close ${title}`, close_q)),
        $('div', { class: 'qb' }, group(error ? 'error' : 'question', heading, null, error), $('p', {}, text)),
        $('div', { class: 'foot' },
          ghost ? btn(ghost[0], ghost[1]) : null,
          $('span', { class: 'sp' }),
          btn(verb, async () => {
            try {
              await act();
            } catch (e) {
              ask({ title, heading: 'It did not work', text: String(e.message || e), error: true, verb: 'OK', act: () => close_q() });
            }
          }, 'primary'))));
    root.append(question);
    const p = question.querySelector('.primary');
    if (p) p.focus();
  }
  function xButton(name, on) {
    const b = $('button', { class: 'x', title: name, 'aria-label': name, 'data-nav': '', onclick: on });
    b.innerHTML = X_SVG;
    return b;
  }
  async function show(sec, refocus) {
    section = sec;
    for (const b of rail.children) b.classList.toggle('on', b.dataset.sec === sec);
    const content = sec === 'saves' ? await savesSection()
      : sec === 'graphics' ? graphicsSection()
        : sec === 'audio' ? audioSection() : controllerSection();
    if (section !== sec) return;
    form.replaceChildren(content);
    const again = refocus && form.querySelector(`[aria-label="${CSS.escape(refocus)}"]`);
    if (again) again.focus();
  }
  function build() {
    rail = $('nav', { class: 'rail', 'aria-label': 'Sections' }, SECTIONS.map(([id, label]) =>
      $('button', { 'data-sec': id, 'data-nav': '', onclick: () => show(id) }, svg(ICON[id]), label)));
    form = $('section', { class: 'form' });
    root = $('div', { id: 'nx-menu', hidden: true },
      $('div', { class: 'scrim' }),
      $('div', { class: 'win', role: 'dialog', 'aria-label': ctx.title },
        $('div', { class: 'head' }, $('span', { class: 't' }, ctx.title), $('span', { class: 'chip' }, 'Paused'), $('span', { class: 'sp' }),
          xButton('Resume game (Esc)', () => setOpen(false))),
        $('div', { class: 'body' }, rail, form)));
    document.body.append(root);
  }
  function navItems() {
    const scope = question || root;
    return [...scope.querySelectorAll('[data-nav]')].filter((e) => e.offsetParent !== null);
  }
  function focusFirst() {
    const on = rail && rail.querySelector('.on');
    (question ? question.querySelector('.primary') : on || navItems()[0])?.focus();
  }
  function move(d) {
    const items = navItems();
    if (!items.length) return;
    const i = items.indexOf(document.activeElement);
    const j = i < 0 ? 0 : Math.max(0, Math.min(items.length - 1, i + d));
    items[j].focus();
    items[j].scrollIntoView({ block: 'nearest' });
  }
  function step(d) {
    const a = document.activeElement;
    if (a && a.nxStep) a.nxStep(d);
    else if (a && a.closest('.slider')) a.closest('.slider').querySelector('input').nxStep(d);
    else move(d);
  }
  function nextSection(d) {
    const i = SECTIONS.findIndex(([id]) => id === section);
    const j = (i + d + SECTIONS.length) % SECTIONS.length;
    show(SECTIONS[j][0]).then(() => rail.children[j].focus());
  }
  function back() {
    if (question) close_q();
    else setOpen(false);
  }
  function onKey(e) {
    if (capture) {
      e.preventDefault();
      if (e.code === 'Escape') return endCapture(null);
      if (capture.kind !== 'key') return;
      const n = codeName(e.code);
      if (n) endCapture((c) => bindKey(n, c.guest));
      return;
    }
    const k = e.code;
    if (k === 'Escape') { e.preventDefault(); back(); return; }
    if (k === 'Tab' && e.ctrlKey) { e.preventDefault(); nextSection(e.shiftKey ? -1 : 1); return; }
    if (k === 'Tab') { e.preventDefault(); move(e.shiftKey ? -1 : 1); return; }
    if (k === 'ArrowDown') { e.preventDefault(); move(1); return; }
    if (k === 'ArrowUp') { e.preventDefault(); move(-1); return; }
    const a = document.activeElement;
    if (k === 'ArrowLeft' || k === 'ArrowRight') { e.preventDefault(); step(k === 'ArrowLeft' ? -1 : 1); return; }
    if ((k === 'Enter' || k === 'Space' || k === 'NumpadEnter') && a && a.tagName === 'BUTTON') { e.preventDefault(); a.click(); return; }
    if (k === 'Enter' || k === 'Space') e.preventDefault();
  }
  // the gamepad in the menu: D-pad / left stick move, A acts (by the chosen face-button layout), B or Home closes,
  // L / R change the section
  function padLoop() {
    if (!open) return;
    const g = NxrsInput.firstPad();
    let b = g ? NxrsInput.padBits(g) : 0;
    if (g) {
      const ax = g.axes || [];
      if ((ax[1] || 0) < -0.6) b |= 1 << 12;
      if ((ax[1] || 0) > 0.6) b |= 1 << 13;
      if ((ax[0] || 0) < -0.6) b |= 1 << 14;
      if ((ax[0] || 0) > 0.6) b |= 1 << 15;
    }
    const pressed = b & ~padPrev;
    const now = performance.now();
    let rep = 0;
    if (b & 0xf000 && now >= padRepeat) {
      rep = b & 0xf000 & padPrev;
      if (rep) padRepeat = now + 120;
    }
    if (pressed & 0xf000) padRepeat = now + 400;
    padPrev = b;
    const hit = (i) => ((pressed | rep) >> i & 1) !== 0;
    const xbox = NxrsSettings.get('button_layout') === 'xbox';
    const A = xbox ? 0 : 1;
    const B = xbox ? 1 : 0;
    if (capture) {
      if (capture.kind === 'pad') {
        if (pressed >> 16 & 1) endCapture(null);
        else {
          for (let i = 0; i < 16; i++) if (pressed >> i & 1) { endCapture((c) => bindPad(PHYS[i], c.guest)); break; }
        }
      } else if (pressed >> B & 1) endCapture(null);
    } else {
      if (hit(12)) move(-1);
      if (hit(13)) move(1);
      if (hit(14)) step(-1);
      if (hit(15)) step(1);
      if (pressed >> 4 & 1) nextSection(-1);
      if (pressed >> 5 & 1) nextSection(1);
      if (pressed >> A & 1) document.activeElement?.click?.();
      if (pressed >> B & 1 || pressed >> 16 & 1) back();
    }
    requestAnimationFrame(padLoop);
  }
  function setOpen(on) {
    if (on === open || (on && !ready)) return;
    open = on;
    if (!root) build();
    root.hidden = !on;
    NxrsInput.setMenu(on);
    if (on) {
      NxrsAudio.suspend();
      const g = NxrsInput.firstPad();
      padPrev = g ? NxrsInput.padBits(g) : 0;
      show(section).then(focusFirst);
      requestAnimationFrame(padLoop);
      ctx.log('[web] menu: opened, the game is paused');
    } else {
      close_q();
      capture = null;
      NxrsAudio.resume();
      document.activeElement?.blur?.();
      ctx.log('[web] menu: closed, the game runs');
    }
  }

  // Esc in page fullscreen: the page holds it so it reaches the menu (holding Esc still leaves fullscreen)
  document.addEventListener('fullscreenchange', () => {
    if (document.fullscreenElement && navigator.keyboard && navigator.keyboard.lock) navigator.keyboard.lock(['Escape']).catch(() => {});
  });

  // the button table's gamepad names follow the face-button layout
  NxrsSettings.onChange((k) => {
    if (k === 'button_layout' && open && section === 'controller') show('controller', 'Face buttons');
  });

  return {
    init(c) {
      ctx = c;
      NxrsInput.hook(() => setOpen(true), onKey);
    },
    // the game shows its first picture: the menu can open from now on
    ready() { ready = true; },
    open: () => setOpen(true),
    close: () => setOpen(false),
    isOpen: () => open,
    // the I/O worker stored a save, or could not
    saved(m) {
      lastSave = m;
      if (open && section === 'saves' && !question && !capture) show('saves');
    },
  };
})();
