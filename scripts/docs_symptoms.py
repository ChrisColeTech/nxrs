#!/usr/bin/env python3
"""docs/symptoms.md, the symptom index, from the reference docs (nxnative dm/tools/docs_symptoms.py): one line per entry
that has a `- **Symptom:**` bullet (its text, links reduced to their text) with a link to the entry's heading, grouped by
reference file in the order docs/reference/README.md lists them. Also checks that every repository path the reference
docs name in backticks (`src/...`, `scripts/...`, `docs/...`) exists.

  docs_symptoms.py           write the index
  docs_symptoms.py --check   exit 1 when the index is not current or a named path does not exist
"""
import re
import sys
import unicodedata
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
REF = REPO / 'docs' / 'reference'
OUT = REPO / 'docs' / 'symptoms.md'
PATH = re.compile(r'`((?:src|scripts|docs)/[A-Za-z0-9_./\\-]+?)`')


def anchor(heading):
    """GitHub's heading anchor: lower case, punctuation dropped, spaces to hyphens."""
    s = unicodedata.normalize('NFKC', heading.strip().lower())
    s = re.sub(r'[^\w\- ]', '', s)
    return s.replace(' ', '-')


def order():
    readme = REF / 'README.md'
    names = re.findall(r'\]\(([\w.-]+\.md)\)', readme.read_text(encoding='utf-8')) if readme.exists() else []
    rest = sorted(p.name for p in REF.glob('*.md') if p.name != 'README.md' and p.name not in names)
    return [n for n in names if (REF / n).exists()] + rest


def entries(path):
    heading, out = None, []
    for line in path.read_text(encoding='utf-8').splitlines():
        m = re.match(r'#{2,4} (.+)', line)
        if m:
            heading = m.group(1).strip()
            continue
        m = re.match(r'\s*- \*\*Symptom:\*\*\s*(.+)', line)
        if m and heading:
            text = re.sub(r'\[([^\]]+)\]\([^)]+\)', r'\1', m.group(1)).strip()
            out.append((heading, text))
    return out


def index():
    lines = ['# Symptoms', '', 'What a player or developer sees, and the reference entry that explains it. Written by `scripts/docs_symptoms.py` from the',
             '`- **Symptom:**` bullets of [the reference docs](reference/README.md); edit those, not this page.', '']
    for name in order():
        es = entries(REF / name)
        if not es:
            continue
        lines += [f'## [{name[:-3]}](reference/{name})', '']
        lines += [f'- {t} ([{h}](reference/{name}#{anchor(h)}))' for h, t in es]
        lines.append('')
    return '\n'.join(lines)


def missing_paths():
    bad = []
    for p in sorted(REF.glob('*.md')):
        for m in PATH.finditer(p.read_text(encoding='utf-8')):
            q = m.group(1).rstrip('.').replace('\\', '/')
            if any(c in q for c in '*<>{}'):
                continue
            # a path in another project's repository, named as such on its line
            text = p.read_text(encoding='utf-8')
            line = text[:m.start()].rsplit(chr(10), 1)[-1] + text[m.end():].split(chr(10), 1)[0]
            if "repository" in line and ("Atmosph" in line or "nxnative" in line):
                continue
            if not (REPO / q).exists():
                bad.append(f'{p.relative_to(REPO).as_posix()}: `{q}` does not exist')
    return bad


def main():
    text = index()
    if '--check' in sys.argv[1:]:
        problems = missing_paths()
        if not OUT.exists() or OUT.read_text(encoding='utf-8') != text:
            problems.append('docs/symptoms.md is not current: run scripts/docs_symptoms.py')
        for x in problems:
            print(x)
        print(f'{len(problems)} problems')
        return 1 if problems else 0
    OUT.write_text(text, encoding='utf-8', newline='\n')
    print(f'{OUT.relative_to(REPO).as_posix()}: {sum(len(entries(REF / n)) for n in order())} symptoms')
    return 0


if __name__ == '__main__':
    sys.exit(main())
