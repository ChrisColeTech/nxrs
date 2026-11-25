#!/usr/bin/env python3
"""Imports nxnative's game profiles as data (developer side; the engine never runs Python).

    py -I src/profiles/tools/export.py <nxnative checkout> [--check]

Writes, under src/profiles/:
  games/<id>/game.tsv       each profile's game.tsv, byte for byte
  games/<id>/patches.toml   its patches.py as data (below)
  shared/sdk_patches.toml   dm/tools/sdk_patches.py (Nintendo SDK module builds, any game)
  shared/engine_patches.toml dm/tools/engine_patches.py (middleware: Unity, Unreal, ...)
  shared/settings.tsv       dm/games/settings.tsv (the player-facing settings list)
  shared/source.txt         the nxnative commit the files came from
Hand-written files beside them (games/<id>/owner.toml: the owner's patches) are never touched.

A patches.py is code: CUTS / OBS / CAPABILITIES / BINDING are data already; FRAME_STEP and STUBS (by build id) and the
stubs of MATCH_* rules are functions that assemble ARM words into the rewriter's patch area through a context
(here / emit / site / put / state / base / others). Each is run twice here, against a recording context placed at
two different layouts; a word that differs between the two is position-dependent, and its target is named by the
symbol whose displacement explains it (the patch area, the frame-step state page, the matched function, another
matched function, or a fixed module offset): ARM words plus relocations. A third layout then checks the recording:
relocating the recorded words to it must give exactly what the stub assembles there. A stub that reads the
module's own code (ctx.address, an anchored rule's find) cannot run without the game and is kept as a named entry
with evaluated = false.

--check compares the files on disk with a fresh export (and a TOML parse of each) without writing.
"""
import importlib.util
import subprocess
import sys
import tomllib
from pathlib import Path

sys.dont_write_bytecode = True   # importing the patch files must leave no __pycache__ in the nxnative checkout (-I ignores PYTHONDONTWRITEBYTECODE)
HERE = Path(__file__).resolve().parent
OUT = HERE.parent


class NeedsBinary(Exception):
    pass


# ---------------------------------------------------------------------------------------------- layouts and relocations
# Module offsets (the rewriter's coordinates: the module image starts at 0). Every symbol is page aligned and moves by
# a different amount between layouts, so a displacement names one symbol. The patch area stays within a B's reach of
# the code (+-128 MB) for the sites the profiles use.
LAYOUTS = [
    {'area': 0x0700_0000, 'state': 0x0600_0000, 'base': 0x0100_0000, 'other': 0x0200_0000},
    {'area': 0x0710_0000, 'state': 0x0602_3000, 'base': 0x0108_0000, 'other': 0x0204_1000},
    {'area': 0x0720_5000, 'state': 0x0604_7000, 'base': 0x0110_3000, 'other': 0x0208_3000},
]
OTHER_STEP = 0x0010_0000   # each other function label i sits at other + i * OTHER_STEP (+ its own layout delta i * 0x1000)


def symbols(layout, labels):
    s = {'module': 0, 'area': layout['area'], 'state': layout['state'], 'base': layout['base']}
    k = LAYOUTS.index(layout)
    for i, label in enumerate(sorted(labels)):
        s['other:' + label] = layout['other'] + i * OTHER_STEP + k * i * 0x1000
    return s


def op_of(w):
    if w & 0x7C000000 == 0x14000000: return 'b' if not w & 0x80000000 else 'bl'
    if w & 0xFF000010 == 0x54000000: return 'bcond'
    if w & 0x7E000000 == 0x34000000: return 'cbz'
    if w & 0x7E000000 == 0x36000000: return 'tbz'
    if w & 0x9F000000 == 0x90000000: return 'adrp'
    if w & 0x9F000000 == 0x10000000: return 'adr'
    return None


def sext(v, bits): return v - (1 << bits) if v & (1 << (bits - 1)) else v


def target_of(op, w, pc):
    if op in ('b', 'bl'): return pc + 4 * sext(w & 0x3FFFFFF, 26)
    if op in ('bcond', 'cbz'): return pc + 4 * sext((w >> 5) & 0x7FFFF, 19)
    if op == 'tbz': return pc + 4 * sext((w >> 5) & 0x3FFF, 14)
    imm = sext(((w >> 5) & 0x7FFFF) << 2 | (w >> 29) & 3, 21)
    if op == 'adrp': return (pc & ~0xFFF) + (imm << 12)
    return pc + imm


def encode(op, w, pc, target):
    if op in ('b', 'bl'):
        d = (target - pc) >> 2; assert -(1 << 25) <= d < (1 << 25); return (w & ~0x3FFFFFF) | (d & 0x3FFFFFF)
    if op in ('bcond', 'cbz'):
        d = (target - pc) >> 2; assert -(1 << 18) <= d < (1 << 18); return (w & ~(0x7FFFF << 5)) | ((d & 0x7FFFF) << 5)
    if op == 'tbz':
        d = (target - pc) >> 2; assert -(1 << 13) <= d < (1 << 13); return (w & ~(0x3FFF << 5)) | ((d & 0x3FFF) << 5)
    d = ((target & ~0xFFF) - (pc & ~0xFFF)) >> 12 if op == 'adrp' else target - pc
    assert -(1 << 20) <= d < (1 << 20)
    return (w & ~(0x60000000 | 0x7FFFF << 5)) | ((d & 3) << 29) | (((d >> 2) & 0x7FFFF) << 5)


def place(offsets, syms_a, syms_b):
    """(symbol, addend) of an offset seen as offsets[0] in layout a and offsets[1] in layout b."""
    delta = offsets[1] - offsets[0]
    names = [n for n in syms_a if syms_b[n] - syms_a[n] == delta]
    if len(names) != 1: raise ValueError(f'offset {offsets[0]:#x}: moves by {delta:#x}, which no single symbol explains ({names})')
    return names[0], offsets[0] - syms_a[names[0]]


# ---------------------------------------------------------------------------------------------- the recording context
class Rec:
    def __init__(self, layout, labels, found=None):
        self.syms = symbols(layout, labels)
        self.area, self.code = layout['area'], []
        self.sites, self.puts = [], []
        self.state, self.base = layout['state'], layout['base']
        self.others = {lab: self.syms['other:' + lab] for lab in labels}
        self.found = found
        self.render_scale = self.step_max = 0
        self.capabilities, self.fps_ready = set(), False

    def here(self): return self.area + 4 * len(self.code)
    def emit(self, w): self.code.append(w & 0xFFFFFFFF)
    def site(self, off, orig): self.sites.append((off, orig))
    def put(self, off, words, why): self.puts.append((off, [w & 0xFFFFFFFF for w in words], why))
    def address(self, off, lo_off=None): raise NeedsBinary('ctx.address (reads the module\'s own code)')


def run_stub(fn, labels, k):
    r = Rec(LAYOUTS[k], labels)
    fn(r)
    return r


def word_relocs(words_a, words_b, pc_a, pc_b, syms_a, syms_b):
    relocs = []
    for i, (wa, wb) in enumerate(zip(words_a, words_b)):
        if wa == wb: continue
        op = op_of(wa)
        if op is None or op_of(wb) != op: raise ValueError(f'word {i}: {wa:#010x} / {wb:#010x} differ but are not a pc-relative instruction')
        sym, add = place((target_of(op, wa, pc_a + 4 * i), target_of(op, wb, pc_b + 4 * i)), syms_a, syms_b)
        relocs.append({'index': i, 'op': op, 'target': sym, 'addend': add})
    return relocs


def record(fn, labels=()):
    """The stub as data: words + relocations, checked against a third layout. Raises NeedsBinary."""
    a, b, c = (run_stub(fn, labels, k) for k in range(3))
    if len(a.code) != len(b.code) or len(a.sites) != len(b.sites) or len(a.puts) != len(b.puts):
        raise ValueError('the stub assembles different amounts of code at different places')
    out = {
        'code': a.code,
        'relocs': word_relocs(a.code, b.code, a.area, b.area, a.syms, b.syms),
        'sites': [], 'puts': [],
        'step_max': a.step_max, 'render_scale': a.render_scale,
        'capabilities': sorted(a.capabilities), 'fps_ready': bool(a.fps_ready),
    }
    for (oa, orig), (ob, _) in zip(a.sites, b.sites):
        sym, add = place((oa, ob), a.syms, b.syms)
        out['sites'].append({'base': sym, 'offset': add, 'original': orig})
    for (oa, wa, why), (ob, wb, _) in zip(a.puts, b.puts):
        sym, add = place((oa, ob), a.syms, b.syms)
        out['puts'].append({'base': sym, 'offset': add, 'words': wa, 'reason': why,
                            'relocs': word_relocs(wa, wb, oa, ob, a.syms, b.syms)})
    # the check: the recording relocated to the third layout is what the stub assembles there
    want = materialize(out, c.syms)
    got = {'code': c.code, 'sites': [o for o, _ in c.sites], 'puts': [(o, w) for o, w, _ in c.puts]}
    if want != got: raise ValueError('the recorded relocations do not reproduce the stub at a third layout')
    return out


def materialize(rec, syms):
    area = syms['area']
    def apply(words, pc0, relocs):
        words = list(words)
        for r in relocs:
            words[r['index']] = encode(r['op'], words[r['index']], pc0 + 4 * r['index'], syms[r['target']] + r['addend'])
        return words
    return {'code': apply(rec['code'], area, rec['relocs']),
            'sites': [syms[s['base']] + s['offset'] for s in rec['sites']],
            'puts': [(syms[p['base']] + p['offset'], apply(p['words'], syms[p['base']] + p['offset'], p['relocs'])) for p in rec['puts']]}


# ---------------------------------------------------------------------------------------------- TOML writer (our schema)
def tstr(s):
    s = str(s)
    return '"' + s.replace('\\', '\\\\').replace('"', '\\"').replace('\n', '\\n').replace('\r', '\\r').replace('\t', '\\t') + '"'


def tval(v, hexint=False):
    if isinstance(v, bool): return 'true' if v else 'false'
    if isinstance(v, int): return f'0x{v:x}' if hexint and v >= 0 else str(v)   # TOML has no negative hex
    if isinstance(v, str): return tstr(v)
    if isinstance(v, (list, tuple)): return '[' + ', '.join(tval(x, hexint) for x in v) + ']'
    if isinstance(v, dict): return '{ ' + ', '.join(f'{k} = {tval(x, hexint or k in HEX)}' for k, x in v.items()) + ' }'
    raise TypeError(f'{type(v)}: {v!r}')


HEX = {'offset', 'delta', 'size', 'addend', 'original_word', 'code', 'words'}


def table(name, d, nested=()):
    lines = [f'[[{name}]]']
    for k, v in d.items():
        if k in nested or v is None: continue
        lines.append(f'{k} = {tval(v, k in HEX)}')
    for k in nested:
        if d.get(k) is None: continue
        sub = d[k]
        lines.append(f'[{name}.{k}]')
        for kk, vv in sub.items():
            if vv is not None: lines.append(f'{kk} = {tval(vv, kk in HEX)}')
    return '\n'.join(lines) + '\n'


# ---------------------------------------------------------------------------------------------- patches.py → data
def load_module(path, n):
    spec = importlib.util.spec_from_file_location(f'patches_export_{n}', path)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def stub_entry(fn, labels=(), anchored=False):
    if anchored:
        return {'evaluated': False, 'needs': "an anchored rule's find (reads the module's own code)"}
    try:
        rec = record(fn, labels)
    except NeedsBinary as e:
        return {'evaluated': False, 'needs': str(e)}
    return {'evaluated': True, **rec}


def cut_list(cuts):
    return [{'delta': d, 'original': o, 'replacement': n, 'reason': r} for d, o, n, r in cuts]


def match_rule(kind, module, rule):
    e = {'kind': kind, 'module': module, 'name': rule.get('name', '')}
    if 'functions' in rule:   # a cut of several functions, all or nothing
        e['functions'] = [{'size': f['size'], 'fingerprints': list(f['fingerprints']), 'cuts': cut_list(f['cuts'])} for f in rule['functions']]
        return e
    for k in ('size', 'calls'):
        if k in rule: e[k] = rule[k]
    if 'fingerprints' in rule: e['fingerprints'] = list(rule['fingerprints'])
    if 'fps_required' in rule: e['fps_required'] = bool(rule['fps_required'])
    if 'reviewed' in rule: e['reviewed'] = [repr(x) for x in rule['reviewed']]
    if 'cuts' in rule: e['cuts'] = cut_list(rule['cuts'])
    if 'others' in rule:
        e['others'] = [{'label': lab, 'size': f['size'], 'fingerprints': list(f['fingerprints'])} for lab, f in sorted(rule['others'].items())]
    if 'stub' in rule:
        e['stub'] = stub_entry(rule['stub'], tuple(rule.get('others', {})), anchored='calls' in rule)
    return e


def export_patches(path, n, source):
    m = load_module(path, n)
    parts = [f'# Exported by src/profiles/tools/export.py from {source}; do not edit (re-run the exporter).\n'
             f'source = {tstr(source)}\n']
    for bid, cuts in sorted(getattr(m, 'CUTS', {}).items()):
        for off, orig, new, why in cuts:
            parts.append(table('cut', {'build': bid, 'offset': off, 'original': orig, 'replacement': new, 'reason': why}))
    for bid, obs in sorted(getattr(m, 'OBS', {}).items()):
        for off, num, word, why in obs:
            parts.append(table('obs', {'build': bid, 'offset': off, 'number': num, 'original_word': int(word, 16) if isinstance(word, str) else word, 'reason': why}))
    for bid, names in sorted(getattr(m, 'CAPABILITIES', {}).items()):
        parts.append(table('capability', {'build': bid, 'names': list(names)}))
    for b in getattr(m, 'BINDING', []):
        parts.append(table('binding', {k: (str(v) if not isinstance(v, (str, int, bool)) else v) for k, v in b.items()}))
    for kind, attr in (('frame_step', 'FRAME_STEP'), ('stub', 'STUBS')):
        for bid, fn in sorted(getattr(m, attr, {}).items()):
            e = {'kind': kind, 'build': bid, **stub_entry(fn)}
            parts.append(stub_table('stub', e))
    for kind, attr in (('cuts', 'MATCH_CUTS'), ('frame_step', 'MATCH_FRAME_STEP'), ('stubs', 'MATCH_STUBS')):
        for module, rules in sorted(getattr(m, attr, {}).items()):
            for rule in rules:
                parts.append(match_table(match_rule(kind, module, rule)))
    return '\n'.join(parts)


def stub_table(name, e):
    """[[stub]] with its sites / puts / relocs as inline tables."""
    return table(name, e)


def match_table(e):
    stub = e.pop('stub', None)
    text = table('match', e)
    if stub is not None:
        text += f'[match.stub]\n' + ''.join(f'{k} = {tval(v, k in HEX)}\n' for k, v in stub.items() if v is not None)
    return text


# ---------------------------------------------------------------------------------------------- main
def main():
    args = [a for a in sys.argv[1:] if not a.startswith('--')]
    check = '--check' in sys.argv
    if len(args) != 1: sys.exit(__doc__)
    nx = Path(args[0]).resolve()
    games, tools = nx / 'dm' / 'games', nx / 'dm' / 'tools'
    sys.path[:0] = [str(tools), str(games)]
    commit = subprocess.run(['git', '-C', str(nx), 'log', '-1', '--format=%H %cI'], capture_output=True, text=True).stdout.strip()
    files = {}   # path under OUT -> bytes
    n = 0
    stats = {'profiles': 0, 'patch_files': 0, 'cut': 0, 'stub': 0, 'stub_unevaluated': 0, 'match': 0}
    for d in sorted(games.iterdir()):
        if not (d / 'game.tsv').is_file(): continue
        stats['profiles'] += 1
        files[f'games/{d.name}/game.tsv'] = (d / 'game.tsv').read_bytes()
        if (d / 'patches.py').is_file():
            n += 1; stats['patch_files'] += 1
            files[f'games/{d.name}/patches.toml'] = export_patches(d / 'patches.py', n, f'nxnative dm/games/{d.name}/patches.py').encode()
    for name in ('sdk_patches', 'engine_patches'):
        n += 1
        files[f'shared/{name}.toml'] = export_patches(tools / f'{name}.py', n, f'nxnative dm/tools/{name}.py').encode()
    files['shared/settings.tsv'] = (games / 'settings.tsv').read_bytes()
    files['shared/source.txt'] = f'nxnative {commit}\n'.encode()
    for rel, data in files.items():
        if rel.endswith('.toml'):
            t = tomllib.loads(data.decode())
            stats['cut'] += len(t.get('cut', []))
            stats['stub'] += len(t.get('stub', [])) + sum('stub' in mm for mm in t.get('match', []))
            stats['stub_unevaluated'] += sum(not s.get('evaluated', True) for s in t.get('stub', [])) + sum(not mm['stub']['evaluated'] for mm in t.get('match', []) if 'stub' in mm)
            stats['match'] += len(t.get('match', []))
    if check:
        bad = [rel for rel, data in files.items() if not (OUT / rel).is_file() or (OUT / rel).read_bytes() != data]
        print(f'{len(files)} files, {len(bad)} differ: {bad[:10]}')
        sys.exit(1 if bad else 0)
    for rel, data in files.items():
        p = OUT / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        if not (p.is_file() and p.read_bytes() == data): p.write_bytes(data)
    print(f'exported {stats} from {commit}')


if __name__ == '__main__':
    main()
