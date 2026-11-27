// Saves in the browser (docs/35 W3, WEB-W9): restores what the I/O worker stored (nxrs-saves/<title>/<save>.bin, a
// packed save folder, os/src/webdata.rs pack_save) into the in-memory file system before main. A save that cannot be
// read stops the start: the game must never begin with empty saves over stored ones.
//
// A save's file name is its folder name escaped (encodeURIComponent: `user1` stays `user1`, a nested `a/b` is
// `a%2Fb`), so restore puts it back at the same path. The menu (menu.js) lists, exports, imports and deletes them:
// an export (.nxsave) is `NXSE1` + version + the JSON description's length (16 bytes), the description
// {titleId, title, save, written, engine}, then the stored pack unchanged; an import is checked whole (its pack
// unpacked) before anything is written, and stored with a `.json` beside it saying when it was imported.
'use strict';
const NxrsSaves = (() => {
  const MAGIC = 'NXSV1\0\0\0';
  const EXPORT_MAGIC = 'NXSE1\0\0\0';
  const EXPORT_VERSION = 1;
  const safe = (s) => String(s).replace(/[^\w .-]/g, '_') || 'game';
  const fileName = (save) => encodeURIComponent(save);
  const saveName = (file) => decodeURIComponent(file);

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

  async function folder(title) {
    const opfs = await navigator.storage.getDirectory();
    return (await opfs.getDirectoryHandle('nxrs-saves', { create: true })).getDirectoryHandle(safe(title), { create: true });
  }

  // Every stored save of `title` into FS under `root` (/data/save); throws when one cannot be read. Returns the count.
  async function restore(FS, root, title, log) {
    let dir;
    try {
      dir = await folder(title);
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
      let save;
      try {
        save = saveName(m[1]);
        if (!save || save.split('/').includes('..')) throw new Error('not a save folder name');
        entries = unpack(await read(dir, n));
      } catch (e) {
        throw new Error(`the stored save ${n} cannot be read: ${e.message || e}`);
      }
      const base = root + '/' + save;
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
      log(`[web] saves: ${save} restored from the browser (${entries.length} entries, ${bytes} bytes${m[2] === 'new' ? ', from an interrupted commit' : ''})`);
    }
    if (!count) log('[web] saves: none stored in this browser yet');
    return count;
  }

  // The stored saves: [{save, file, size, written (ms), imported (ms or 0)}], by name.
  async function list(title) {
    const dir = await folder(title);
    const names = new Set();
    for await (const n of dir.keys()) names.add(n);
    const out = [];
    for (const n of names) {
      const m = /^(.*)\.(bin|new)$/.exec(n);
      if (!m || (m[2] === 'new' && names.has(m[1] + '.bin'))) continue;
      const f = await (await dir.getFileHandle(n)).getFile();
      let imported = 0;
      if (names.has(m[1] + '.json')) {
        try {
          imported = JSON.parse(new TextDecoder().decode(await read(dir, m[1] + '.json'))).imported || 0;
        } catch { /* no note */ }
      }
      out.push({ save: saveName(m[1]), file: n, size: f.size, written: f.lastModified, imported });
    }
    return out.sort((a, b) => a.save.localeCompare(b.save, undefined, { numeric: true }));
  }

  // The .nxsave file of one stored save.
  async function exportFile(title, entry, desc) {
    const dir = await folder(title);
    const pack = await read(dir, entry.file);
    const json = new TextEncoder().encode(JSON.stringify({ ...desc, save: entry.save, written: entry.written }) + '\n');
    const head = new Uint8Array(16);
    head.set(new TextEncoder().encode(EXPORT_MAGIC));
    new DataView(head.buffer).setUint32(8, EXPORT_VERSION, true);
    new DataView(head.buffer).setUint32(12, json.length, true);
    return new Blob([head, json, pack], { type: 'application/octet-stream' });
  }

  // A chosen .nxsave checked whole: {desc, pack}; throws with the reason the player reads.
  async function readExport(file, titleId, titleName) {
    const b = new Uint8Array(await file.arrayBuffer());
    if (b.length < 16 || new TextDecoder().decode(b.subarray(0, 8)) !== EXPORT_MAGIC) {
      throw new Error('This is not an nxrs save file (.nxsave).');
    }
    const dv = new DataView(b.buffer, b.byteOffset, b.byteLength);
    if (dv.getUint32(8, true) > EXPORT_VERSION) throw new Error('This save file comes from a newer nxrs.');
    const n = dv.getUint32(12, true);
    let desc;
    try {
      desc = JSON.parse(new TextDecoder().decode(b.subarray(16, 16 + n)));
    } catch {
      throw new Error('This save file is damaged.');
    }
    if (String(desc.titleId || '').toLowerCase() !== String(titleId || '').toLowerCase()) {
      throw new Error(`This save is for ${desc.title || 'another game'} (${desc.titleId || 'no title id'}), not ${titleName}.`);
    }
    if (!desc.save || String(desc.save).split('/').includes('..')) throw new Error('This save file names no save folder.');
    const pack = b.slice(16 + n);
    try {
      unpack(pack);
    } catch (e) {
      throw new Error(`This save file is damaged (${e.message || e}).`);
    }
    return { desc, pack };
  }

  async function writeFile(dir, name, bytes) {
    const h = await dir.getFileHandle(name, { create: true });
    const s = await h.createWritable();
    await s.write(bytes);
    await s.close();
  }

  // Stores a checked import as the save `save` (replacing one stored), noting when it was imported.
  async function importSave(title, save, pack) {
    const dir = await folder(title);
    const f = fileName(save);
    await writeFile(dir, f + '.new', pack);
    await dir.removeEntry(f + '.bin').catch(() => {});
    const h = await dir.getFileHandle(f + '.new');
    await h.move(f + '.bin');
    await writeFile(dir, f + '.json', new TextEncoder().encode(JSON.stringify({ imported: Date.now() })));
  }

  async function remove(title, save) {
    const dir = await folder(title);
    const f = fileName(save);
    for (const n of [f + '.bin', f + '.new', f + '.json']) await dir.removeEntry(n).catch(() => {});
  }

  return { restore, list, exportFile, readExport, importSave, remove, fileName };
})();
