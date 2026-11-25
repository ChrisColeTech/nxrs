#!/usr/bin/env python3
"""The environment switches the engine reads, as one table: docs/env_switches.tsv (name, class, meaning, where).
nxnative's dm/tools/env_switches.py for this tree's switches (NXGFX_, NXOS_, NXRS_, NXRT_, NXSHADERS_, NXCFG_).

  env_switches.py           write the table: a new switch gets a row whose class is guessed and whose meaning is the
                            comment line that names it in the code (empty when none does); class and meaning of rows
                            already there are kept (edit them by hand); a switch nothing reads any more loses its row
  env_switches.py --check   exit 1 when the table does not list exactly the switches the sources read, or a row has no
                            meaning or an unknown class

Classes: player (a setting a player may use), path (where something is), switch (turns a behaviour on / off),
diagnostic (logs, dumps, measurements), test (tests and tools only).
"""
from pathlib import Path
import re
import sys

REPO = Path(__file__).resolve().parents[1]
TABLE = REPO / 'docs' / 'env_switches.tsv'
HEAD = 'name\tclass\tmeaning\twhere\n'
CLASSES = ('player', 'path', 'switch', 'diagnostic', 'test')
NAME = r'(?:NXGFX|NXOS|NXRS|NXRT|NXSHADERS|NXCFG)_[A-Z0-9_]*[A-Z0-9]'
# a literal name the code reads: env::var / var_os / env_flag-like helpers all take it as a string literal
READ = re.compile(r'"(' + NAME + r')"')


def sources():
    for p in sorted((REPO / 'src').rglob('*.rs')):
        if 'target' in p.parts:
            continue
        yield p


def scan():
    """{switch: (files, the first comment line naming it, read only by tests)}"""
    found = {}
    for p in sources():
        text = p.read_text(encoding='utf-8', errors='replace')
        rel = p.relative_to(REPO).as_posix()
        lines = text.split('\n')
        test_from = next((i for i, l in enumerate(lines) if l.strip().startswith('#[cfg(test)]')), len(lines))
        is_test = '/tests/' in rel
        for i, l in enumerate(lines):
            for n in READ.findall(l):
                files, note, only_tests = found.get(n, (set(), '', True))
                files.add(rel)
                found[n] = (files, note, only_tests and (is_test or i > test_from))
        for i, l in enumerate(lines):
            s = l.strip()
            if not s.startswith('//'):
                continue
            for n in re.findall(NAME, s):
                if n in found and not found[n][1]:
                    files, _, t = found[n]
                    found[n] = (files, re.sub(r'^/+!?\s*', '', s), t)
    return found


def guess(name, only_tests):
    if only_tests:
        return 'test'
    if re.search(r'DIR|PATH|FILE|ROOT', name):
        return 'path'
    if re.search(r'LOG|DUMP|TRACE|PROFILE|DEBUG|STATS|SHOT|CSV|PERF|CHECK|VALID|REPORT|CAPTURE', name):
        return 'diagnostic'
    return 'switch'


def load():
    rows = {}
    if TABLE.exists():
        for l in TABLE.read_text(encoding='utf-8').split('\n')[1:]:
            f = l.split('\t')
            if len(f) >= 3 and f[0]:
                rows[f[0]] = (f[1], f[2])
    return rows


def main():
    found = scan()
    rows = load()
    if '--check' in sys.argv[1:]:
        bad = []
        for n in sorted(set(found) - set(rows)):
            bad.append(f'{n}: read by {", ".join(sorted(found[n][0]))} but not in the table')
        for n in sorted(set(rows) - set(found)):
            bad.append(f'{n}: in the table but nothing reads it')
        for n, (c, m) in sorted(rows.items()):
            if c not in CLASSES:
                bad.append(f'{n}: unknown class {c!r}')
            if not m.strip():
                bad.append(f'{n}: no meaning')
        for b in bad:
            print(b)
        print(f'{len(found)} switches read, {len(rows)} rows, {len(bad)} problems')
        return 1 if bad else 0
    out = [HEAD]
    for n in sorted(found):
        files, note, only_tests = found[n]
        c, m = rows.get(n, (guess(n, only_tests), note))
        out.append(f'{n}\t{c}\t{m}\t{", ".join(sorted(files))}\n')
    TABLE.write_text(''.join(out), encoding='utf-8', newline='\n')
    empty = sum(1 for l in out[1:] if not l.split('\t')[2].strip())
    print(f'{TABLE.relative_to(REPO)}: {len(found)} switches ({empty} without a meaning yet)')
    return 0


if __name__ == '__main__':
    sys.exit(main())
