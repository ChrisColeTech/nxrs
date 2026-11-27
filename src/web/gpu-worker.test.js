// gpu-worker.js's pipeline creation (WEB-W8e), against a fake WebGPU device (not shipped; run with node:
// build\tools\emsdk\node\<version>\node.exe src\web\gpu-worker.test.js). The worker script runs in a vm context whose
// later scripts see its top-level bindings (objs, run, settlePipelines, ...).
'use strict';
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

function worker() {
  const posted = [];
  const ctx = vm.createContext({
    posted, console, TextDecoder, URL, performance, setTimeout,
    setInterval: () => 0,
    postMessage: (m) => posted.push(m),
    MessageChannel: class { constructor() { this.port1 = {}; this.port2 = { postMessage() {} }; } },
    self: { location: { href: 'http://127.0.0.1/' } },
    navigator: {},
  });
  vm.runInContext(fs.readFileSync(path.join(__dirname, 'gpu-worker.js'), 'utf8'), ctx);
  // a fake device whose async pipelines resolve when the test says so; the sync calls are counted
  vm.runInContext(`
    var pending = [], syncMade = 0, scopes = [];
    device = {
      createRenderPipelineAsync: (d) => new Promise((ok, no) => pending.push({ d, ok, no })),
      createComputePipelineAsync: (d) => new Promise((ok, no) => pending.push({ d, ok, no })),
      createRenderPipeline: (d) => { syncMade++; return { invalid: true, d }; },
      createComputePipeline: (d) => { syncMade++; return { invalid: true, d }; },
      pushErrorScope: (f) => scopes.push(f),
      popErrorScope: () => { scopes.pop(); return Promise.resolve({ message: 'bad blend' }); },
      features: new Set(),
    };
    D8 = new Uint8Array(4096);
    DV = new DataView(D8.buffer);
    var op = (o, j) => { const b = new TextEncoder().encode(JSON.stringify(j)); D8.set(b, 0); return run(o, 0, b.length); };
  `, Object.assign(ctx, { TextEncoder }));
  return ctx;
}
const js = (ctx, code) => vm.runInContext(code, ctx);
const tick = () => new Promise((r) => setTimeout(r, 0));

const tests = {
  async 'a render pipeline is compiled asynchronously and is there once settled'() {
    const w = worker();
    js(w, `op(OP.RPIPE, { id: 7, layout: 1, vertex: { module: 2, entryPoint: 'vs', buffers: [] }, primitive: {}, multisample: {} })`);
    assert.strictEqual(js(w, 'pending.length'), 1, 'createRenderPipelineAsync called');
    assert.strictEqual(js(w, 'syncMade'), 0, 'no synchronous pipeline');
    assert.strictEqual(js(w, 'pipesPending.length'), 1, 'the next submission waits for it');
    let settled = false;
    js(w, 'settlePipelines()').then(() => { settled = true; });
    await tick();
    assert.strictEqual(settled, false, 'the submission does not start before the compile');
    js(w, `pending[0].ok({ pipeline: 7 })`);
    await tick(); await tick();
    assert.strictEqual(settled, true);
    assert.strictEqual(js(w, 'objs.get(7).pipeline'), 7);
    assert.strictEqual(js(w, 'pipesPending.length'), 0);
  },
  async 'a compute pipeline too'() {
    const w = worker();
    js(w, `op(OP.CPIPE, { id: 9, layout: 1, module: 3, entry: 'main' })`);
    assert.strictEqual(js(w, 'pending[0].d.compute.entryPoint'), 'main');
    js(w, `pending[0].ok({ compute: 9 })`);
    await js(w, 'settlePipelines()');
    assert.strictEqual(js(w, 'objs.get(9).compute'), 9);
  },
  async 'an invalid pipeline falls back to the synchronous call: the same invalid object and its message'() {
    const w = worker();
    js(w, `op(OP.RPIPE, { id: 4, layout: 1, vertex: { module: 2, entryPoint: 'vs', buffers: [] }, primitive: {}, multisample: {} })`);
    js(w, `pending[0].no(new Error('GPUPipelineError'))`);
    await js(w, 'settlePipelines()');
    await tick();
    assert.strictEqual(js(w, 'syncMade'), 1);
    assert.strictEqual(js(w, 'objs.get(4).invalid'), true);
    assert.strictEqual(js(w, 'scopes.length'), 0, 'the error scope is popped');
    assert.ok(js(w, 'posted').some((m) => /render pipeline 4: bad blend/.test(m.log)), 'the validation message is logged');
  },
  async 'settling with nothing pending returns at once'() {
    const w = worker();
    await js(w, 'settlePipelines()');
    assert.strictEqual(js(w, 'posted.length'), 0);
  },
};

(async () => {
  let failed = 0;
  for (const [name, f] of Object.entries(tests)) {
    try {
      await f();
      console.log(`ok   ${name}`);
    } catch (e) {
      failed++;
      console.log(`FAIL ${name}\n     ${e.stack}`);
    }
  }
  console.log(`${Object.keys(tests).length - failed} passed, ${failed} failed`);
  process.exit(failed ? 1 : 0);
})();
