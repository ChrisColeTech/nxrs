#!/usr/bin/env python3
"""Collect what a bug report needs about one game into a zip file to attach to an issue, and never any game data
(nxnative scripts/bug_report.py for this engine's game folders).

    py scripts/bug_report.py <game folder or its .exe> [--out FILE] [--max-kb 512]

A game folder is what nxrs-build writes: <name>.exe, <name>_code.dll, <name>.ini, <name>_data\\ (exefs, saves, shader
translations: never read here). The zip holds report.md (the engine build, the machine, the game's settings and what
the folder shows) and the text files ALLOWED names: the game's log, its ini, captures\\threads.txt (F5), the newest
metrics / frames CSV, the shader build's manifest summary and problems list. Every text file is redacted (the home
folder becomes ~, lines that look like key material are dropped) and cut to --max-kb from its end.
Exit code: 0 done, 1 the zip could not be written, 2 the target is not a game folder.
"""
import argparse
import datetime
import os
import platform
import re
import subprocess
import sys
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
KEY_LINE = re.compile(r'(?i)(\b[0-9a-f]{32,}\b.*=|prod\.keys|title\.keys|header_key|key_area_key|titlekek)')


def redact(text):
    home = str(Path.home())
    out = []
    for line in text.splitlines():
        if KEY_LINE.search(line):
            out.append('[line dropped: it looked like key material]')
            continue
        out.append(line.replace(home, '~').replace(home.replace('\\', '/'), '~'))
    return '\n'.join(out) + '\n'


def tail(path, max_bytes):
    data = path.read_bytes()
    cut = len(data) > max_bytes
    data = data[-max_bytes:]
    text = data.decode('utf-8', errors='replace')
    return (f'[first {len(path.read_bytes()) - max_bytes} bytes cut]\n' if cut else '') + text


def newest(folder, pattern):
    files = sorted(folder.glob(pattern), key=lambda p: p.stat().st_mtime)
    return files[-1] if files else None


def game_folder(target):
    p = Path(target)
    if p.suffix.lower() == '.exe':
        return p.parent, p.stem
    if p.is_dir():
        exes = [e for e in p.glob('*.exe') if (p / f'{e.stem}_code.dll').exists()]
        if len(exes) == 1:
            return p, exes[0].stem
    return None, None


def allowed(folder, name):
    """(name in the zip, path) of every file a report may hold."""
    out = []
    for n, p in [
        (f'{name}.log', folder / f'{name}.log'),
        (f'{name}.ini', folder / f'{name}.ini'),
        ('threads.txt', folder / 'captures' / 'threads.txt'),
        ('metrics.csv', newest(folder, 'metrics-*.csv')),
        ('frames.csv', newest(folder, 'frames-*.csv')),
        ('shaders-problems.txt', folder / f'{name}_data' / 'shaders' / 'problems.txt'),
        ('shaders-regressions.tsv', folder / f'{name}_data' / 'shaders' / 'regressions.tsv'),
    ]:
        if p is not None and p.is_file():
            out.append((n, p))
    return out


def engine_version():
    try:
        r = subprocess.run(['git', 'describe', '--always', '--dirty'], cwd=REPO, capture_output=True, text=True, timeout=10)
        return r.stdout.strip() or 'unknown'
    except OSError:
        return 'unknown'


def manifest_summary(folder, name):
    m = folder / f'{name}_data' / 'shaders' / 'manifest.tsv'
    if not m.is_file():
        return 'no shader build'
    head = [l for l in m.read_text(errors='replace').splitlines()[:3] if l.startswith('#')]
    return '; '.join(h.lstrip('# ') for h in head)


def report(folder, name, files):
    ini = (folder / f'{name}.ini')
    settings = []
    if ini.is_file():
        settings = [l for l in ini.read_text(errors='replace').splitlines() if l.strip() and not l.lstrip().startswith(';')]
    lines = [
        f'# Bug report: {name}',
        '',
        f'- Written: {datetime.datetime.now().isoformat(timespec="seconds")}',
        f'- Engine: nxrs-engine {engine_version()}',
        f'- Machine: {platform.platform()}, {platform.machine()}, {os.cpu_count()} logical processors',
        f'- Shaders: {manifest_summary(folder, name)}',
        f'- Files: {", ".join(n for n, _ in files) or "none"}',
        '',
        '## What happened',
        '',
        '(describe what you did, what you saw and what you expected; the game version and where in the game)',
        '',
        '## Settings',
        '',
        '```',
        *settings,
        '```',
        '',
    ]
    return redact('\n'.join(lines))


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('target')
    ap.add_argument('--out')
    ap.add_argument('--max-kb', type=int, default=512)
    a = ap.parse_args(argv)
    folder, name = game_folder(a.target)
    if folder is None:
        print(f'{a.target}: not a game folder (no <name>.exe beside <name>_code.dll)', file=sys.stderr)
        return 2
    files = allowed(folder, name)
    out = Path(a.out) if a.out else folder / f'bug-report-{name}-{datetime.datetime.now():%Y%m%d-%H%M%S}.zip'
    try:
        with zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED) as z:
            z.writestr('report.md', report(folder, name, files))
            for n, p in files:
                z.writestr(n, redact(tail(p, a.max_kb << 10)))
    except OSError as e:
        print(f'{out}: {e}', file=sys.stderr)
        return 1
    print(f'{out}: report.md and {len(files)} file(s)')
    return 0


if __name__ == '__main__':
    sys.exit(main())
