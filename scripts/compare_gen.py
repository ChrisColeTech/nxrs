"""Compare two generated-C trees (mkw-recomp gen.py vs nxrs translate): whole files and per function.

Usage: py scripts/compare_gen.py <reference dir> <candidate dir> [--show N]
Files compared: layout.h, layout.json, patches.h, <mod>/<mod>_NNNN.c, <mod>/<mod>_table.c (manifest.json holds
timings and is skipped). Per function: each chunk's body is split into the blocks gen.py joins with a blank line,
keyed by the f_<addr> it defines.
"""
import re
import sys
from pathlib import Path

FN = re.compile(r'^void (f_[0-9a-f]+)\(Cpu \*c\) \{$', re.M)


def functions(path):
    text = path.read_text(encoding='utf-8')
    out = {}
    for block in text.split('\n\n'):
        m = FN.search(block)
        if m and '}' in block:
            out[m.group(1)] = block
    return out


def main():
    ref, cand = Path(sys.argv[1]), Path(sys.argv[2])
    show = int(sys.argv[sys.argv.index('--show') + 1]) if '--show' in sys.argv else 3
    files_same = files_diff = 0
    missing = []
    fn_same = fn_diff = fn_missing = 0
    shown = 0
    for p in sorted(ref.rglob('*')):
        if p.is_dir() or p.name == 'manifest.json':
            continue
        rel = p.relative_to(ref)
        q = cand / rel
        if not q.exists():
            missing.append(str(rel))
            continue
        if p.read_bytes() == q.read_bytes():
            files_same += 1
            if p.suffix == '.c' and not p.name.endswith('_table.c'):
                fn_same += len(FN.findall(p.read_text(encoding='utf-8')))
            continue
        files_diff += 1
        if p.suffix != '.c' or p.name.endswith('_table.c'):
            print(f'DIFF {rel}')
            continue
        a, b = functions(p), functions(q)
        for k, v in a.items():
            if k not in b:
                fn_missing += 1
            elif b[k] == v:
                fn_same += 1
            else:
                fn_diff += 1
                if shown < show:
                    shown += 1
                    la, lb = v.splitlines(), b[k].splitlines()
                    for i, (x, y) in enumerate(zip(la, lb)):
                        if x != y:
                            print(f'--- {rel} {k} line {i}\n  ref: {x[:300]}\n  rs : {y[:300]}')
                            break
                    else:
                        print(f'--- {rel} {k}: length {len(la)} vs {len(lb)}')
    print(f'files: {files_same} identical, {files_diff} differ, {len(missing)} missing {missing[:5]}')
    print(f'functions: {fn_same} identical, {fn_diff} differ, {fn_missing} missing')


main()
