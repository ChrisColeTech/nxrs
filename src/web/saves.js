// Saves in the browser (docs/35 W3): restores what the I/O worker stored (nxrs-saves/<title>/<save>.bin, a packed
// save folder, os/src/webdata.rs pack_save) into the in-memory file system before main. A save that cannot be read
// stops the start: the game must never begin with empty saves over stored ones.
'use strict';
const NxrsSaves = (() => {
  const MAGIC = 'NXSV1\0\0\0';
  const safe = (s) => String(s).replace(/[^\w .-]/g, '_') || 'game';

  function unpack(bytes) {
    const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    if (new TextDecoder().decode(bytes.subarray(0, 8)) !== MAGIC) throw new Error('not a packed save');
    const out = [];
    let at = 8;
    while (at < bytes.length) {
      const kind = dv.getUint32(at, true);
      const plen = dv.getUint32(at + 4, true);
      const path = new TextDecoder().decode(bytes.subarray(at + 8, at + 8 + plen));
      const size = dv.getUint32(at + 8 + plen, true) + dv.getUint32(at + 12 + plen, true) * 4294967296;
      const start = at + 16 + plen;
      if (start + size > bytes.length || path.split('/').includes('..')) throw new Error('damaged at byte ' + at);
      out.push([kind, path, bytes.subarray(start, start + size)]);
      at = start + size;
    }
    return out;
  }

  async function read(dir, name) {
    return new Uint8Array(await (await (await dir.getFileHandle(name)).getFile()).arrayBuffer());
  }

  // Every stored save of `title` into FS under `root` (/data/save); throws when one cannot be read.
  async function restore(FS, root, title, log) {
    let dir;
    try {
      const opfs = await navigator.storage.getDirectory();
      dir = await (await opfs.getDirectoryHandle('nxrs-saves', { create: true })).getDirectoryHandle(safe(title), { create: true });
    } catch (e) {
      throw new Error(`the browser's storage (OPFS) cannot be opened: ${e}`);
    }
    const names = new Set();
    for await (const n of dir.keys()) names.add(n);
    FS.mkdirTree(root);
    let count = 0;
    for (const n of names) {
      const m = /^(.*)\.(bin|new)$/.exec(n);
      if (!m || (m[2] === 'new' && names.has(m[1] + '.bin'))) continue;
      let entries;
      try {
        entries = unpack(await read(dir, n));
      } catch (e) {
        throw new Error(`the stored save ${n} cannot be read: ${e.message || e}`);
      }
      const base = root + '/' + m[1];
      FS.mkdirTree(base);
      let bytes = 0;
      for (const [kind, path, data] of entries) {
        if (kind === 0) FS.mkdirTree(base + '/' + path);
        else {
          FS.writeFile(base + '/' + path, data);
          bytes += data.length;
        }
      }
      count++;
      log(`[web] saves: ${m[1]} restored from the browser (${entries.length} entries, ${bytes} bytes${m[2] === 'new' ? ', from an interrupted commit' : ''})`);
    }
    if (!count) log('[web] saves: none stored in this browser yet');
  }
  return { restore };
})();
