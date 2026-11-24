"""CPU-only prerequisite/source/release verification, run inside the watched build job."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
ARTIFACTS = {'nxrs.exe': 'nxrs.exe', 'nxrs-build.exe': 'nxrs-engine/nxrs-build.exe',
             'nxfront.lib': 'nxrs-engine/nxfront.lib', 'nxrt.lib': 'nxrs-engine/nxrt.lib'}


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def command(args):
    return subprocess.check_output(args, cwd=ROOT, timeout=10).decode('utf-8').strip()


def sources():
    paths = command(['git', '-C', str(ROOT), 'ls-files', '-z', '--cached', '--others',
                     '--exclude-standard', '--', 'src', 'Cargo.toml', 'Cargo.lock', '.cargo',
                     'scripts/build', 'scripts/build_supervisor.py', 'scripts/build_provenance.py'])
    return {p: digest(ROOT / p) for p in sorted(set(paths.split('\0'))) if p}


def release_hashes():
    return {name: digest(ROOT / 'dist/nxrs' / path) for name, path in ARTIFACTS.items()}


def stops():
    paths = [ROOT / 'build/GPU-STOP.json', ROOT / 'build/GPU-STOP.json.pending']
    if os.environ.get('NXRS_GPU_STOP_PATH'):
        path = Path(os.environ['NXRS_GPU_STOP_PATH'])
        paths.extend([path, path.with_name(path.name + '.pending')])
    return {str(path): digest(path) if path.exists() else None for path in paths}


def preflight():
    # A reused target/ is recorded, not refused: provenance rests on the source hashes compared at verify
    # and on Cargo's own fingerprints.
    target_absent = not (ROOT / 'target').exists()
    sdk = Path(os.environ.get('VULKAN_SDK', 'missing-sdk'))
    needed = [ROOT / 'build/tools/dmsh_xlate/dmsh_xlate.exe', sdk / 'Bin/spirv-val.exe',
              Path(os.environ.get('NXGFX_GLSLANG', str(sdk / 'Bin/glslangValidator.exe')))]
    needed += [ROOT / 'src/os/third_party' / path for path in
               ('opus/libopus.a', 'vpx/libvpx.a', 'dav1d/libdav1d.a')]
    for path in needed:
        if not path.is_file():
            raise RuntimeError('missing prerequisite: ' + str(path))
    for tool in ('cargo', 'rustc', 'pwsh', 'git', 'nvidia-smi'):
        if not shutil.which(tool):
            raise RuntimeError('missing tool: ' + tool)
    free = {drive: shutil.disk_usage(drive).free for drive in ('C:\\', 'D:\\')}
    if free['C:\\'] < 10 * 2**30 or free['D:\\'] < 50 * 2**30:
        raise RuntimeError('insufficient preflight disk space: ' + str(free))
    # Metadata only: catch unsupported configured rustc options before cargo starts.
    rust_flags_check = command(['rustc', '--print', 'cfg', *os.environ.get('RUSTFLAGS', '').split()])
    return dict(head=command(['git', '-C', str(ROOT), 'rev-parse', 'HEAD']),
                dirty_status=command(['git', '-C', str(ROOT), 'status', '--short']),
                source_sha256=sources(), existing_release=release_hashes(), stops=stops(),
                target_absent=target_absent, free_bytes=free,
                rust_flags_check=rust_flags_check,
                rustc=command(['rustc', '-Vv']), cargo=command(['cargo', '-V']),
                prerequisite_sha256={str(p): digest(p) for p in needed})


def verify(folder):
    before = json.loads((folder / 'preflight.json').read_text(encoding='utf-8'))
    if sources() != before['source_sha256']:
        raise RuntimeError('build input files changed during build; provenance unconfirmed')
    if stops() != before['stops']:
        raise RuntimeError('existing game/GPU stop markers changed during build')
    journal = json.loads((folder / 'publication.json').read_text(encoding='utf-8-sig'))
    actual = release_hashes()
    if actual != {k: v.lower() for k, v in journal['hashes'].items()}:
        raise RuntimeError('deployed bytes differ from freshly built/staged artifact hashes')
    release = ROOT / 'dist/nxrs'
    for item in ('nxrs-engine/tools/dmsh_xlate.exe', 'nxrs-engine/tools/spirv-val.exe'):
        if not (release / item).is_file():
            raise RuntimeError('missing packaged tool: ' + item)
    info = json.loads((release / 'nxrs-engine/engine.json').read_text(encoding='utf-8-sig'))
    if info['contract'] != 2 or journal['contract'] != 'nxrs-contract 2':
        raise RuntimeError('contract 2 not verified')
    if (release / 'nxrs-engine/gnu').exists():
        raise RuntimeError('unrebuilt optional GNU artifacts must not be deployed')
    if (ROOT / 'target').exists():
        raise RuntimeError('default lean target cleanup incomplete')
    return dict(artifact_sha256=actual, contract=info['contract'], source_unchanged=True,
                target_removed=True, game_stop_markers_unchanged=True,
                previous_release=journal['previous'])


def audit_stopped(folder):
    before = json.loads((folder / 'preflight.json').read_text(encoding='utf-8'))
    actual = release_hashes()
    markers = stops()
    if actual != before['existing_release'] or markers != before['stops']:
        raise RuntimeError('existing release or game/GPU stop markers changed after stopped build')
    return dict(existing_release_unchanged=True, artifact_sha256=actual,
                game_stop_markers_unchanged=True, stop_sha256=markers,
                publication_attempted=(folder / 'publication.json').exists(),
                target_present=(ROOT / 'target').exists(),
                completed_target_artifacts={name: (ROOT / 'target/release' / name).is_file()
                                            for name in ARTIFACTS})


if __name__ == '__main__':
    mode, folder = sys.argv[1], Path(sys.argv[2])
    result = {'preflight': preflight, 'verify': lambda: verify(folder),
              'audit-stopped': lambda: audit_stopped(folder)}[mode]()
    (folder / (mode + '.json')).write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps({k: v for k, v in result.items() if k not in ('source_sha256', 'dirty_status')},
                     indent=2), flush=True)
