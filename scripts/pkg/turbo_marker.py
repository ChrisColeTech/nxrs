"""Write the romfs.nsp marker Turbo's nca.Package builds for a game (with --update/--no-update), to compare with the
one nxrs stages. The file holds content keys still wrapped, as a build's marker does; nothing is printed.
  py -B scripts/pkg/turbo_marker.py <Turbo dir> <game> <out file> [--update U | --no-update]"""
import sys

sys.path.insert(0, sys.argv[1])
import nca  # noqa: E402

args = sys.argv[4:]
update = args[args.index('--update') + 1] if '--update' in args else None
package = nca.Package(sys.argv[2], None, update, '--no-update' not in args)
with open(sys.argv[3], 'w', encoding='utf-8', newline='\n') as f:
    f.write(package.marker())
