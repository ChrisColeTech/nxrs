"""Hash every file of a RomFS image with the owner's Turbo reader (nsp.romfs_tree), e.g. a DLC romfs.bin Turbo's
installer wrote: py -B scripts/pkg/turbo_romfs_image.py <Turbo dir> <romfs.bin>  ->  "sha256  size  path" lines."""
import hashlib
import os
import sys

sys.path.insert(0, sys.argv[1])
import nsp  # noqa: E402

image = sys.argv[2]
with open(image, 'rb') as f:
    data = f.read()
root = nsp.romfs_tree(image, len(data), lambda offset, size: data[offset:offset + size])
rom = nsp.NspRomfs(type('P', (), {'path': image, 'root': root, 'read': lambda self, n: data[n.offset:n.offset + n.size]})())
for path, node in sorted(rom.walk_files(), key=lambda item: item[0]):
    sys.stdout.write('%s  %d  %s\n' % (hashlib.sha256(data[node.offset:node.offset + node.size]).hexdigest(), node.size, path))
