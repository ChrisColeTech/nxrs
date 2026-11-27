// The DualSense's motors through WebHID (WEB-RUMBLE-HID). Chrome on Windows gives a DualSense no vibrationActuator,
// so input.js hands such a pad's motor levels (the engine's words, 0..65535) to this file, which writes the output
// report the native engine writes (os/src/input/sony.rs `output`): USB report 0x02, Bluetooth 0x31 (sequence, CRC).
// Only the motors: valid flags 0x03 / 0x00, so the pad's lights and triggers are left as they are.
//
// Chrome asks the player once (the menu's "Allow…" button calls `allow`); a pad allowed before reopens by itself on
// later visits (`navigator.hid.getDevices`, and `connect` when it is plugged in again).
'use strict';
const NxrsHid = (() => {
  const SONY = 0x054c;
  const PRODUCTS = { 0x0ce6: 'DualSense', 0x0df2: 'DualSense Edge' };
  const FILTERS = Object.keys(PRODUCTS).map((p) => ({ vendorId: SONY, productId: Number(p) }));
  const USB = 0x02;
  const BT = 0x31;
  const USB_BYTES = 47; // report 0x02 after its id, when the descriptor does not say
  const BT_BYTES = 77;
  const hid = typeof navigator !== 'undefined' && navigator.hid ? navigator.hid : null;
  const devs = []; // { device, transport, len, v2, seq, sent: [low, high] | null, busy, next }
  let logged = 0;

  function log(line) {
    if (logged < 200) {
      logged++;
      console.log(`[nxrs] [web] ${line}`);
    }
  }

  // CRC-32 (reflected, 0xEDB88320) over `seed` then `bytes`, as Sony's Bluetooth reports carry it (sony.rs `crc`).
  function crc32(seed, bytes) {
    let c = 0xffffffff;
    const step = (b) => {
      c ^= b;
      for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
    };
    step(seed);
    for (const b of bytes) step(b);
    return ~c >>> 0;
  }

  // an engine word (0..65535) to the report's 0..255, as native rounds amp x 255
  const level = (word) => Math.max(0, Math.min(255, Math.floor((Number(word) || 0) * 255 / 65535 + 0.5)));

  // The output report for `transport` (USB or BT): { id, data } with `data` the bytes after the report id, as
  // HIDDevice.sendReport takes them. `low` / `high` are 0..255; `len` the USB report's size after its id.
  function report(transport, seq, low, high, v2, len = USB_BYTES) {
    const bt = transport === BT;
    const data = new Uint8Array(bt ? BT_BYTES : Math.max(len, USB_BYTES));
    const o = bt ? 2 : 0; // sony.rs: the packet's byte o + 1 is the data's byte o
    if (bt) {
      data[0] = (seq & 15) << 4;
      data[1] = 0x10;
    }
    data[o] = 0x03; // compatible vibration, haptics select
    data[o + 1] = 0x00; // no lightbar / player LED / mute LED control
    data[o + 2] = high & 255; // the right (light) motor
    data[o + 3] = low & 255; // the left (heavy) motor
    if (v2) data[o + 38] = 0x04; // compatible vibration 2 (firmware 2.21 and later)
    if (bt) {
      const c = crc32(0xa2, [BT, ...data.subarray(0, 73)]);
      data[73] = c & 255;
      data[74] = c >>> 8 & 255;
      data[75] = c >>> 16 & 255;
      data[76] = c >>> 24 & 255;
    }
    return { id: bt ? BT : USB, data };
  }

  // the size in bytes of output report `id` from the device's collections (0 when it has none)
  function outputBytes(device, id) {
    let bits = 0;
    const walk = (cols) => {
      for (const c of cols || []) {
        for (const r of c.outputReports || []) {
          if (r.reportId === id) for (const it of r.items || []) bits += it.reportSize * it.reportCount;
        }
        walk(c.children);
      }
    };
    walk(device.collections);
    return bits >> 3;
  }

  // sony.rs `vibration_v2`: feature report 0x20's update version (bytes 44-45 counting the id) >= 2.21; always on the
  // Edge, on when the report cannot be read. Chrome may or may not return the id as the first byte.
  async function vibrationV2(device) {
    if (device.productId === 0x0df2) return true;
    try {
      const v = await device.receiveFeatureReport(0x20);
      const off = v.byteLength >= 46 && v.getUint8(0) === 0x20 ? 44 : 43;
      if (v.byteLength < off + 2) return true;
      return v.getUint16(off, true) >= 0x0215;
    } catch {
      return true;
    }
  }

  const known = (d) => d.vendorId === SONY && PRODUCTS[d.productId] !== undefined;

  async function add(device) {
    if (!known(device) || devs.some((d) => d.device === device)) return;
    try {
      if (!device.opened) await device.open();
    } catch (e) {
      log(`rumble: ${device.productName || 'DualSense'} could not be opened: ${e}`);
      return;
    }
    if (devs.some((d) => d.device === device)) return;
    const usb = outputBytes(device, USB);
    const transport = usb ? USB : outputBytes(device, BT) ? BT : USB;
    const d = { device, transport, len: usb || USB_BYTES, v2: await vibrationV2(device), seq: 0, sent: null, busy: false, next: null };
    devs.push(d);
    log(`rumble: ${device.productName || PRODUCTS[device.productId]} opened through WebHID (${transport === BT ? 'Bluetooth' : 'USB'}, report ${hex(transport)}, ${transport === BT ? BT_BYTES : d.len} bytes, vibration v2 ${d.v2 ? 'on' : 'off'})`);
    changed();
  }
  const hex = (n) => '0x' + n.toString(16).padStart(2, '0');

  // one report in flight per device: a newer level waits and replaces an older waiting one
  function send(d, low, high, why) {
    const same = d.sent && d.sent[0] === low && d.sent[1] === high;
    if (d.busy) {
      d.next = same ? null : [low, high, why];
      return;
    }
    if (same) return;
    d.busy = true;
    d.sent = [low, high];
    const r = report(d.transport, d.seq++, low, high, d.v2, d.len);
    const bytes = [...r.data.subarray(0, 6)].map((b) => b.toString(16).padStart(2, '0')).join(' ');
    log(`rumble ${d.device.productName || 'DualSense'}: report ${hex(r.id)} low ${low} high ${high} (of 255)${why ? ` (${why})` : ''} [${bytes} ...]`);
    d.device.sendReport(r.id, r.data)
      .catch((e) => log(`rumble ${d.device.productName || 'DualSense'}: sendReport failed: ${e}`))
      .finally(() => {
        d.busy = false;
        const n = d.next;
        d.next = null;
        if (n) send(d, n[0], n[1], n[2]);
      });
  }

  // "Vendor: 054c Product: 0ce6" in a Gamepad.id to the WebHID product it is (0 for another pad)
  function product(id) {
    const m = /Vendor: *([0-9a-f]{4}) *Product: *([0-9a-f]{4})/i.exec(id || '');
    if (!m || parseInt(m[1], 16) !== SONY) return 0;
    const p = parseInt(m[2], 16);
    return PRODUCTS[p] !== undefined ? p : 0;
  }

  // Each poll: the pads without an actuator, with their player's words. The n-th gamepad of a product takes the n-th
  // open device of that product; an open device no pad takes is stopped.
  function play(pads) {
    const used = new Set();
    for (const { gamepad, low, high } of pads) {
      const p = product(gamepad.id);
      const d = devs.find((x) => x.device.productId === p && !used.has(x));
      if (!d) continue;
      used.add(d);
      send(d, level(low), level(high));
    }
    for (const d of devs) if (!used.has(d) && d.sent && (d.sent[0] || d.sent[1])) send(d, 0, 0, 'no gamepad');
  }

  // every open pad's motors off now (menu open, blur, hidden, page leaving)
  function stopAll(why) {
    for (const d of devs) {
      if (d.busy) d.next = [0, 0, why];
      else if (d.sent && (d.sent[0] || d.sent[1])) send(d, 0, 0, why);
    }
  }

  // The menu's "Allow…": Chrome's device chooser (needs the click's user activation), then open what was chosen.
  async function allow() {
    if (!hid) throw new Error('This browser has no WebHID');
    log(`rumble: requestDevice ${JSON.stringify(FILTERS.map((f) => ({ vendorId: hex4(f.vendorId), productId: hex4(f.productId) })))}`);
    const chosen = await hid.requestDevice({ filters: FILTERS });
    log(`rumble: requestDevice chose ${chosen.length} device(s)`);
    for (const d of chosen) await add(d);
    return chosen.length;
  }
  const hex4 = (n) => '0x' + n.toString(16).padStart(4, '0');

  const listeners = [];
  function changed() {
    for (const f of listeners) f();
  }

  if (hid) {
    hid.getDevices().then((list) => {
      const mine = list.filter(known);
      if (mine.length) log(`rumble: ${mine.length} controller(s) allowed before, reopening`);
      for (const d of mine) add(d);
    }).catch((e) => log(`rumble: getDevices failed: ${e}`));
    hid.addEventListener('connect', (e) => add(e.device));
    hid.addEventListener('disconnect', (e) => {
      const i = devs.findIndex((d) => d.device === e.device);
      if (i >= 0) {
        devs.splice(i, 1);
        log(`rumble: ${e.device.productName || 'DualSense'} disconnected`);
        changed();
      }
    });
  }
  if (typeof addEventListener === 'function') addEventListener('pagehide', () => stopAll('page closing'));

  return {
    FILTERS,
    report,
    level,
    crc32,
    product,
    play,
    stopAll,
    allow,
    // the menu: WebHID exists, a gamepad id it can drive, the open devices' names, a hook for changes
    available: () => hid !== null,
    drives: (gamepad) => hid !== null && product(gamepad.id) !== 0,
    opened: () => devs.map((d) => d.device.productName || PRODUCTS[d.device.productId]),
    onChange: (f) => listeners.push(f),
  };
})();
if (typeof module !== 'undefined') module.exports = NxrsHid;
