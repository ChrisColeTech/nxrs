"""Reference for the Rust package reader: list a RomFS and hash sample files with the owner's Turbo readers
(nsp.py / nca.py / nup.py), read-only. Run with the Turbo checkout as the import root:

  py -B scripts/pkg/turbo_romfs.py <Turbo dir> <package> <out prefix> [--keys K] [--update U | --no-update] [--sample N]

Writes <out prefix>.list ("size  path" for every file, sorted by path) and <out prefix>.sha ("sha256  size  path
patched" for the sample: N files spread over the tree plus, for a BKTR-patched RomFS, N files that read bytes from
the update's patch section).
"""
import hashlib
import os
import sys

turbo, package, out = sys.argv[1:4]
args = sys.argv[4:]
opt = lambda name: args[args.index(name) + 1] if name in args else None
sys.path.insert(0, turbo)
import nca  # noqa: E402
import nsp  # noqa: E402
import nup  # noqa: E402

keys, update, sample = opt('--keys'), opt('--update'), int(opt('--sample') or 30)
if nup.is_nup(package):
    rom = nup.NupRomfs(nup.Nup(package))
    patched_ranges = []
else:
    pkg = nca.Package(package, keys, update, '--no-update' not in args)
    rom = nca.PackageRomfs(pkg)
    patched_ranges = []
    if isinstance(pkg.storage, nca.Patched):
        rel = pkg.storage.relocations
        for i, (virtual, _, source) in enumerate(rel):
            end = rel[i + 1][0] if i + 1 < len(rel) else pkg.storage.size
            if source: patched_ranges.append((virtual - pkg.romfs_at, end - pkg.romfs_at))

files = sorted(rom.walk_files(), key=lambda item: item[0])
with open(out + '.list', 'w', encoding='utf-8', newline='\n') as f:
    for path, node in files:
        f.write('%12d  %s\n' % (node.size, path))


def touches_patch(node):
    if not hasattr(node, 'offset') or not patched_ranges or not node.size: return False
    return any(lo < node.offset + node.size and node.offset < hi for lo, hi in patched_ranges)


chosen = [files[i * len(files) // sample] for i in range(sample)] if files else []
patched = [item for item in files if touches_patch(item[1])]
if patched:
    chosen += [patched[i * len(patched) // sample] for i in range(min(sample, len(patched)))]
seen = set()
with open(out + '.sha', 'w', encoding='utf-8', newline='\n') as f:
    for path, node in chosen:
        if path in seen or node.size > 64 << 20: continue
        seen.add(path)
        f.write('%s  %d  %s %d\n' % (hashlib.sha256(rom.read(node)).hexdigest(), node.size, path, touches_patch(node)))
print('%d files, %d hashed (%d touching the patch)' % (len(files), len(seen), sum(1 for p, n in chosen if p in seen and touches_patch(n))))
