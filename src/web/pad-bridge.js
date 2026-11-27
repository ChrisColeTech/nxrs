// The pads' rumble and lights through the launcher (WEB-PADBRIDGE). A page the launcher serves opens a WebSocket to
// `pad-bridge` beside it; the launcher runs `nxrs-build pad-bridge`, which drives the real pads with the native
// engine's input layer (DualSense / DualShock 4 output reports with the player lights, XInput, Nintendo pads): no
// browser permission, the same reports and lights as a native run. Input stays on the Gamepad API.
//
// Line: `pad <active> <low 1> <high 1> .. <low 4> <high 4>`: active = the game owns input (page focused, visible,
// menu closed; natively the window's focus), the levels the engine's words [56..64) (0..65535). Sent when it
// changes and every HEARTBEAT_MS (the bridge stops the motors and turns the lights off after 1 s without a line).
// While the socket is open input.js plays nothing itself; without it (dev server, a static host, an older launcher)
// its vibrationActuator / WebHID paths work as before.
'use strict';
const NxrsPadBridge = (() => {
  const HEARTBEAT_MS = 250;
  const RETRY_MS = 3000;
  let ws = null;
  let open = false;
  let everOpen = false;
  let last = '';
  let sentAt = 0;
  let logged = 0;
  let onOpen = () => {};

  function log(line) {
    if (logged < 100) {
      logged++;
      console.log(`[nxrs] [web] ${line}`);
    }
  }
  // the route beside the page: ws://host/<slug>/pad-bridge for http://host/<slug>/index.html
  function url(loc) {
    const dir = loc.pathname.slice(0, loc.pathname.lastIndexOf('/') + 1);
    return `${loc.protocol === 'https:' ? 'wss:' : 'ws:'}//${loc.host}${dir}pad-bridge`;
  }
  function line(active, levels) {
    const v = [];
    for (let i = 0; i < 8; i++) v.push(active ? Math.max(0, Math.min(65535, levels[i] | 0)) : 0);
    return `pad ${active ? 1 : 0} ${v.join(' ')}`;
  }
  // a line is due when it differs from the last one sent or the heartbeat is due
  function due(text, now) {
    return text !== last || now - sentAt >= HEARTBEAT_MS;
  }
  function send(text, now) {
    if (!open) return;
    try {
      ws.send(text);
      if (text !== last) log(`pads (launcher): ${text}`);
      last = text;
      sentAt = now;
    } catch (e) {
      log(`pads (launcher): send failed: ${e}`);
    }
  }
  function connect() {
    if (typeof WebSocket !== 'function' || typeof location === 'undefined' || !/^https?:$/.test(location.protocol)) return;
    try {
      ws = new WebSocket(url(location));
    } catch (e) {
      return;
    }
    ws.onopen = () => {
      open = true;
      everOpen = true;
      last = '';
      log('pads (launcher): rumble and lights through the launcher\'s pad bridge (the engine\'s native pad code)');
      onOpen();
    };
    ws.onclose = () => {
      const was = open;
      open = false;
      ws = null;
      if (was) log('pads (launcher): bridge closed; the page\'s own rumble again');
      // a bridge that was there may come back (the launcher restarts its child); none at all: the page's own paths
      if (everOpen) setTimeout(connect, RETRY_MS);
    };
    ws.onerror = () => {};
  }
  // every poll: the state now (sent when due)
  function update(active, levels) {
    const now = performance.now();
    const text = line(active, levels);
    if (due(text, now)) send(text, now);
  }
  // stop at once: menu open, blur, hidden, closing
  function stop(why) {
    if (!open) return;
    const text = line(false, []);
    if (text !== last) log(`pads (launcher): stop (${why})`);
    send(text, performance.now());
  }
  if (typeof addEventListener === 'function') {
    addEventListener('pagehide', () => {
      stop('page closing');
      if (ws) ws.close();
    });
    // the heartbeat when no poll runs (a hidden page has no animation frames)
    setInterval(() => {
      if (open && performance.now() - sentAt >= HEARTBEAT_MS) send(last || line(false, []), performance.now());
    }, HEARTBEAT_MS);
    connect();
  }
  return {
    url,
    line,
    due,
    update,
    stop,
    drives: () => open,
    onOpen: (f) => { onOpen = f; },
    _set: (s, t) => { last = s; sentAt = t; },
  };
})();
if (typeof module !== 'undefined') module.exports = NxrsPadBridge;
