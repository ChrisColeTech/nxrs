// The DualSense output report hid-rumble.js builds, against os/src/input/sony.rs `output` (not shipped; run with node:
// build\tools\emsdk\node\<version>\node.exe src\web\hid-rumble.test.js). sony.rs indexes the whole packet (id at
// [0]); the page's report data starts after the id, so the packet's [k] is data[k - 1].
'use strict';
const assert = require('assert');
const H = require('./hid-rumble.js');

const tests = {
  'CRC-32 matches the standard (sony.rs crc32_matches_the_standard)'() {
    assert.strictEqual(H.crc32('1'.charCodeAt(0), [...'23456789'].map((c) => c.charCodeAt(0))), 0xcbf43926);
  },
  'engine words to 0..255 as native rounds amp x 255'() {
    assert.strictEqual(H.level(0), 0);
    assert.strictEqual(H.level(65535), 255);
    assert.strictEqual(H.level(32768), 128, 'the self-test half level: native (0.5 * 255 + 0.5) as u32 = 128');
    assert.strictEqual(H.level(257), 1);
    assert.strictEqual(H.level(128), 0);
    assert.strictEqual(H.level(70000), 255);
    assert.strictEqual(H.level(-5), 0);
    assert.strictEqual(H.level(undefined), 0);
  },
  'USB: report 0x02, 47 bytes, flags, high then low (sony.rs dualsense_usb_lights_packet)'() {
    const r = H.report(0x02, 0, 255, 128, false);
    assert.strictEqual(r.id, 0x02);
    assert.strictEqual(r.data.length, 47, 'sony.rs: 48 with the id');
    assert.deepStrictEqual([...r.data.subarray(0, 4)], [0x03, 0x00, 128, 255], 'packet [1..5): flags 03, no light flags, high (right) then low (left)');
    assert.ok(r.data.subarray(4).every((b) => b === 0), 'nothing else: no lights, no trigger effects, no v2');
    const v2 = H.report(0x02, 0, 0, 0, true);
    assert.strictEqual(v2.data[38], 4, 'packet [39] = 4: compatible vibration 2');
    assert.strictEqual(H.report(0x02, 0, 1, 2, false, 62).data.length, 62, 'the descriptor\'s size when larger (sony.rs: 63 with the id)');
  },
  'Bluetooth: report 0x31, 77 bytes, sequence, flag 0x10, motors shifted, signed (sony.rs bluetooth_packets_are_signed)'() {
    const r = H.report(0x31, 3, 10, 20, false);
    assert.strictEqual(r.id, 0x31);
    assert.strictEqual(r.data.length, 77, 'sony.rs: 78 with the id');
    assert.deepStrictEqual([...r.data.subarray(0, 6)], [0x30, 0x10, 0x03, 0x00, 20, 10], 'packet [1..7): seq << 4, 0x10, flags, high, low');
    const crc = H.crc32(0xa2, [0x31, ...r.data.subarray(0, 73)]);
    const stored = (r.data[73] | r.data[74] << 8 | r.data[75] << 16 | r.data[76] << 24) >>> 0;
    assert.strictEqual(stored, crc, 'CRC at packet [74..78), little-endian');
    assert.strictEqual(H.report(0x31, 19, 0, 0, false).data[0], 0x30, 'the sequence wraps at 16');
    assert.strictEqual(H.report(0x31, 0, 0, 0, true).data[2 + 38], 4, 'v2 shifted too');
  },
  'gamepad ids: the DualSense and the Edge, nothing else'() {
    assert.strictEqual(H.product('DualSense Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 0ce6)'), 0x0ce6);
    assert.strictEqual(H.product('Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 0df2)'), 0x0df2);
    assert.strictEqual(H.product('Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 09cc)'), 0, 'DualShock 4: Chrome drives it');
    assert.strictEqual(H.product('Xbox 360 Controller (XInput STANDARD GAMEPAD)'), 0);
    assert.deepStrictEqual(H.FILTERS, [{ vendorId: 0x054c, productId: 0x0ce6 }, { vendorId: 0x054c, productId: 0x0df2 }]);
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
console.log(`${Object.keys(tests).length - failed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
