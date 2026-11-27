// The web build's GPU worker (docs/35 W4): owns the WebGPU device and the canvas, executes the engine's command ring.
//
// The engine (src/graphics/src/web) resolves its Vulkan replay into WebGPU-shaped packets; this file only carries them
// out. Ring layout and opcodes: src/graphics/src/web/ring.rs and obj.rs (`op`). Uploads are written straight from the
// wasm memory (a SharedArrayBuffer); completion values are written back into it and the engine woken (Atomics).
'use strict';

const H_MAGIC = 0, H_HEAD = 1, H_TAIL = 2, H_STATE = 3, H_CANVAS_W = 4, H_CANVAS_H = 5, H_ERRORS = 6,
  H_PRESENTS = 7, H_GEN = 8, H_DATA_SIZE = 9, H_FEATURES = 10, H_MAX_STORAGE_PER_STAGE = 11,
  H_MAX_UNIFORM_PER_STAGE = 12, H_MAX_TEXTURES_PER_STAGE = 13, H_MAX_BIND_GROUPS = 14, H_MAX_STORAGE_MB = 15,
  H_MAX_TEX_2D = 16, H_DONE = 17, H_EXEC_US = 18, H_FRAME_US = 19, H_SLEEPING = 20;
const F_BC = 1, F_FLOAT32_FILTER = 2, F_D32S8 = 4, F_DUAL_SRC = 8, F_SWIZZLE = 16, F_RG11B10_RENDER = 32, F_TIER1 = 64;
const OP = {
  BUFFER: 1, DESTROY: 2, TEXTURE: 3, VIEW: 4, SAMPLER: 5, SHADER: 6, BGL: 7, PLAYOUT: 8, RPIPE: 9, CPIPE: 10,
  BINDGROUP: 11, SUBMIT_BEGIN: 20, UPLOAD: 21, UPLOAD_TEX: 22, PASS_BEGIN: 23, PASS_END: 24, SET_PIPELINE: 25,
  SET_BIND_GROUP: 26, SET_VB: 27, SET_IB: 28, VIEWPORT: 29, SCISSOR: 30, BLEND_CONST: 31, STENCIL_REF: 32,
  DRAW: 33, DRAW_INDEXED: 34, DRAW_INDIRECT: 35, COMPUTE_BEGIN: 36, COMPUTE_END: 37, DISPATCH: 38,
  DISPATCH_INDIRECT: 39, COPY_B2B: 40, COPY_B2T: 41, COPY_T2B: 42, COPY_T2T: 43, CLEAR_BUFFER: 44, BLIT: 45,
  CLEAR_TEX: 46, READBACK: 47, SIGNAL: 48, SUBMIT_END: 49, PRESENT: 50, CLEAR_RECT: 51, FILL_BUFFER: 52,
};

let memory, ringAddr, H, DATA, D8, DV, dataBase;
let device, ctx, canvasFormat = 'bgra8unorm';
const objs = new Map();
const info = new Map(); // id -> descriptor facts (textures: {format, dim, w, h, d, mips, samples}; views: {...})
let chain = Promise.resolve();
let lastPresent = 0;
const logged = new Map();

const say = (text, red) => postMessage({ log: text, red });
const once = (key, text) => {
  const n = (logged.get(key) || 0) + 1;
  logged.set(key, n);
  if (n <= 3) say(text + (n === 3 ? ' (further ones not logged)' : ''), true);
};

function heap() {
  return memory.buffer;
}
function bytes(addr, n) {
  let b = memory.buffer;
  return new Uint8Array(b, addr, n);
}

onmessage = async (e) => {
  const m = e.data;
  // development: the ring header's words (NXRS_GPU.ask() on the page)
  if (m.ask) {
    postMessage({ stats: H ? Array.from(H.slice(0, 24)) : [] });
    return;
  }
  memory = m.memory;
  ringAddr = m.ring;
  shotEvery = m.shotEvery | 0;
  H = new Int32Array(memory.buffer, ringAddr, 64);
  DATA = H[H_DATA_SIZE];
  dataBase = ringAddr + 256;
  D8 = new Uint8Array(memory.buffer, dataBase, DATA);
  DV = new DataView(memory.buffer, dataBase, DATA);
  try {
    if (!navigator.gpu) throw new Error('navigator.gpu is missing');
    const adapter = await navigator.gpu.requestAdapter({ powerPreference: 'high-performance' });
    if (!adapter) throw new Error('requestAdapter gave no adapter');
    const want = ['texture-compression-bc', 'float32-filterable', 'depth32float-stencil8', 'depth-clip-control',
      'dual-source-blending', 'indirect-first-instance', 'texture-component-swizzle', 'rg11b10ufloat-renderable',
      'texture-formats-tier1'];
    const features = want.filter((f) => adapter.features.has(f));
    const L = adapter.limits;
    const lim = {};
    for (const k of ['maxBufferSize', 'maxStorageBufferBindingSize', 'maxStorageBuffersPerShaderStage',
      'maxUniformBuffersPerShaderStage', 'maxSampledTexturesPerShaderStage', 'maxSamplersPerShaderStage',
      'maxBindingsPerBindGroup', 'maxTextureDimension2D', 'maxTextureDimension1D', 'maxTextureArrayLayers',
      'maxColorAttachmentBytesPerSample', 'maxComputeWorkgroupStorageSize', 'maxInterStageShaderVariables',
      'maxVertexAttributes', 'maxDynamicUniformBuffersPerPipelineLayout', 'maxStorageTexturesPerShaderStage',
      'maxComputeInvocationsPerWorkgroup', 'maxComputeWorkgroupSizeX', 'maxComputeWorkgroupSizeY',
      'maxComputeWorkgroupSizeZ', 'maxBindGroups', 'maxUniformBufferBindingSize']) {
      if (L[k] !== undefined) lim[k] = L[k];
    }
    device = await adapter.requestDevice({ requiredFeatures: features, requiredLimits: lim });
    device.lost.then((i) => {
      say(`device lost: ${i.reason} ${i.message}`, true);
      postMessage({ fatal: `the GPU device was lost (${i.reason || 'unknown'}): reload the page to start again` });
      Atomics.store(H, H_STATE, 2);
      Atomics.notify(H, H_STATE);
      Atomics.notify(H, H_GEN);
    });
    device.onuncapturederror = (ev) => {
      Atomics.add(H, H_ERRORS, 1);
      const msg = ev.error.message || String(ev.error);
      once(msg.slice(0, 80), 'WebGPU: ' + msg.slice(0, 600));
    };
    ctx = m.canvas.getContext('webgpu');
    ctx.configure({ device, format: canvasFormat, alphaMode: 'opaque',
      usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.COPY_DST });
    let f = 0;
    if (device.features.has('texture-compression-bc')) f |= F_BC;
    if (device.features.has('float32-filterable')) f |= F_FLOAT32_FILTER;
    if (device.features.has('depth32float-stencil8')) f |= F_D32S8;
    if (device.features.has('dual-source-blending')) f |= F_DUAL_SRC;
    if (device.features.has('texture-component-swizzle')) f |= F_SWIZZLE;
    if (device.features.has('rg11b10ufloat-renderable')) f |= F_RG11B10_RENDER;
    if (device.features.has('texture-formats-tier1')) f |= F_TIER1;
    H[H_FEATURES] = f;
    H[H_MAX_STORAGE_PER_STAGE] = device.limits.maxStorageBuffersPerShaderStage;
    H[H_MAX_UNIFORM_PER_STAGE] = device.limits.maxUniformBuffersPerShaderStage;
    H[H_MAX_TEXTURES_PER_STAGE] = device.limits.maxSampledTexturesPerShaderStage;
    H[H_MAX_BIND_GROUPS] = device.limits.maxBindGroups;
    H[H_MAX_STORAGE_MB] = Math.floor(Math.min(device.limits.maxStorageBufferBindingSize, device.limits.maxBufferSize) / 1048576);
    H[H_MAX_TEX_2D] = device.limits.maxTextureDimension2D;
    H[H_CANVAS_W] = m.width;
    H[H_CANVAS_H] = m.height;
    const ai = adapter.info || {};
    say(`WebGPU device: ${ai.vendor || '?'} ${ai.architecture || ''} ${ai.description || ''}; features ${features.join(', ')}`);
    Atomics.store(H, H_STATE, 1);
    Atomics.notify(H, H_STATE);
  } catch (err) {
    say('no WebGPU device: ' + err, true);
    postMessage({ fatal: 'no WebGPU device: ' + err });
    Atomics.store(H, H_STATE, 2);
    Atomics.notify(H, H_STATE);
    return;
  }
  loop();
};

// ---------- the ring ----------

const yieldTask = () => new Promise((r) => {
  const c = new MessageChannel();
  c.port1.onmessage = () => r();
  c.port2.postMessage(0);
});

async function loop() {
  let t0 = performance.now();
  for (;;) {
    let tail = Atomics.load(H, H_TAIL);
    let head = Atomics.load(H, H_HEAD);
    if (head === tail) {
      Atomics.store(H, H_SLEEPING, 1);
      head = Atomics.load(H, H_HEAD);
      if (head === tail) {
        const w = Atomics.waitAsync(H, H_HEAD, head, 1000);
        if (w.async) await w.value;
      }
      Atomics.store(H, H_SLEEPING, 0);
      t0 = performance.now();
      continue;
    }
    while (tail !== head) {
      if (DATA - tail < 8) { tail = 0; continue; }
      const op = DV.getUint32(tail, true);
      const len = DV.getUint32(tail + 4, true);
      if (op === 0) { tail = 0; continue; }
      let presented = false;
      try {
        presented = run(op, tail + 8, len);
      } catch (err) {
        once('op' + op + String(err).slice(0, 40), `op ${op}: ${err && err.stack || err}`);
      }
      tail = (tail + 8 + ((len + 3) & ~3)) % DATA;
      Atomics.store(H, H_TAIL, tail);
      Atomics.notify(H, H_TAIL);
      if (presented || performance.now() - t0 > 4) {
        await yieldTask();
        t0 = performance.now();
        head = Atomics.load(H, H_HEAD);
      }
    }
  }
}

const u32 = (p) => DV.getUint32(p, true);
const i32 = (p) => DV.getInt32(p, true);
const f32 = (p) => DV.getFloat32(p, true);
const u64 = (p) => DV.getUint32(p, true) + DV.getUint32(p + 4, true) * 4294967296;
const text = (p, n) => new TextDecoder().decode(D8.slice(p, p + n));

function run(op, p, len) {
  switch (op) {
    case OP.BUFFER: {
      const id = u32(p), usage = u32(p + 4), size = u64(p + 8);
      objs.set(id, device.createBuffer({ size: Math.max(16, size), usage }));
      info.set(id, { size: Math.max(16, size) });
      return false;
    }
    case OP.DESTROY: {
      const id = u32(p);
      const o = objs.get(id);
      if (o && o.destroy) o.destroy();
      objs.delete(id);
      info.delete(id);
      attViews.delete(id);
      return false;
    }
    case OP.TEXTURE: return createTexture(JSON.parse(text(p, len)));
    case OP.VIEW: return createView(JSON.parse(text(p, len)));
    case OP.SAMPLER: {
      const j = JSON.parse(text(p, len));
      let d = j;
      if (j.twinOf !== undefined) {
        d = Object.assign({}, info.get(j.twinOf), { magFilter: 'nearest', minFilter: 'nearest', mipmapFilter: 'nearest', maxAnisotropy: 1 });
      }
      const id = d.id === undefined ? j.id : j.id;
      const desc = Object.assign({}, d);
      delete desc.id;
      delete desc.twinOf;
      objs.set(id, device.createSampler(desc));
      info.set(id, desc);
      return false;
    }
    case OP.SHADER: {
      const j = JSON.parse(text(p, len));
      const m = device.createShaderModule({ code: j.code });
      objs.set(j.id, m);
      m.getCompilationInfo().then((ci) => {
        for (const msg of ci.messages) {
          if (msg.type === 'error') once('shader' + j.id, `shader ${j.id}: ${msg.lineNum}:${msg.linePos} ${msg.message}`);
        }
      });
      return false;
    }
    case OP.BGL: {
      const j = JSON.parse(text(p, len));
      objs.set(j.id, device.createBindGroupLayout({ entries: j.entries }));
      return false;
    }
    case OP.PLAYOUT: {
      const j = JSON.parse(text(p, len));
      objs.set(j.id, device.createPipelineLayout({ bindGroupLayouts: j.bgls.map((b) => objs.get(b)) }));
      return false;
    }
    case OP.RPIPE: {
      const j = JSON.parse(text(p, len));
      const d = {
        layout: objs.get(j.layout),
        vertex: { module: objs.get(j.vertex.module), entryPoint: j.vertex.entryPoint, buffers: j.vertex.buffers },
        primitive: j.primitive,
        multisample: j.multisample,
      };
      if (j.fragment) d.fragment = { module: objs.get(j.fragment.module), entryPoint: j.fragment.entryPoint, targets: j.fragment.targets };
      if (j.depthStencil) d.depthStencil = j.depthStencil;
      if (d.primitive.unclippedDepth && !device.features.has('depth-clip-control')) delete d.primitive.unclippedDepth;
      device.pushErrorScope('validation');
      objs.set(j.id, device.createRenderPipeline(d));
      device.popErrorScope().then((er) => {
        if (er) once('rpipe' + er.message.slice(0, 60), `render pipeline ${j.id}: ${er.message.slice(0, 900)}`);
      });
      return false;
    }
    case OP.CPIPE: {
      const j = JSON.parse(text(p, len));
      device.pushErrorScope('validation');
      objs.set(j.id, device.createComputePipeline({ layout: objs.get(j.layout), compute: { module: objs.get(j.module), entryPoint: j.entry } }));
      device.popErrorScope().then((er) => {
        if (er) once('cpipe' + er.message.slice(0, 60), `compute pipeline ${j.id}: ${er.message.slice(0, 900)}`);
      });
      return false;
    }
    case OP.BINDGROUP: {
      const j = JSON.parse(text(p, len));
      const entries = j.entries.map((x) => {
        if (x.buffer !== undefined) return { binding: x.binding, resource: { buffer: objs.get(x.buffer), offset: x.offset, size: x.size } };
        if (x.view !== undefined) return { binding: x.binding, resource: objs.get(x.view) };
        return { binding: x.binding, resource: objs.get(x.sampler) };
      });
      device.pushErrorScope('validation');
      objs.set(j.id, device.createBindGroup({ layout: objs.get(j.layout), entries }));
      device.popErrorScope().then((er) => {
        if (er) once('bg' + er.message.slice(0, 60), `bind group ${j.id}: ${er.message.slice(0, 700)}`);
      });
      return false;
    }
    case OP.SUBMIT_BEGIN: return submit(p, len);
    case OP.PRESENT: return present(u32(p));
    default:
      once('unknown' + op, `unknown ring op ${op}`);
      return false;
  }
}

function createTexture(j) {
  const d = {
    size: [j.w, j.h, j.d], format: j.format, dimension: j.dim, mipLevelCount: j.mips, sampleCount: j.samples,
    usage: j.usage, viewFormats: j.viewFormats || [],
  };
  if (j.dim === '1d') d.size = [j.w, 1, 1];
  device.pushErrorScope('validation');
  objs.set(j.id, device.createTexture(d));
  device.popErrorScope().then((er) => {
    if (er) once('tex' + er.message.slice(0, 60), `texture ${j.id} ${j.format} ${j.w}x${j.h}x${j.d}: ${er.message.slice(0, 500)}`);
  });
  info.set(j.id, j);
  return false;
}

function createView(j) {
  const t = objs.get(j.tex);
  if (!t) return false;
  const d = { format: j.format, dimension: j.dim, baseMipLevel: j.baseMip, mipLevelCount: j.mips, aspect: j.aspect };
  // a one-aspect view of a depth-stencil texture takes that aspect's format (WebGPU resolves it when left out)
  if (j.aspect !== 'all') delete d.format;
  if (j.dim !== '3d') {
    d.baseArrayLayer = j.baseLayer;
    d.arrayLayerCount = j.layers;
  }
  if (j.swizzle && j.swizzle !== 'rgba') {
    if (device.features.has('texture-component-swizzle')) d.swizzle = j.swizzle;
    else once('swizzle', `view swizzle ${j.swizzle}: no texture-component-swizzle on this device (shown unswizzled)`);
  }
  device.pushErrorScope('validation');
  objs.set(j.id, t.createView(d));
  device.popErrorScope().then((er) => {
    if (er) once('view' + er.message.slice(0, 60), `view ${j.id} of texture ${j.tex} (${JSON.stringify(d)}): ${er.message.slice(0, 500)}`);
  });
  info.set(j.id, j);
  return false;
}

// a single-mip, single-layer 2D view of a view or texture, for render attachments
const attViews = new Map();
function attView(viewId) {
  const v = info.get(viewId);
  if (!v) return null;
  if ((v.dim === '2d') && v.mips === 1) return objs.get(viewId);
  let a = attViews.get(viewId);
  if (!a) {
    const t = objs.get(v.tex);
    const ti = info.get(v.tex);
    const d = { format: v.format, dimension: '2d', baseMipLevel: v.baseMip, mipLevelCount: 1, aspect: v.aspect };
    if (ti.dim !== '3d') {
      d.baseArrayLayer = v.baseLayer;
      d.arrayLayerCount = 1;
    }
    a = t.createView(d);
    attViews.set(viewId, a);
  }
  return a;
}
function subView(texId, mip, layer, aspect) {
  const t = objs.get(texId);
  const ti = info.get(texId);
  const d = { dimension: '2d', baseMipLevel: mip, mipLevelCount: 1, aspect: aspect || 'all' };
  if (ti.dim === '3d') d.dimension = '3d';
  else {
    d.baseArrayLayer = layer;
    d.arrayLayerCount = 1;
  }
  return t.createView(d);
}

// ---------- a submission ----------

const ASPECT = ['all', 'depth-only', 'stencil-only'];
const DEPTH_FORMATS = { depth16unorm: [1, 0], depth32float: [1, 0], depth24plus: [1, 0], 'depth24plus-stencil8': [1, 1], 'depth32float-stencil8': [1, 1], stencil8: [0, 1] };

function submit(p0, len) {
  const t0 = performance.now();
  const end = p0 + len;
  let enc = device.createCommandEncoder();
  let used = false;
  let pass = null, cpass = null, passInfo = null;
  const readbacks = [];
  const signals = [];
  const temps = [];
  const flush = () => {
    if (used) {
      device.queue.submit([enc.finish()]);
      enc = device.createCommandEncoder();
      used = false;
    }
  };
  let p = p0;
  while (p < end) {
    const op = u32(p), n = u32(p + 4), a = p + 8;
    p = a + ((n + 3) & ~3);
    try {
      switch (op) {
        case OP.UPLOAD: {
          const id = u32(a), off = u64(a + 4), src = u64(a + 12), size = u64(a + 20);
          const buf = objs.get(id);
          if (!buf || size === 0) break;
          flush();
          if (src === 0) device.queue.writeBuffer(buf, off, D8, a + 28, size);
          else device.queue.writeBuffer(buf, off, bytes(src, size));
          break;
        }
        case OP.UPLOAD_TEX: {
          const tex = u32(a), mip = u32(a + 4), x = u32(a + 8), y = u32(a + 12), z = u32(a + 16);
          const w = u32(a + 20), h = u32(a + 24), d = u32(a + 28), asp = u32(a + 32);
          const src = u64(a + 36), bpr = u32(a + 44), rpi = u32(a + 48);
          const t = objs.get(tex);
          if (!t) break;
          const ti = info.get(tex);
          const fi = blockOf(ti.format);
          const rows = Math.ceil(h / fi.bh);
          const size = bpr * rpi * (d - 1) + bpr * (rows - 1) + Math.ceil(w / fi.bw) * fi.bytes;
          flush();
          device.queue.writeTexture({ texture: t, mipLevel: mip, origin: { x, y, z }, aspect: ASPECT[asp] },
            bytes(src, size), { offset: 0, bytesPerRow: bpr, rowsPerImage: rpi }, blockExtent(fi, w, h, d));
          break;
        }
        case OP.PASS_BEGIN: {
          const nc = u32(a);
          let q = a + 4;
          const colors = [];
          passInfo = { formats: [], depth: null, samples: 1 };
          for (let i = 0; i < nc; i++) {
            const view = u32(q), resolve = u32(q + 4), clear = u32(q + 8), store = u32(q + 12), int = u32(q + 16);
            const cv = [q + 20, q + 24, q + 28, q + 32];
            q += 36;
            if (!view) { colors.push(null); passInfo.formats.push(null); continue; }
            const vi = info.get(view);
            const c = { view: attView(view), loadOp: clear ? 'clear' : 'load', storeOp: store ? 'store' : 'discard' };
            if (vi && info.get(vi.tex)?.dim === '3d') c.depthSlice = vi.baseLayer;
            if (clear) c.clearValue = int ? cv.map((x) => (int === 1 ? i32(x) : u32(x))) : cv.map(f32);
            if (resolve) c.resolveTarget = attView(resolve);
            colors.push(c);
            passInfo.formats.push(vi ? vi.format : null);
            passInfo.samples = info.get(vi.tex)?.samples || 1;
          }
          const dview = u32(q), dmode = u32(q + 4), smode = u32(q + 8), dclear = f32(q + 12), sclear = u32(q + 16);
          const desc = { colorAttachments: colors };
          if (dview) {
            const vi = info.get(dview);
            const ds = { view: attView(dview) };
            const [hasD, hasS] = DEPTH_FORMATS[vi.format] || [1, 0];
            if (hasD) {
              ds.depthLoadOp = dmode === 2 ? 'clear' : 'load';
              ds.depthStoreOp = 'store';
              ds.depthClearValue = dclear;
            }
            if (hasS) {
              ds.stencilLoadOp = smode === 2 ? 'clear' : 'load';
              ds.stencilStoreOp = 'store';
              ds.stencilClearValue = sclear;
            }
            desc.depthStencilAttachment = ds;
            passInfo.depth = vi.format;
            passInfo.samples = info.get(vi.tex)?.samples || passInfo.samples;
          }
          pass = enc.beginRenderPass(desc);
          used = true;
          break;
        }
        case OP.PASS_END:
          if (pass) pass.end();
          pass = null;
          break;
        case OP.COMPUTE_BEGIN:
          cpass = enc.beginComputePass();
          used = true;
          break;
        case OP.COMPUTE_END:
          if (cpass) cpass.end();
          cpass = null;
          break;
        case OP.SET_PIPELINE:
          (pass || cpass).setPipeline(objs.get(u32(a)));
          break;
        case OP.SET_BIND_GROUP: {
          const idx = u32(a), bg = objs.get(u32(a + 4)), nd = u32(a + 8);
          const offs = [];
          for (let i = 0; i < nd; i++) offs.push(u32(a + 12 + 4 * i));
          (pass || cpass).setBindGroup(idx, bg, offs);
          break;
        }
        case OP.SET_VB:
          pass.setVertexBuffer(u32(a), objs.get(u32(a + 4)), u64(a + 8), u64(a + 16));
          break;
        case OP.SET_IB:
          pass.setIndexBuffer(objs.get(u32(a)), u32(a + 4) ? 'uint32' : 'uint16', u64(a + 8), u64(a + 16));
          break;
        case OP.VIEWPORT:
          pass.setViewport(f32(a), f32(a + 4), f32(a + 8), f32(a + 12), f32(a + 16), f32(a + 20));
          break;
        case OP.SCISSOR:
          pass.setScissorRect(u32(a), u32(a + 4), u32(a + 8), u32(a + 12));
          break;
        case OP.BLEND_CONST:
          pass.setBlendConstant([f32(a), f32(a + 4), f32(a + 8), f32(a + 12)]);
          break;
        case OP.STENCIL_REF:
          pass.setStencilReference(u32(a));
          break;
        case OP.DRAW:
          pass.draw(u32(a), u32(a + 4), u32(a + 8), u32(a + 12));
          break;
        case OP.DRAW_INDEXED:
          pass.drawIndexed(u32(a), u32(a + 4), u32(a + 8), i32(a + 12), u32(a + 16));
          break;
        case OP.DRAW_INDIRECT:
          if (u32(a + 12)) pass.drawIndexedIndirect(objs.get(u32(a)), u64(a + 4));
          else pass.drawIndirect(objs.get(u32(a)), u64(a + 4));
          break;
        case OP.DISPATCH:
          cpass.dispatchWorkgroups(u32(a), u32(a + 4), u32(a + 8));
          break;
        case OP.DISPATCH_INDIRECT:
          cpass.dispatchWorkgroupsIndirect(objs.get(u32(a)), u64(a + 4));
          break;
        case OP.COPY_B2B: {
          const s = objs.get(u32(a)), so = u64(a + 4), d = objs.get(u32(a + 12)), dof = u64(a + 16), size = u64(a + 24);
          if (s && d && size) {
            if (s === d) {
              // WebGPU copies between two buffers only: within one, through a temporary
              const tmp = device.createBuffer({ size, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.COPY_SRC });
              temps.push(tmp);
              enc.copyBufferToBuffer(s, so, tmp, 0, size);
              enc.copyBufferToBuffer(tmp, 0, d, dof, size);
            } else enc.copyBufferToBuffer(s, so, d, dof, size);
            used = true;
          }
          break;
        }
        case OP.COPY_B2T: {
          const buf = objs.get(u32(a)), off = u64(a + 4), bpr = u32(a + 12), rpi = u32(a + 16);
          const tex = u32(a + 20), mip = u32(a + 24), x = u32(a + 28), y = u32(a + 32), z = u32(a + 36);
          const w = u32(a + 40), h = u32(a + 44), d = u32(a + 48), asp = u32(a + 52);
          const t = objs.get(tex);
          if (!buf || !t) break;
          const fi = blockOf(info.get(tex).format);
          const rows = Math.ceil(h / fi.bh);
          let src = buf, so = off, sbpr = bpr;
          if (bpr % 256 !== 0 || off % fi.bytes !== 0) {
            // WebGPU copies rows at 256-byte pitches: rows go through a temporary buffer at that pitch
            sbpr = Math.ceil(bpr / 256) * 256;
            const rowBytes = Math.ceil(Math.ceil(w / fi.bw) * fi.bytes / 4) * 4;
            src = device.createBuffer({ size: sbpr * rows * d, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.COPY_SRC });
            temps.push(src);
            so = 0;
            for (let l = 0; l < d; l++) {
              for (let r = 0; r < rows; r++) {
                enc.copyBufferToBuffer(buf, off + (l * rpi + r) * bpr, src, (l * rows + r) * sbpr, Math.min(rowBytes, sbpr));
              }
            }
          }
          enc.copyBufferToTexture({ buffer: src, offset: so, bytesPerRow: sbpr, rowsPerImage: src === buf ? rpi : rows },
            { texture: t, mipLevel: mip, origin: { x, y, z }, aspect: ASPECT[asp] }, blockExtent(fi, w, h, d));
          used = true;
          break;
        }
        case OP.COPY_T2B: {
          const tex = u32(a), mip = u32(a + 4), x = u32(a + 8), y = u32(a + 12), z = u32(a + 16);
          const w = u32(a + 20), h = u32(a + 24), d = u32(a + 28), asp = u32(a + 32);
          const buf = objs.get(u32(a + 36)), off = u64(a + 40), bpr = u32(a + 48), rpi = u32(a + 52);
          const t = objs.get(tex);
          if (!buf || !t) break;
          const fi = blockOf(info.get(tex).format);
          const rows = Math.ceil(h / fi.bh);
          const src = { texture: t, mipLevel: mip, origin: { x, y, z }, aspect: ASPECT[asp] };
          const ext = blockExtent(fi, w, h, d);
          if (bpr % 256 === 0 && off % 4 === 0) {
            enc.copyTextureToBuffer(src, { buffer: buf, offset: off, bytesPerRow: bpr, rowsPerImage: rpi }, ext);
          } else {
            const pbpr = Math.ceil(bpr / 256) * 256;
            const tmp = device.createBuffer({ size: pbpr * rows * d, usage: GPUBufferUsage.COPY_DST | GPUBufferUsage.COPY_SRC });
            temps.push(tmp);
            enc.copyTextureToBuffer(src, { buffer: tmp, offset: 0, bytesPerRow: pbpr, rowsPerImage: rows }, ext);
            const rowBytes = Math.ceil(w / fi.bw) * fi.bytes;
            if (rowBytes % 4 !== 0 || bpr % 4 !== 0 || off % 4 !== 0) {
              once('t2b-align', `a texture read-back with rows of ${rowBytes} bytes at pitch ${bpr} (not 4-aligned): not copied`);
              break;
            }
            for (let l = 0; l < d; l++) {
              for (let r = 0; r < rows; r++) {
                enc.copyBufferToBuffer(tmp, (l * rows + r) * pbpr, buf, off + (l * rpi + r) * bpr, rowBytes);
              }
            }
          }
          used = true;
          break;
        }
        case OP.COPY_T2T: {
          const s = objs.get(u32(a)), d = objs.get(u32(a + 20));
          if (!s || !d) break;
          enc.copyTextureToTexture(
            { texture: s, mipLevel: u32(a + 4), origin: { x: u32(a + 8), y: u32(a + 12), z: u32(a + 16) }, aspect: ASPECT[u32(a + 52)] },
            { texture: d, mipLevel: u32(a + 24), origin: { x: u32(a + 28), y: u32(a + 32), z: u32(a + 36) }, aspect: ASPECT[u32(a + 52)] },
            blockExtent(blockOf(info.get(u32(a)).format), u32(a + 40), u32(a + 44), u32(a + 48)));
          used = true;
          break;
        }
        case OP.CLEAR_BUFFER: {
          const b = objs.get(u32(a)), off = u64(a + 4), size = u64(a + 12);
          if (b && size) {
            enc.clearBuffer(b, off, size);
            used = true;
          }
          break;
        }
        case OP.FILL_BUFFER: {
          const b = objs.get(u32(a)), off = u64(a + 4), size = u64(a + 12), v = u32(a + 20);
          if (!b || !size) break;
          flush();
          const arr = new Uint32Array(size / 4);
          arr.fill(v);
          device.queue.writeBuffer(b, off, arr);
          break;
        }
        case OP.BLIT:
          blit(enc, a);
          used = true;
          break;
        case OP.CLEAR_TEX:
          clearTex(enc, a);
          used = true;
          break;
        case OP.CLEAR_RECT:
          if (pass && passInfo) clearRect(pass, passInfo, a);
          break;
        case OP.READBACK:
          readbacks.push([u32(a), u64(a + 4), u64(a + 12), u64(a + 20)]);
          break;
        case OP.SIGNAL:
          // [done word, value, queued word (0: none)]
          signals.push([u64(a), u64(a + 8), n >= 24 ? u64(a + 16) : 0]);
          break;
        default:
          once('sub' + op, `unknown submission op ${op}`);
      }
    } catch (err) {
      once('subop' + op + String(err).slice(0, 30), `submission op ${op}: ${err && err.stack || err}`);
      if (op === OP.PASS_BEGIN) pass = null;
    }
  }
  if (pass) pass.end();
  if (cpass) cpass.end();
  // read-backs: copies into mappable buffers in the same submission, mapped once the GPU is done
  const maps = [];
  for (const [id, off, n, dst] of readbacks) {
    const b = objs.get(id);
    if (!b) continue;
    const a0 = off - (off % 4);
    const a1 = Math.min(Math.ceil((off + n) / 4) * 4, b.size);
    if (a1 <= a0) continue;
    const st = device.createBuffer({ size: a1 - a0, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST });
    enc.copyBufferToBuffer(b, a0, st, 0, a1 - a0);
    used = true;
    maps.push([st, off - a0, n, dst]);
  }
  device.queue.submit([enc.finish()]);
  for (const t of temps) t.destroy();
  const exec = performance.now() - t0;
  H[H_EXEC_US] = Math.round(exec * 1000);
  // queued: the engine's GPU-time budget for this submission starts now (its CPU-side execute is done)
  let queued = false;
  for (const [, v, q] of signals) {
    if (q) {
      Atomics.store(new BigInt64Array(heap(), q, 1), 0, BigInt(v));
      queued = true;
    }
  }
  if (queued) {
    Atomics.add(H, H_GEN, 1);
    Atomics.notify(H, H_GEN);
  }
  const done = device.queue.onSubmittedWorkDone();
  const mapped = maps.map(([st]) => st.mapAsync(GPUMapMode.READ));
  chain = chain.then(async () => {
    await done;
    for (let i = 0; i < maps.length; i++) {
      const [st, skip, n, dst] = maps[i];
      try {
        await mapped[i];
        const src = new Uint8Array(st.getMappedRange(), skip, n);
        bytes(dst, n).set(src);
      } catch (err) {
        once('rb', 'read-back: ' + err);
      }
      st.destroy();
    }
    for (const [addr, v] of signals) {
      Atomics.store(new BigInt64Array(heap(), addr, 1), 0, BigInt(v));
    }
    Atomics.add(H, H_DONE, 1);
    Atomics.add(H, H_GEN, 1);
    Atomics.notify(H, H_GEN);
  });
  return false;
}

// ---------- formats ----------

function blockOf(f) {
  if (f.startsWith('bc')) return { bw: 4, bh: 4, bytes: /^bc[14]-/.test(f) ? 8 : 16 };
  const b = {
    r8unorm: 1, r8snorm: 1, r8uint: 1, r8sint: 1, stencil8: 1, r16uint: 2, r16sint: 2, r16float: 2, rg8unorm: 2, rg8snorm: 2,
    rg8uint: 2, rg8sint: 2, depth16unorm: 2, r16unorm: 2, rg16unorm: 4, rgba16unorm: 8, r32uint: 4, r32sint: 4, r32float: 4, rg16uint: 4, rg16sint: 4, rg16float: 4,
    rgba8unorm: 4, 'rgba8unorm-srgb': 4, rgba8snorm: 4, rgba8uint: 4, rgba8sint: 4, bgra8unorm: 4, 'bgra8unorm-srgb': 4,
    rgb10a2unorm: 4, rgb10a2uint: 4, rg11b10ufloat: 4, rgb9e5ufloat: 4, depth32float: 4, depth24plus: 4,
    'depth32float-stencil8': 4, rg32uint: 8, rg32sint: 8, rg32float: 8, rgba16uint: 8, rgba16sint: 8, rgba16float: 8,
    rgba32uint: 16, rgba32sint: 16, rgba32float: 16,
  }[f] || 4;
  return { bw: 1, bh: 1, bytes: b };
}
const isInt = (f) => /[us]int$/.test(f);
// a copy of a block-compressed mip smaller than a block covers the whole block (WebGPU copies whole blocks)
const blockExtent = (fi, w, h, d) => ({ width: Math.ceil(w / fi.bw) * fi.bw, height: Math.ceil(h / fi.bh) * fi.bh, depthOrArrayLayers: d });
const UNFILTERABLE = new Set(['r32float', 'rg32float', 'rgba32float', 'depth32float', 'depth16unorm', 'depth24plus', 'depth32float-stencil8']);

// ---------- blit, clears, present ----------

const BLIT_WGSL = `
struct U { s0: vec2f, s1: vec2f, lod: f32, layer: f32 }
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var t: texture_2d<f32>;
@group(0) @binding(2) var s: sampler;
struct V { @builtin(position) p: vec4f, @location(0) uv: vec2f }
@vertex fn vs(@builtin(vertex_index) i: u32) -> V {
  let c = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
  var o: V;
  o.p = vec4f(c.x * 2.0 - 1.0, 1.0 - c.y * 2.0, 0.0, 1.0);
  o.uv = mix(u.s0, u.s1, c);
  return o;
}
@fragment fn fs(v: V) -> @location(0) vec4f { return textureSampleLevel(t, s, v.uv, u.lod); }
`;
const blitPipes = new Map();
let blitModule = null;
const blitSamplers = {};
function blitPipe(format, filterable) {
  const key = format + filterable;
  let bp = blitPipes.get(key);
  if (bp) return bp;
  if (!blitModule) blitModule = device.createShaderModule({ code: BLIT_WGSL });
  const bgl = device.createBindGroupLayout({ entries: [
    { binding: 0, visibility: 3, buffer: { type: 'uniform' } },
    { binding: 1, visibility: 2, texture: { sampleType: filterable ? 'float' : 'unfilterable-float' } },
    { binding: 2, visibility: 2, sampler: { type: filterable ? 'filtering' : 'non-filtering' } },
  ] });
  const pipe = device.createRenderPipeline({
    layout: device.createPipelineLayout({ bindGroupLayouts: [bgl] }),
    vertex: { module: blitModule, entryPoint: 'vs' },
    fragment: { module: blitModule, entryPoint: 'fs', targets: [{ format }] },
    primitive: { topology: 'triangle-list' },
  });
  bp = { pipe, bgl };
  blitPipes.set(key, bp);
  return bp;
}
function sampler(linear) {
  const k = linear ? 'l' : 'n';
  if (!blitSamplers[k]) blitSamplers[k] = device.createSampler({ magFilter: linear ? 'linear' : 'nearest', minFilter: linear ? 'linear' : 'nearest' });
  return blitSamplers[k];
}
function blit(enc, a) {
  const st = u32(a), smip = u32(a + 4), sl = u32(a + 8);
  const sx0 = i32(a + 12), sy0 = i32(a + 16), sx1 = i32(a + 20), sy1 = i32(a + 24);
  const dt = u32(a + 28), dmip = u32(a + 32), dl = u32(a + 36);
  let dx0 = i32(a + 40), dy0 = i32(a + 44), dx1 = i32(a + 48), dy1 = i32(a + 52);
  const linear = u32(a + 56);
  const si = info.get(st), di = info.get(dt);
  if (!si || !di) return;
  if (isInt(si.format) || isInt(di.format) || DEPTH_FORMATS[di.format]) {
    once('blitfmt', `blit ${si.format} -> ${di.format}: not on the web device`);
    return;
  }
  const sw = Math.max(1, si.w >> smip), sh = Math.max(1, si.h >> smip);
  // the destination rectangle in ascending order, the source corners swapped to match
  let u0 = sx0 / sw, u1 = sx1 / sw, v0 = sy0 / sh, v1 = sy1 / sh;
  if (dx0 > dx1) { [dx0, dx1] = [dx1, dx0]; [u0, u1] = [u1, u0]; }
  if (dy0 > dy1) { [dy0, dy1] = [dy1, dy0]; [v0, v1] = [v1, v0]; }
  if (dx1 === dx0 || dy1 === dy0) return;
  const filterable = !UNFILTERABLE.has(si.format) || device.features.has('float32-filterable') && /32float$/.test(si.format);
  const bp = blitPipe(di.format, filterable);
  const ub = device.createBuffer({ size: 32, usage: GPUBufferUsage.UNIFORM, mappedAtCreation: true });
  new Float32Array(ub.getMappedRange()).set([u0, v0, u1, v1, smip, sl, 0, 0]);
  ub.unmap();
  const srcView = objs.get(st).createView({ dimension: '2d', baseMipLevel: smip, mipLevelCount: 1, baseArrayLayer: si.dim === '3d' ? 0 : sl, arrayLayerCount: 1 });
  const bg = device.createBindGroup({ layout: bp.bgl, entries: [
    { binding: 0, resource: { buffer: ub } }, { binding: 1, resource: srcView }, { binding: 2, resource: sampler(linear && filterable) },
  ] });
  const rp = enc.beginRenderPass({ colorAttachments: [{ view: subView(dt, dmip, dl), loadOp: 'load', storeOp: 'store' }] });
  const dw = Math.max(1, di.w >> dmip), dh = Math.max(1, di.h >> dmip);
  const x0 = Math.max(0, dx0), y0 = Math.max(0, dy0), x1 = Math.min(dw, dx1), y1 = Math.min(dh, dy1);
  rp.setPipeline(bp.pipe);
  rp.setBindGroup(0, bg);
  rp.setViewport(dx0, dy0, dx1 - dx0, dy1 - dy0, 0, 1);
  if (x1 > x0 && y1 > y0) {
    rp.setScissorRect(x0, y0, x1 - x0, y1 - y0);
    rp.draw(3);
  }
  rp.end();
}

function clearTex(enc, a) {
  const tex = u32(a), mip = u32(a + 4), layer = u32(a + 8), kind = u32(a + 12);
  const ti = info.get(tex);
  if (!ti) return;
  const t = objs.get(tex);
  if (kind === 3) {
    const [hasD, hasS] = DEPTH_FORMATS[ti.format] || [1, 0];
    const ds = { view: subView(tex, mip, layer) };
    if (hasD) Object.assign(ds, { depthLoadOp: 'clear', depthStoreOp: 'store', depthClearValue: f32(a + 16) });
    if (hasS) Object.assign(ds, { stencilLoadOp: 'clear', stencilStoreOp: 'store', stencilClearValue: u32(a + 20) });
    enc.beginRenderPass({ colorAttachments: [], depthStencilAttachment: ds }).end();
    return;
  }
  const raw = [a + 16, a + 20, a + 24, a + 28];
  const clearValue = kind === 0 ? raw.map(f32) : raw.map((x) => (kind === 1 ? i32(x) : u32(x)));
  if (!(t.usage & GPUTextureUsage.RENDER_ATTACHMENT)) {
    // compressed or non-renderable: a zero clear by writing zeros
    if (clearValue.every((v) => v === 0)) {
      const fi = blockOf(ti.format);
      const w = Math.max(1, ti.w >> mip), h = Math.max(1, ti.h >> mip);
      const bpr = Math.ceil(w / fi.bw) * fi.bytes, rows = Math.ceil(h / fi.bh);
      device.queue.writeTexture({ texture: t, mipLevel: mip, origin: { x: 0, y: 0, z: layer } }, new Uint8Array(bpr * rows),
        { bytesPerRow: bpr }, { width: Math.ceil(w / fi.bw) * fi.bw, height: Math.ceil(h / fi.bh) * fi.bh, depthOrArrayLayers: 1 });
    } else once('clearfmt', `colour clear of ${ti.format} (not renderable on WebGPU): not cleared`);
    return;
  }
  const c = { view: subView(tex, mip, ti.dim === '3d' ? 0 : layer), loadOp: 'clear', storeOp: 'store', clearValue };
  if (ti.dim === '3d') c.depthSlice = layer;
  enc.beginRenderPass({ colorAttachments: [c] }).end();
}

const CLEAR_WGSL = `
struct U { c: vec4f, ci: vec4i, cu: vec4u, d: f32 }
@group(0) @binding(0) var<uniform> u: U;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  let c = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
  return vec4f(c.x * 2.0 - 1.0, 1.0 - c.y * 2.0, u.d, 1.0);
}
@fragment fn ff() -> @location(0) vec4f { return u.c; }
@fragment fn fi() -> @location(0) vec4i { return u.ci; }
@fragment fn fu() -> @location(0) vec4u { return u.cu; }
@fragment fn fd() {}
`;
let clearModule = null;
const clearPipes = new Map();
function clearRect(pass, pi, a) {
  const att = u32(a), x = i32(a + 4), y = i32(a + 8), w = u32(a + 12), h = u32(a + 16);
  if (!clearModule) clearModule = device.createShaderModule({ code: CLEAR_WGSL });
  const depthClear = att === 0xff;
  const fmt = depthClear ? pi.depth : pi.formats[att];
  if (!fmt) return;
  const key = JSON.stringify([pi.formats, pi.depth, pi.samples, att, depthClear ? u32(a + 20) : 0]);
  let cp = clearPipes.get(key);
  if (!cp) {
    const bgl = device.createBindGroupLayout({ entries: [{ binding: 0, visibility: 3, buffer: { type: 'uniform' } }] });
    const targets = pi.formats.map((f, i) => (f ? { format: f, writeMask: !depthClear && i === att ? 15 : 0 } : null));
    const entry = depthClear ? 'fd' : (isInt(fmt) ? (/sint$/.test(fmt) ? 'fi' : 'fu') : 'ff');
    const d = {
      layout: device.createPipelineLayout({ bindGroupLayouts: [bgl] }),
      vertex: { module: clearModule, entryPoint: 'vs' },
      fragment: { module: clearModule, entryPoint: entry, targets: depthClear ? pi.formats.map((f) => (f ? { format: f, writeMask: 0 } : null)) : targets },
      primitive: { topology: 'triangle-list' },
      multisample: { count: pi.samples > 1 ? 4 : 1 },
    };
    if (!depthClear) {
      // the other targets keep their texels: the fragment writes only to `att`; non-matching outputs are not allowed,
      // so the pipeline names only its target's output (others write mask 0 and are fed the same output type)
      if (targets.some((t, i) => t && i !== att && isInt(t.format) !== isInt(fmt))) {
        once('clearmix', 'a clear inside a pass with mixed integer / float targets: not cleared');
        return;
      }
    }
    if (pi.depth) {
      const aspects = u32(a + 20);
      const [hasD, hasS] = DEPTH_FORMATS[pi.depth] || [1, 0];
      d.depthStencil = { format: pi.depth, depthWriteEnabled: depthClear && !!(aspects & 2) && !!hasD, depthCompare: 'always' };
      if (depthClear && (aspects & 4) && hasS) {
        const f = { compare: 'always', passOp: 'replace', failOp: 'keep', depthFailOp: 'keep' };
        Object.assign(d.depthStencil, { stencilFront: f, stencilBack: f, stencilWriteMask: 0xff });
      }
    }
    cp = { pipe: device.createRenderPipeline(d), bgl };
    clearPipes.set(key, cp);
  }
  const ub = device.createBuffer({ size: 64, usage: GPUBufferUsage.UNIFORM, mappedAtCreation: true });
  const ab = ub.getMappedRange();
  const raw = new Uint32Array(D8.buffer, D8.byteOffset + a + 20, 4);
  if (depthClear) {
    new Float32Array(ab, 48, 1)[0] = f32(a + 24);
  } else {
    new Uint32Array(ab, 0, 4).set(raw);
    new Uint32Array(ab, 16, 4).set(raw);
    new Uint32Array(ab, 32, 4).set(raw);
  }
  ub.unmap();
  pass.setPipeline(cp.pipe);
  pass.setBindGroup(0, device.createBindGroup({ layout: cp.bgl, entries: [{ binding: 0, resource: { buffer: ub } }] }));
  if (depthClear) pass.setStencilReference(u32(a + 28));
  pass.setScissorRect(Math.max(0, x), Math.max(0, y), w, h);
  pass.draw(3);
}

function present(texId) {
  const t = objs.get(texId);
  if (!t) return false;
  const ti = info.get(texId);
  const cur = ctx.getCurrentTexture();
  const enc = device.createCommandEncoder();
  enc.copyTextureToTexture({ texture: t }, { texture: cur },
    { width: Math.min(ti.w, cur.width), height: Math.min(ti.h, cur.height), depthOrArrayLayers: 1 });
  device.queue.submit([enc.finish()]);
  const now = performance.now();
  if (lastPresent) H[H_FRAME_US] = Math.round((now - lastPresent) * 1000);
  frameTimes.push(now - lastPresent);
  lastPresent = now;
  const n = Atomics.add(H, H_PRESENTS, 1) + 1;
  if (n === 1) postMessage({ first: 1 });
  if (shotEvery && n % shotEvery === 0) shot(t, ti, n);
  if (frameTimes.length >= 300) {
    const v = frameTimes.splice(0).sort((x, y) => x - y);
    const avg = v.reduce((s, x) => s + x, 0) / v.length;
    say(`frames: ${(1000 / avg).toFixed(1)} FPS over the last ${v.length} presents (median ${v[v.length >> 1].toFixed(1)} ms, p95 ${v[Math.floor(v.length * 0.95)].toFixed(1)} ms); ring exec ${(H[H_EXEC_US] / 1000).toFixed(2)} ms last submission; ${H[H_ERRORS]} WebGPU errors`);
  }
  return true;
}

// development: every `shotEvery` presents the presented image goes to the dev server (POST /shot, dev_server.py --log)
const frameTimes = [];
let shotEvery = 0;
let shotBusy = false;
async function shot(t, ti, n) {
  if (shotBusy) return;
  shotBusy = true;
  try {
    const bpr = Math.ceil(ti.w * 4 / 256) * 256;
    const buf = device.createBuffer({ size: bpr * ti.h, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST });
    const enc = device.createCommandEncoder();
    enc.copyTextureToBuffer({ texture: t }, { buffer: buf, bytesPerRow: bpr }, { width: ti.w, height: ti.h });
    device.queue.submit([enc.finish()]);
    await buf.mapAsync(GPUMapMode.READ);
    const src = new Uint8Array(buf.getMappedRange());
    const px = new Uint8ClampedArray(ti.w * ti.h * 4);
    for (let y = 0; y < ti.h; y++) {
      for (let x = 0; x < ti.w; x++) {
        const s = y * bpr + x * 4, d = (y * ti.w + x) * 4;
        px[d] = src[s + 2]; px[d + 1] = src[s + 1]; px[d + 2] = src[s]; px[d + 3] = 255;
      }
    }
    buf.destroy();
    const oc = new OffscreenCanvas(ti.w, ti.h);
    oc.getContext('2d').putImageData(new ImageData(px, ti.w, ti.h), 0, 0);
    const blob = await oc.convertToBlob({ type: 'image/png' });
    await fetch(new URL('shot?n=' + n, self.location.href).href.replace(/gpu-worker\.js.*shot/, 'shot'), { method: 'POST', body: blob });
  } catch (e) {
    once('shot', 'shot: ' + e);
  }
  shotBusy = false;
}
