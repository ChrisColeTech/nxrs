// The I/O worker (docs/35 W3): every game-data read of the engine and every save commit, served from shared memory
// so no engine thread ever needs the page's thread. Started by the page (boot.js) before the module loads.
//
// Protocol (os/src/web/io.rs is the engine's side): a table of 32-bit words in linear memory; header 16 words ([0]
// doorbell, [1] slot count, [2] version, [3] slots freed), then slots of 16 words ([0] state 0 idle / 4 claimed /
// 1 requested / 3 taken here / 2 done, [1] op 1 read / 2 save, [2] file id or name length, [3..5) offset or name
// address, [5..7) destination or data address, [7] length, [8] result). This worker waits on the doorbell
// (Atomics.waitAsync), takes requested slots (compareExchange 1 -> 3), serves each asynchronously, writes the result,
// marks it done and notifies the slot's state word, on which the engine thread is blocked.
//
// Data: files are the manifest's (data.json; a file's index is its id), fetched by HTTP range from data/<path> in
// 4 KiB blocks into an OPFS store (nxrs-data/<title>/store.bin, append-only, and journal.bin: a 16-byte header with
// the data version, then 16-byte entries file, block, store offset lo, hi; data is flushed before the journal entries
// that point at it, every 2 s). Another data version starts an empty store. Reads are copied from the store straight
// into linear memory. Without OPFS (another tab holds the store, or none), a memory cache. The boot set (bootset.json)
// is fetched at low priority as soon as the worker starts, while the page downloads and compiles the module; demand
// reads go at high priority and join a fetch already in flight.
//
// Saves: op 2 stores a packed save (os/src/webdata.rs pack_save) as nxrs-saves/<title>/<name>.bin through
// <name>.new (written, flushed, then moved over the old one); the result is the acknowledgement.
'use strict';

const BS = 4096;
const RUN_MAX = 256; // blocks per fetch (1 MiB)
const BOOT_LANES = 16;
const MEM_BLOCKS = 65536; // the memory cache: 256 MiB
const JOURNAL_MAGIC = 0x314a584e; // 'NXJ1'

let files = []; // [path, size]
let version = '';
let title = '';
let memory = null;
let table = 0;
let slots = 0;

let store = null; // OPFS sync access handles
let journal = null;
let storeEnd = 0;
let pending = []; // journal entries not yet written
const index = new Map(); // block key -> store offset (OPFS) or Uint8Array (memory)
const inflight = new Map(); // block key -> Promise
// boot / demand: bytes fetched (compressed where the file is); zraw: the data those fetches carried, inflated
const stats = { boot: 0, demand: 0, zraw: 0, hit: 0, reads: 0, fetches: 0, bootFiles: 0, bootBytes: 0, bootDoneBytes: 0, bootDone: false, bootMs: 0, cached: 0 };

const log = (text) => postMessage({ log: text });
const key = (f, b) => f * 4294967296 + b;
const safe = (s) => String(s).replace(/[^\w .-]/g, '_') || 'game';
const url = (f) => 'data/' + files[f][0].split('/').map(encodeURIComponent).join('/');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---------------------------------------------------------------- the store

async function retryHandle(fileHandle) {
  // a reload finds the previous page's worker still holding the lock for a moment
  for (let i = 0; ; i++) {
    try {
      return await fileHandle.createSyncAccessHandle();
    } catch (e) {
      if (i >= 50) throw e;
      await sleep(200);
    }
  }
}

function verWords() {
  const v = BigInt('0x' + (/^[0-9a-f]{1,16}$/.test(version) ? version : '0'));
  return [Number(v & 0xffffffffn), Number(v >> 32n)];
}

async function openStore(fresh) {
  try {
    const root = await navigator.storage.getDirectory();
    const dir = await (await root.getDirectoryHandle('nxrs-data', { create: true })).getDirectoryHandle(safe(title), { create: true });
    store = await retryHandle(await dir.getFileHandle('store.bin', { create: true }));
    journal = await retryHandle(await dir.getFileHandle('journal.bin', { create: true }));
  } catch (e) {
    store = journal = null;
    log(`[RED] io: no OPFS store (${e}): a memory cache of ${MEM_BLOCKS * BS / 2 ** 20} MiB, nothing kept for the next start`);
    return 'memory';
  }
  const [lo, hi] = verWords();
  const size = journal.getSize();
  const head = new Uint32Array(4);
  if (size >= 16) journal.read(head, { at: 0 });
  if (fresh || size < 16 || head[0] !== JOURNAL_MAGIC || head[1] !== lo || head[2] !== hi) {
    store.truncate(0);
    journal.truncate(0);
    journal.write(new Uint32Array([JOURNAL_MAGIC, lo, hi, 0]), { at: 0 });
    journal.flush();
    storeEnd = 0;
    log(`io: OPFS store for data version ${version} started empty${fresh ? ' (asked for: ?fresh=1)' : size >= 16 ? ' (another data version was there)' : ''}`);
    return 'opfs';
  }
  const storeSize = store.getSize();
  const n = Math.floor((size - 16) / 16);
  const ent = new Uint32Array(n * 4);
  journal.read(ent, { at: 16 });
  for (let i = 0; i < n; i++) {
    const off = ent[i * 4 + 2] + ent[i * 4 + 3] * 4294967296;
    if (off + BS <= storeSize && ent[i * 4] < files.length) index.set(key(ent[i * 4], ent[i * 4 + 1]), off);
  }
  storeEnd = storeSize;
  stats.cached = index.size;
  // drop entries past the last whole one (a crash mid-append)
  if (size !== 16 + n * 16) journal.truncate(16 + n * 16);
  log(`io: OPFS store for data version ${version}: ${index.size} blocks (${(index.size * BS / 2 ** 20).toFixed(1)} MiB) cached`);
  return 'opfs';
}

function flushJournal() {
  if (!store || !pending.length) return;
  store.flush(); // the data first, then the entries that point at it
  const ent = new Uint32Array(pending.length * 4);
  pending.forEach((e, i) => ent.set(e, i * 4));
  journal.write(ent, { at: journal.getSize() });
  journal.flush();
  pending = [];
}

function closeStore() {
  try {
    flushJournal();
    store?.close();
    journal?.close();
  } catch (e) { /* closing anyway */ }
  store = journal = null;
}

function keep(f, b0, bytes) {
  const n = Math.ceil(bytes.length / BS);
  if (store) {
    const at = storeEnd;
    storeEnd += n * BS;
    store.write(bytes, { at });
    // whole blocks: the last one padded, so every journal entry lies inside the store
    if (bytes.length < n * BS) store.truncate(storeEnd);
    for (let i = 0; i < n; i++) {
      index.set(key(f, b0 + i), at + i * BS);
      const off = at + i * BS;
      pending.push([f, b0 + i, off % 4294967296, Math.floor(off / 4294967296)]);
    }
  } else {
    for (let i = 0; i < n; i++) {
      index.set(key(f, b0 + i), bytes.slice(i * BS, (i + 1) * BS));
      if (index.size > MEM_BLOCKS) index.delete(index.keys().next().value);
    }
  }
}

// ---------------------------------------------------------------- fetching

// A compressed file (WEB-W6, src/cli/src/webzip.rs): data.json lists it as [path, size, [extent, end0, end1, ...]];
// zdata/<path> holds its extents, each raw-deflated alone (or stored as is when that did not shrink it: its length is
// then the extent's own). Whole extents are fetched by exact range from the compressed file and inflated here; the
// store keeps the inflated blocks, so the cache and the reads are as for an identity file.
const zurl = (f) => 'zdata/' + files[f][0].split('/').map(encodeURIComponent).join('/');

async function inflate(bytes) {
  const s = new Blob([bytes]).stream().pipeThrough(new DecompressionStream('deflate-raw'));
  return new Uint8Array(await new Response(s).arrayBuffer());
}

async function fetchExtents(f, b0, b1, priority, kind) {
  const [size, z] = [files[f][1], files[f][2]];
  const E = z[0];
  const ends = z; // ends[1 + i] is extent i's start in the compressed file
  const x0 = Math.floor((b0 * BS) / E);
  const x1 = Math.floor((Math.min((b1 + 1) * BS, size) - 1) / E);
  const first = ends[1 + x0];
  const last = ends[2 + x1] - 1;
  const zsize = ends[ends.length - 1];
  const buf = await rangeGet(zurl(f), files[f][0] + ' (compressed)', first, last, zsize, priority);
  stats[kind] += buf.length;
  stats.zraw += Math.min((x1 + 1) * E, size) - x0 * E;
  stats.fetches++;
  const parts = await Promise.all(Array.from({ length: x1 - x0 + 1 }, (_, i) => {
    const x = x0 + i;
    const part = buf.subarray(ends[1 + x] - first, ends[2 + x] - first);
    const rawLen = Math.min(E, size - x * E);
    return part.length === rawLen ? part : inflate(part);
  }));
  const out = new Uint8Array(Math.min((x1 + 1) * E, size) - x0 * E);
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  if (at !== out.length) throw new Error(`${files[f][0]}: extents ${x0}-${x1} inflated to ${at} bytes, wanted ${out.length}`);
  keep(f, (x0 * E) / BS, out);
}

// bytes [first, last] of url (a file of `size` bytes), checked: exact 206 Content-Range or a whole 200, retried
async function rangeGet(u, name, first, last, size, priority) {
  for (let attempt = 0; ; attempt++) {
    let r;
    try {
      r = await fetch(u, { headers: { Range: `bytes=${first}-${last}` }, cache: 'no-store', priority });
    } catch (e) {
      r = null;
    }
    if (r && (r.status === 206 || r.status === 200)) {
      const ct = r.headers.get('Content-Type') || '';
      if (ct.startsWith('text/html')) throw new Error(`${name}: the server answered with a page, not data`);
      if (r.status === 206) {
        const want = `bytes ${first}-${last}/${size}`;
        if (r.headers.get('Content-Range') !== want) {
          throw new Error(`${name}: Content-Range ${r.headers.get('Content-Range')}, wanted ${want}`);
        }
      }
      let buf = new Uint8Array(await r.arrayBuffer());
      if (r.status === 200) {
        if (buf.length !== size) throw new Error(`${name}: ${buf.length} bytes, the manifest says ${size}`);
        buf = buf.subarray(first, last + 1);
      }
      if (buf.length !== last - first + 1) throw new Error(`${name}: ${buf.length} bytes for ${last - first + 1}`);
      return buf;
    }
    if (r && r.status < 500 && r.status !== 429) throw new Error(`${name}: HTTP ${r.status}`);
    if (attempt >= 7) throw new Error(`${name}: ${r ? 'HTTP ' + r.status : 'no response'} after ${attempt + 1} tries`);
    await sleep(250 * 2 ** attempt * (0.5 + Math.random()));
  }
}

async function fetchRun(f, b0, b1, priority, kind) {
  if (files[f][2]) return fetchExtents(f, b0, b1, priority, kind);
  const size = files[f][1];
  const first = b0 * BS;
  const last = Math.min((b1 + 1) * BS, size) - 1;
  const buf = await rangeGet(url(f), files[f][0], first, last, size, priority);
  stats[kind] += buf.length;
  stats.zraw += buf.length;
  stats.fetches++;
  keep(f, b0, buf);
}

// a compressed file's blocks come in whole extents: [b0, b1] widened to its extents' bounds
function extentBounds(f, b0, b1) {
  const z = files[f][2];
  if (!z) return [b0, b1];
  const per = z[0] / BS;
  const lastBlock = Math.ceil(files[f][1] / BS) - 1;
  return [Math.floor(b0 / per) * per, Math.min(lastBlock, Math.floor(b1 / per) * per + per - 1)];
}

// Every block of [b0, b1] of file f present; returns whether all were already there.
async function ensure(f, b0, b1, priority, kind) {
  [b0, b1] = extentBounds(f, b0, b1);
  const waits = [];
  let run = -1;
  let hit = true;
  const flush = (end) => {
    if (run < 0) return;
    for (let s = run; s < end; s += RUN_MAX) {
      const e = Math.min(end, s + RUN_MAX) - 1;
      const p = fetchRun(f, s, e, priority, kind);
      for (let b = s; b <= e; b++) inflight.set(key(f, b), p);
      p.finally(() => { for (let b = s; b <= e; b++) inflight.delete(key(f, b)); });
      waits.push(p);
    }
    run = -1;
  };
  for (let b = b0; b <= b1; b++) {
    const k = key(f, b);
    if (index.has(k)) { flush(b); continue; }
    hit = false;
    const p = inflight.get(k);
    if (p) { flush(b); waits.push(p); continue; }
    if (run < 0) run = b;
  }
  flush(b1 + 1);
  await Promise.all(waits);
  return hit;
}

function heap(addr, n) {
  return new Uint8Array(memory.buffer, addr, n);
}

async function serveRead(f, off, dst, len) {
  if (f >= files.length) return -2;
  const size = files[f][1];
  if (off >= size || len === 0) return 0;
  const end = Math.min(off + len, size);
  const b0 = Math.floor(off / BS);
  const b1 = Math.floor((end - 1) / BS);
  stats.reads++;
  if (await ensure(f, b0, b1, 'high', 'demand')) stats.hit += end - off;
  for (let b = b0; b <= b1; b++) {
    const bs = b * BS;
    const from = Math.max(off, bs);
    const to = Math.min(end, bs + BS);
    const v = index.get(key(f, b));
    const out = heap(dst + (from - off), to - from);
    if (v === undefined) return -3; // evicted from the memory cache meanwhile: the engine retries
    if (typeof v === 'number') {
      try {
        store.read(out, { at: v + (from - bs) });
      } catch (e) {
        // a browser that refuses shared views for file reads: through a temporary buffer
        const t = new Uint8Array(to - from);
        store.read(t, { at: v + (from - bs) });
        out.set(t);
      }
    } else {
      out.set(v.subarray(from - bs, to - bs));
    }
  }
  return end - off;
}

// ---------------------------------------------------------------- saves

let saveChain = Promise.resolve();

async function saveCommit(name, bytes) {
  const root = await navigator.storage.getDirectory();
  const dir = await (await root.getDirectoryHandle('nxrs-saves', { create: true })).getDirectoryHandle(safe(title), { create: true });
  // the save folder's name escaped, as saves.js restores it (a nested a/b was flattened to a_b and came back there)
  const base = encodeURIComponent(name);
  const fh = await dir.getFileHandle(base + '.new', { create: true });
  const h = await retryHandle(fh);
  try {
    h.truncate(0);
    if (h.write(bytes, { at: 0 }) !== bytes.length) throw new Error('short write');
    h.flush();
  } finally {
    h.close();
  }
  try {
    await fh.move(base + '.bin');
  } catch (e) {
    // a browser whose move does not replace: remove the old one first (restore takes a lone .new)
    await dir.removeEntry(base + '.bin').catch(() => {});
    await fh.move(base + '.bin');
  }
  // written by the game now, no longer the imported copy (saves.js's note)
  await dir.removeEntry(base + '.json').catch(() => {});
}

// ---------------------------------------------------------------- the slot loop

function handle(i32, s) {
  const op = i32[s + 1];
  const a = i32[s + 2];
  const lo = (x) => i32[s + x] >>> 0;
  const off = lo(3) + lo(4) * 4294967296;
  const addr = lo(5) + lo(6) * 4294967296;
  const len = lo(7);
  const done = (r) => {
    Atomics.store(i32, s + 8, r | 0);
    Atomics.store(i32, s, 2);
    Atomics.notify(i32, s);
  };
  if (op === 1) {
    serveRead(a, off, addr, len).then(done, (e) => {
      log(`[RED] io: read of ${files[a]?.[0] ?? 'file ' + a} at ${off} (+${len}): ${e.message || e}`);
      done(-1);
    });
  } else if (op === 2) {
    const name = new TextDecoder().decode(heap(off, a).slice());
    const bytes = heap(addr, len).slice();
    const t0 = performance.now();
    // the page hears of every stored save (its menu lists it) and of a failed one (the menu says so)
    saveChain = saveChain.then(() => saveCommit(name, bytes)).then(
      () => {
        log(`io: save ${name} written to the browser's storage (${bytes.length} bytes)`);
        done(0);
        postMessage({ saved: { name, bytes: bytes.length, ms: performance.now() - t0 } });
      },
      (e) => {
        log(`[RED] io: save ${name}: ${e.message || e}`);
        done(-1);
        postMessage({ saveFailed: { name, reason: String(e.message || e) } });
      });
  } else {
    done(-4);
  }
}

async function loop() {
  const i32 = new Int32Array(memory.buffer, table, 16 + 16 * slots);
  for (;;) {
    const bell = Atomics.load(i32, 0);
    for (let i = 0; i < slots; i++) {
      const s = 16 + 16 * i;
      if (Atomics.compareExchange(i32, s, 1, 3) === 1) handle(i32, s);
    }
    const w = Atomics.waitAsync(i32, 0, bell);
    if (w.async) await w.value;
  }
}

// ---------------------------------------------------------------- the boot set

async function prefetch(bootset) {
  const t0 = performance.now();
  const byPath = new Map(files.map((f, i) => [f[0], i]));
  const jobs = [];
  for (const [path, ranges] of bootset) {
    const f = byPath.get(path);
    if (f === undefined) continue;
    stats.bootFiles++;
    for (const [a, b] of ranges) {
      stats.bootBytes += b - a + 1;
      jobs.push([f, Math.floor(a / BS), Math.floor(Math.min(b, files[f][1] - 1) / BS), b - a + 1]);
    }
  }
  let next = 0;
  const lane = async () => {
    while (next < jobs.length) {
      const [f, b0, b1, raw] = jobs[next++];
      try {
        await ensure(f, b0, b1, 'low', 'boot');
        // the loading screen's measure: the boot set's bytes present, fetched now or already cached
        stats.bootDoneBytes += raw;
      } catch (e) {
        log(`[RED] io: boot set ${files[f][0]}: ${e.message || e}`);
      }
    }
  };
  await Promise.all(Array.from({ length: BOOT_LANES }, lane));
  stats.bootDone = true;
  stats.bootMs = performance.now() - t0;
  log(`io: boot set ${stats.bootFiles} files, ${(stats.bootBytes / 2 ** 20).toFixed(1)} MiB: ${(stats.boot / 2 ** 20).toFixed(1)} MiB fetched, the rest cached, in ${(stats.bootMs / 1000).toFixed(2)} s`);
}

onmessage = async (e) => {
  const m = e.data;
  if (m.init) {
    ({ files, version, title } = m.init);
    const kind = await openStore(m.init.fresh);
    setInterval(flushJournal, 2000);
    postMessage({ ready: kind });
    prefetch(m.init.bootset || []);
  } else if (m.start) {
    ({ memory, table } = m.start);
    slots = new Int32Array(memory.buffer, table, 16)[1];
    log(`io: serving the engine's reads (${slots} slots at ${table})`);
    loop();
  } else if (m.stats) {
    postMessage({ stats: { ...stats, cachedNow: index.size } });
  } else if (m.close) {
    closeStore();
  }
};
