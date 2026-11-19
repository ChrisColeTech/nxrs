r"""Dump mkw-recomp's jump-table resolver results for every br of one or more modules (reference for the Rust port).

Run from D:\Projects\mkw-recomp-main:  uv run python D:/Projects/RecompLauncherEngine/scripts/jt/dump_py.py [names...]
Writes D:/Projects/RecompLauncherEngine/build/jt/<module>.txt: a '# base 0x...' header, then one line per br:
`br entry end is_computed result` (result = comma list of hex targets or None, EXC:<type> on an exception).
(JT_STARTS=seeds: function starts from mod.seeds() only, written to <module>.seeds.txt.)
"""
import os
import sys
import time

sys.path.insert(0, r'D:\Projects\mkw-recomp-main\recomp')
import gen  # noqa: E402
import jumptables  # noqa: E402
import layout  # noqa: E402
import module  # noqa: E402

EXEFS = r'D:\Projects\RecompLauncherEngine\build\ref\exefs'
OUT = r'D:\Projects\RecompLauncherEngine\build\jt'

SEEDS = os.environ.get("JT_STARTS") == "seeds"  # coarser functions: seeds only (stress test)
names = sys.argv[1:] or ['sdk', 'subsdk0', 'subsdk1', 'main']
bases = {n: b for n, b, _ in layout.layout(EXEFS)}
for name in names:
    t0 = time.time()
    mod = module.Module(name, f'{EXEFS}/{name}', bases[name], 'c')
    starts = sorted(mod.seeds()) if SEEDS else [mod.lo + 4 * int(i) for i in gen.discover(mod)[0]]
    ends = starts[1:] + [mod.hi]
    n = 0
    with open(f"{OUT}/{name}{'.seeds' if SEEDS else ''}.txt", 'w') as f:
        f.write(f'# base {mod.base:#x}\n')
        for entry, end in zip(starts, ends):
            for a in range(entry, end, 4):
                if (mod.word(a) & 0xFFFFFC1F) != 0xD61F0000:
                    continue
                try:
                    r = jumptables.resolve(mod, a, entry, end)
                    r = 'None' if r is None else ','.join(f'{t:x}' for t in r)
                except Exception as e:  # noqa: BLE001
                    r = f'EXC:{type(e).__name__}'
                try:
                    c = int(jumptables.is_computed(mod, a, entry))
                except Exception as e:  # noqa: BLE001
                    c = f'EXC:{type(e).__name__}'
                f.write(f'{a:x} {entry:x} {end:x} {c} {r}\n')
                n += 1
    print(name, n, 'brs', f'{time.time() - t0:.1f}s', flush=True)
