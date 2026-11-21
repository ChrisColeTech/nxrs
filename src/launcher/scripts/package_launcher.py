#!/usr/bin/env python3
"""Stage the nxrs release and archive it with a SHA-256 manifest.

    py -I scripts/package_launcher.py --engine <nxrs-engine archive or folder> [--launcher <nxrs(.exe)>]
                                      [--llvm-mingw <dir>] [--out dist]

nxrs/
  nxrs(.exe)              the launcher (one executable: egui, no runtime libraries beside it)
  nxrs-engine/            the engine bundle scripts/package_engine.py makes (nxrs-build, engine.json, libraries,
                          tools/, LICENSES/); the launcher finds it beside itself
  toolchain/llvm-mingw/   Windows, when the bundle holds the gnullvm engine (gnu/): the x86_64 compiler and linker a
                          game builds with on a machine without Visual Studio (nxrs-build looks in ..\\toolchain)
  legal/                  the launcher's LICENSE, its dependencies' licenses, build-info.json, SHA256SUMS
Output: <out>/nxrs-<platform>-x64.zip (Windows) or .tar.gz (Linux), with a .sha256 beside it.
"""
import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path

LAUNCHER = Path(__file__).resolve().parents[1]
ROOT = LAUNCHER.parents[1]
NAME = 'nxrs'
ENGINE, TOOLCHAIN, LEGAL = Path('nxrs-engine'), Path('toolchain'), Path('legal')
CONTRACT = 2   # constants::engine::CONTRACT_NEEDED
# llvm-mingw as released holds four targets, a debugger and Python; a build needs the x86_64 compiler and linker
LLVM_SKIP = ('aarch64-w64-mingw32', 'armv7-w64-mingw32', 'i686-w64-mingw32', 'python', 'lldb*', 'liblldb*',
             'clangd*', 'clang-tidy*', 'libpython*', '__pycache__', '*.pdb')
PRIVATE = re.compile(r'(^|/)(library(\.qt-backup)?\.json|prod\.keys|title\.keys)$|\.(log|nsp|xci|nca|nso|nro|keys)$', re.I)


def platform_name():
    return 'windows' if os.name == 'nt' else 'linux'


def exe(name, platform):
    return name + ('.exe' if platform == 'windows' else '')


def digest(path):
    with open(path, 'rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def unpack_engine(source, temp):
    """The engine bundle as a folder: an archive is extracted (it holds one nxrs-engine/ folder)."""
    source = Path(source)
    if source.is_dir():
        return source
    dest = Path(temp) / 'engine'
    if source.suffix == '.zip':
        with zipfile.ZipFile(source) as z:
            z.extractall(dest)
    else:
        with tarfile.open(source) as t:
            t.extractall(dest, filter='data')
    folder = dest / ENGINE.name
    if not folder.is_dir():
        sys.exit(f'{source}: no {ENGINE.name}/ folder inside')
    return folder


def check_engine(engine, platform):
    """nxrs-build answering this launcher's contract, its engine.json, one complete set of libraries, the tools."""
    for name in (exe('nxrs-build', platform), 'engine.json', exe('tools/dmsh_xlate', platform),
                 exe('tools/spirv-val', platform)):
        if not (engine / name).is_file():
            sys.exit(f'the engine bundle lacks {name}')
    info = json.loads((engine / 'engine.json').read_text())
    if info.get('engine') != 'nxrs' or int(info.get('contract', 0)) < CONTRACT:
        sys.exit(f'engine.json does not answer launcher contract {CONTRACT}: {info}')
    sets = {'msvc': ('nxfront.lib', 'nxrt.lib'), 'gnu': ('gnu/libnxfront.a', 'gnu/libnxrt.a'),
            'linux': ('libnxfront.a', 'libnxrt.a')}
    present = [k for k, names in sets.items() if all((engine / n).is_file() for n in names)]
    if not present:
        sys.exit('the engine bundle holds no engine libraries')
    return info, 'gnu' in present


def launcher_license():
    found = next((p for p in (LAUNCHER / 'LICENSE', ROOT / 'LICENSE', ROOT / 'LICENSE-MIT') if p.is_file()), None)
    return found.read_text(encoding='utf-8') if found else 'nxrs launcher: MIT License.\n'


def dependency_licenses(stage):
    """The launcher's crates and their licenses, as cargo reports them (no network: the lock file decides)."""
    host = re.search(r'host: (\S+)', subprocess.run(['rustc', '-vV'], capture_output=True, text=True).stdout)
    meta = subprocess.run(['cargo', 'metadata', '--format-version', '1', '--locked', '--offline',
                           '--filter-platform', host.group(1) if host else ''], cwd=LAUNCHER,
                          capture_output=True, text=True, encoding='utf-8')
    if meta.returncode:
        sys.exit('cargo metadata failed: ' + meta.stderr)
    rows = sorted({(p['name'], p['version'], p.get('license') or 'see crate') for p in json.loads(meta.stdout)['packages']})
    (stage / LEGAL / 'THIRD_PARTY.txt').write_text(
        'Crates linked into nxrs (name version: license):\n' + ''.join(f'{n} {v}: {l}\n' for n, v, l in rows),
        encoding='utf-8')


def manifest(stage):
    sums = stage / LEGAL / 'SHA256SUMS'
    files = sorted(p for p in stage.rglob('*') if p.is_file() and p != sums)
    for p in files:
        rel = p.relative_to(stage).as_posix()
        if PRIVATE.search(rel):
            sys.exit('personal or generated file in the release: ' + rel)
    sums.write_text(''.join(f'{digest(p)}  {p.relative_to(stage).as_posix()}\n' for p in files))


def archive(stage, out, platform):
    name = f'{NAME}-{platform}-x64'
    if platform == 'windows':
        path = out / f'{name}.zip'
        with zipfile.ZipFile(path, 'w', zipfile.ZIP_DEFLATED) as z:
            for p in sorted(stage.rglob('*')):
                if p.is_file():
                    z.write(p, Path(NAME) / p.relative_to(stage))
    else:
        path = out / f'{name}.tar.gz'
        with tarfile.open(path, 'w:gz') as t:
            t.add(stage, arcname=NAME)
    (out / f'{path.name}.sha256').write_text(f'{digest(path)}  {path.name}\n')
    return path


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--engine', required=True, help='nxrs-engine-<platform>-x64 archive, or an nxrs-engine folder')
    ap.add_argument('--launcher', help='the built launcher (default: the repository target/release/nxrs)')
    ap.add_argument('--llvm-mingw', help='llvm-mingw (Windows, gnullvm engine; default: build/tools/llvm-mingw)')
    ap.add_argument('--out', default=str(LAUNCHER / 'dist'))
    args = ap.parse_args()
    platform = platform_name()
    launcher = Path(args.launcher or ROOT / 'target/release' / exe(NAME, platform))
    if not launcher.is_file():
        sys.exit(f'{launcher}: build it first (cargo build --release --jobs 4)')
    out = Path(args.out)
    stage = out / 'stage'
    shutil.rmtree(stage, ignore_errors=True)
    stage.mkdir(parents=True)
    with tempfile.TemporaryDirectory() as temp:
        engine = unpack_engine(args.engine, temp)
        info, gnu = check_engine(engine, platform)
        shutil.copy2(launcher, stage / exe(NAME, platform))
        shutil.copytree(engine, stage / ENGINE)
    if platform == 'windows' and gnu:
        llvm = Path(args.llvm_mingw or ROOT / 'build/tools/llvm-mingw')
        if not (llvm / 'bin/clang.exe').is_file():
            sys.exit(f'{llvm}: no bin/clang.exe (the gnullvm engine links with llvm-mingw)')
        shutil.copytree(llvm, stage / TOOLCHAIN / 'llvm-mingw', ignore=shutil.ignore_patterns(*LLVM_SKIP))
    legal = stage / LEGAL
    legal.mkdir()
    (legal / 'LICENSE.txt').write_text(launcher_license(), encoding='utf-8')
    dependency_licenses(stage)
    version = (LAUNCHER / 'VERSION').read_text().strip()
    (legal / 'build-info.json').write_text(json.dumps(
        {'version': version, 'platform': platform, 'engine': info}, indent=2) + '\n')
    manifest(stage)
    path = archive(stage, out, platform)
    print(f'{path} ({path.stat().st_size / 1e6:.1f} MB): nxrs {version}, engine {info.get("version")} '
          f'(contract {info["contract"]}){", llvm-mingw" if gnu and platform == "windows" else ""}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
