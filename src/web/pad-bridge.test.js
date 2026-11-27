// pad-bridge.js's lines and rules, against os/src/input/bridge.rs `parse` (not shipped; run with node:
// build\tools\emsdk\node\<version>\node.exe src\web\pad-bridge.test.js).
'use strict';
const assert = require('assert');
const B = require('./pad-bridge.js');

const tests = {
  'the route sits beside the page'() {
    assert.strictEqual(B.url({ protocol: 'http:', host: '127.0.0.1:8780', pathname: '/beach-bounce/index.html' }),
      'ws://127.0.0.1:8780/beach-bounce/pad-bridge');
    assert.strictEqual(B.url({ protocol: 'http:', host: '127.0.0.1:8780', pathname: '/beach-bounce/' }),
      'ws://127.0.0.1:8780/beach-bounce/pad-bridge');
    assert.strictEqual(B.url({ protocol: 'https:', host: 'x.example', pathname: '/a/b/index.html' }),
      'wss://x.example/a/b/pad-bridge');
  },
  'a line: active, then low and high of players 1-4 (bridge.rs parse)'() {
    assert.strictEqual(B.line(true, [32768, 0, 0, 0, 0, 0, 0, 65535]), 'pad 1 32768 0 0 0 0 0 0 65535');
    assert.strictEqual(B.line(true, [70000, -3, undefined]), 'pad 1 65535 0 0 0 0 0 0 0', 'clamped, missing = 0');
  },
  'not active: every level 0 (the bridge then turns the lights off and stops the motors)'() {
    assert.strictEqual(B.line(false, [32768, 32768, 1, 1, 1, 1, 1, 1]), 'pad 0 0 0 0 0 0 0 0 0');
  },
  'sent on a change or when the heartbeat is due, not every frame'() {
    B._set('pad 1 0 0 0 0 0 0 0 0', 1000);
    assert.strictEqual(B.due('pad 1 0 0 0 0 0 0 0 0', 1100), false);
    assert.strictEqual(B.due('pad 1 0 0 0 0 0 0 0 0', 1250), true, 'heartbeat after 250 ms');
    assert.strictEqual(B.due('pad 1 32768 0 0 0 0 0 0 0', 1010), true, 'a change at once');
  },
  'no socket: the page keeps its own rumble'() {
    assert.strictEqual(B.drives(), false);
  },
};

let failed = 0;
for (const [name, f] of Object.entries(tests)) {
  try {
    f();
    console.log(`ok   ${name}`);
  } catch (e) {
    failed++;
    console.log(`FAIL ${name}\n     ${e.message}`);
  }
}
console.log(`${Object.keys(tests).length - failed}/${Object.keys(tests).length} passed`);
process.exit(failed ? 1 : 0);
