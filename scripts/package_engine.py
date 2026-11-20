#!/usr/bin/env python3
"""package_engine.py: the nxrs engine bundle, the folder the launcher ships as nxrs-engine/ (its package-release.py
--nxrs-engine makes the same layout), as one archive.

    py -I scripts/package_engine.py --target <cargo release dir> [--gnu <gnullvm release dir>] --xlate <dmsh_xlate>
                                    --spirv-val <spirv-val> [--out dist] [--platform linux|windows]

nxrs-engine/
  nxrs-build(.exe)              the CLI the launcher runs (contract v2)
  engine.json                   {"contract", "engine": "nxrs", "version", "platform"} (+ "requires" on Linux)
  libnxfront.a, libnxrt.a       Linux: the engine and the runtime a game's <name>_code.so links (x86-64)
  nxfront.lib, nxrt.lib         Windows: the MSVC engine; gnu/ the gnullvm one (llvm-mingw ships beside, by the launcher)
  tools/dmsh_xlate(.exe)        the shader translator nxrs-build runs during a build
  tools/spirv-val(.exe)         the SPIR-V validator it runs over the translated shaders
  LICENSES/                     the engine's third-party licenses
Output: <out>/nxrs-engine-<platform>-x64.tar.gz (Linux) or .zip (Windows), and a .sha256 beside it. Only x86-64: the
runtime's assembly boundary (guest calls, fibers, setjmp) is x86-64 code.
"""
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# what a Linux machine needs installed to build and run games (nxrs-build doctor checks each)
LINUX_REQUIRES = ['clang', 'lld', 'libvulkan1 + a Vulkan 1.3 driver', 'libasound2', 'libopus0', 'libvpx', 'libdav1d',
                  'libavcodec', 'libavutil', 'optional: libsdl2-2.0-0 (pads), zenity or kdialog (dialogs), ffmpeg (F7)']


def arg(name, default=None):
    a = sys.argv
    return a[a.index(name) + 1] if name in a else default


def version():
    head = subprocess.run(['git', '-C', str(ROOT), 'rev-parse', '--short=12', 'HEAD'], capture_output=True, text=True).stdout.strip()
    dirty = subprocess.run(['git', '-C', str(ROOT), 'status', '--porcelain', '--untracked-files=no'], capture_output=True, text=True).stdout.strip()
    return (head or 'unknown') + ('+local' if dirty else '')


def put(src, dst, executable=False):
    src = Path(src)
    if not src.is_file() or not src.stat().st_size:
        sys.exit(f'missing: {src}')
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dst)
    if executable:
        dst.chmod(dst.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def main():
    platform = arg('--platform', 'windows' if os.name == 'nt' else 'linux')
    exe = '.exe' if platform == 'windows' else ''
    target = Path(arg('--target', ROOT / 'target/release'))
    out = Path(arg('--out', ROOT / 'dist'))
    xlate, spirv = arg('--xlate'), arg('--spirv-val')
    if not xlate or not spirv:
        sys.exit('--xlate <dmsh_xlate> and --spirv-val <spirv-val> are required (a build translates and validates shaders)')
    stage = out / 'stage'
    shutil.rmtree(stage, ignore_errors=True)
    eng = stage / 'nxrs-engine'
    put(target / f'nxrs-build{exe}', eng / f'nxrs-build{exe}', True)
    if platform == 'linux':
        for name in ('libnxfront.a', 'libnxrt.a'):
            put(target / name, eng / name)
    else:
        for name in ('nxfront.lib', 'nxrt.lib'):
            put(target / name, eng / name)
        gnu = arg('--gnu')
        if gnu:
            for name in ('libnxfront.a', 'libnxrt.a'):
                put(Path(gnu) / name, eng / 'gnu' / name)
            # the windows crate's import library the gnullvm engine links (windows_x86_64_gnullvm's lib/)
            put(arg('--windows-lib', ROOT / 'build/tools/gnu-libs/libwindows.0.52.0.a'), eng / 'gnu/libwindows.0.52.0.a')
    put(xlate, eng / 'tools' / f'dmsh_xlate{exe}', True)
    put(spirv, eng / 'tools' / f'spirv-val{exe}', True)
    shutil.copytree(ROOT / 'LICENSES', eng / 'LICENSES')
    # the contract the CLI answers, asked of the copy in the bundle (it runs on this platform)
    answer = subprocess.run([str(eng / f'nxrs-build{exe}'), 'contract'], capture_output=True, text=True, timeout=60).stdout
    m = re.search(r'nxrs-contract (\d+)', answer)
    if not m:
        sys.exit('nxrs-build does not answer "contract": ' + answer.strip())
    info = {'contract': int(m.group(1)), 'engine': 'nxrs', 'version': version(), 'platform': f'{platform}-x64'}
    if platform == 'linux':
        info['requires'] = LINUX_REQUIRES
    (eng / 'engine.json').write_text(json.dumps(info, indent=2) + '\n')
    name = f'nxrs-engine-{platform}-x64'
    out.mkdir(parents=True, exist_ok=True)
    if platform == 'linux':
        archive = out / f'{name}.tar.gz'
        with tarfile.open(archive, 'w:gz') as t:
            t.add(eng, arcname='nxrs-engine')
    else:
        archive = out / f'{name}.zip'
        with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as z:
            for p in sorted(eng.rglob('*')):
                if p.is_file():
                    z.write(p, Path('nxrs-engine') / p.relative_to(eng))
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (out / f'{archive.name}.sha256').write_text(f'{digest}  {archive.name}\n')
    files = sorted(str(p.relative_to(stage)).replace('\\', '/') for p in eng.rglob('*') if p.is_file() and 'LICENSES' not in p.parts)
    print(f'{archive} ({archive.stat().st_size / 1e6:.1f} MB, contract {info["contract"]}, {info["version"]}):')
    for f in files:
        print('  ' + f)
    return 0


if __name__ == '__main__':
    sys.exit(main())
