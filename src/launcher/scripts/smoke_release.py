#!/usr/bin/env python3
"""Extract a release away from the checkout and drive it with no help (no NXRS_BUILD, no PATH, a disposable profile):
the checksums, the engine's doctor, a build of a small game folder through the launcher (--smoke-build), and the
built game imported twice (--import-build --screenshot: one entry, its settings kept, the executable untouched).

    py -I scripts/smoke_release.py <nxrs-...zip|tar.gz> [--source <game folder: exefs/ + romfs/>]

The default source is the engine checkout's build/runtime-e2e/hello (a homebrew; no keys needed). Nothing is run.
"""
import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path, PurePosixPath

LAUNCHER = Path(__file__).resolve().parents[1]
ROOT = LAUNCHER.parents[1]
WIN = os.name == 'nt'
EXE = '.exe' if WIN else ''


def digest(path):
    with open(path, 'rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def safe(name):
    p = PurePosixPath(name)
    if not name or p.is_absolute() or '..' in p.parts or '\\' in name or ':' in name:
        raise RuntimeError('unsafe archive path: ' + name)


def extract(archive, dest):
    sha, name = Path(str(archive) + '.sha256').read_text().split()
    if name != archive.name or sha != digest(archive):
        raise RuntimeError('archive checksum mismatch')
    if archive.suffix == '.zip':
        with zipfile.ZipFile(archive) as z:
            for n in z.namelist():
                safe(n)
            z.extractall(dest)
    else:
        with tarfile.open(archive) as t:
            for n in t.getnames():
                safe(n)
            t.extractall(dest, filter='data')
    (root,) = list(dest.iterdir())
    return root


def verify_manifest(root):
    sums = root / 'legal/SHA256SUMS'
    listed = {}
    for line in sums.read_text().splitlines():
        sha, name = line.split('  ', 1)
        safe(name)
        listed[name] = sha
    actual = {p.relative_to(root).as_posix() for p in root.rglob('*') if p.is_file() and p != sums}
    if set(listed) != actual:
        raise RuntimeError('the manifest does not cover exactly the release: ' + str(set(listed) ^ actual))
    for name, sha in listed.items():
        if digest(root / name) != sha:
            raise RuntimeError('checksum mismatch: ' + name)


def run(args, cwd, env, ok=(0,), timeout=1800):
    r = subprocess.run([str(a) for a in args], cwd=cwd, env=env, capture_output=True, text=True, timeout=timeout)
    if r.returncode not in ok:
        raise RuntimeError(f'{Path(str(args[0])).name} exited {r.returncode}\n{r.stdout}\n{r.stderr}')
    return r.stdout + r.stderr


def clean_env(temp):
    """Nothing from this machine that could find the engine for the launcher, and a disposable profile."""
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(('NXRS_', 'PYTHON', 'CARGO', 'RUST', 'VULKAN_SDK')) and k not in ('LD_LIBRARY_PATH',)}
    env['PATH'] = str(Path(os.environ.get('SystemRoot', 'C:/Windows')) / 'System32') if WIN else '/usr/bin:/bin'
    # shaped like a real profile: the GPU driver keeps its shader cache in <profile>/AppData/Local, and in a bare
    # folder its first start stalls for minutes
    profile = temp / 'profile'
    folders = {'APPDATA': 'AppData/Roaming', 'LOCALAPPDATA': 'AppData/Local', 'TEMP': 'AppData/Local/Temp',
               'TMP': 'AppData/Local/Temp', 'XDG_CONFIG_HOME': '.config', 'XDG_CACHE_HOME': '.cache',
               'XDG_DATA_HOME': '.local/share'}
    for key, sub in folders.items():
        (profile / sub).mkdir(parents=True, exist_ok=True)
        env[key] = str(profile / sub)
    env['HOME'] = env['USERPROFILE'] = str(profile)
    env['NXRS_DATA_DIR'] = str(temp / 'data')   # never the owner's library, whatever the profile says
    return env


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('archive', type=Path)
    ap.add_argument('--source', type=Path, default=ROOT / 'build/runtime-e2e/hello')
    args = ap.parse_args()
    if not (args.source / 'exefs').is_dir():
        sys.exit(f'{args.source}: not a game folder (exefs/ + romfs/)')
    with tempfile.TemporaryDirectory(prefix='nxrs relocated [test] ') as temp:
        temp = Path(temp)
        root = extract(args.archive.resolve(), temp / 'release')
        verify_manifest(root)
        env = clean_env(temp)
        nxrs = root / ('nxrs' + EXE)
        print(run([nxrs, '--nxrs-doctor'], temp, env, ok=(0, 1)).strip())
        source = temp / 'Hello [v0] & game'
        shutil.copytree(args.source, source, ignore=shutil.ignore_patterns('gen-*', '*.exe', '*.ini'))
        output, library = temp / 'Build output', temp / 'library.json'
        # the launcher finds nxrs-engine/ beside itself: no NXRS_BUILD, nothing chosen in Settings
        run([nxrs, '--library', library, '--smoke-build', source, '--build-output', output], temp, env)
        (game,) = json.loads(library.read_text(encoding='utf-8'))['games']
        if game.get('buildIncomplete') or game.get('engine') != 'nxrs':
            raise RuntimeError('the build is incomplete or not an nxrs build: ' + json.dumps(game))
        built = output / (game['outputName'] + EXE)
        if not built.is_file():
            raise RuntimeError('missing build artifact: ' + built.name)
        before = digest(built)
        for i in range(2):
            shot = temp / f'import {i}.png'
            run([nxrs, '--library', library, '--import-build', built, '--screenshot', shot], temp, env, timeout=120)
            games = json.loads(library.read_text(encoding='utf-8'))['games']
            if len(games) != 1 or games[0].get('buildIncomplete') or games[0].get('engine') != 'nxrs':
                raise RuntimeError('importing the build lost its settings or duplicated the entry')
            if not shot.is_file() or shot.stat().st_size < 1000:
                raise RuntimeError('no screenshot')
        if digest(built) != before:
            raise RuntimeError('the import modified the executable')
        print('PASS: archive + file checksums, doctor, engine found beside the launcher (no NXRS_BUILD/PATH), '
              f'game folder built ({built.name}), imported twice (one entry, settings kept, executable untouched)')
    return 0


if __name__ == '__main__':
    sys.exit(main())
