"""Fixed real-library Wonder rebuild, then one separately invoked guarded validation."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys
import time
from types import SimpleNamespace
import uuid

import build_supervisor as bs
import gpu_stop
import harness_job as hj
import library_play  # registers the normal Wonder route
import play_scenario as ps
import wonder_resume
import wonder_tools as wt

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = Path(r'C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-thermal-wonder')
HISTORICAL_SAVE = Path(r'C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-agent2\saves\wonder')
RELEASE_HASHES = {
    'nxrs.exe': '7a300ab37dc0a07be75f2e74f4e09ed72f00418a0c0ab2654cb73223fdf76b88',
    'nxrs-engine/nxrs-build.exe': '0a543e47e812805e4edff039831e1be99ff8c9bcae2765d2656dfbc7d9ef7cd1',
    'nxrs-engine/nxfront.lib': 'cc723594c7fa3ca62287203bf84e4bc1dd5a9d01c3672ce7c525e3994cdc90bb',
    'nxrs-engine/nxrt.lib': 'da51277f21f3d00c1009e6394b932dc1ddb74747a63fc97fad5edf55fe836220',
    'nxrs-engine/tools/dmsh_xlate.exe': '7a28092f4702c5b067e351cbe105ac4b23aed4b9bb1ce5bd09ccfacfb17dc30e',
    'nxrs-engine/tools/spirv-val.exe': 'bd7296d3f4f469642d65a1d135901912483e7e3b661f7fda36bc932729759c0f'}


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def save_state(path):
    if not path.exists():
        return dict(exists=False, files={}, directories=[])
    paths = [path, *path.rglob('*')]
    if any(p.is_symlink() or p.is_junction() for p in paths):
        raise RuntimeError('save reparse path requires review')
    return dict(exists=True, root_identity=path.stat().st_ino,
                files={str(p.relative_to(path)): dict(sha256=digest(p), bytes=p.stat().st_size,
                       identity=p.stat().st_ino, mtime_ns=p.stat().st_mtime_ns) for p in paths if p.is_file()},
                directories=sorted(str(p.relative_to(path)) for p in paths if p.is_dir()))


def locations(game):
    build = Path(game['buildDir'])
    return build / (game['outputName']+'.exe'), build / (game['outputName']+'_data')


def artifact_hashes(game):
    exe, _ = locations(game)
    return {str(p): digest(p) for p in (exe, exe.with_name(exe.stem+'_code.dll'))}


def verified_release():
    hashes = {name: digest(wt.RELEASE / name) for name in RELEASE_HASHES}
    if hashes != RELEASE_HASHES:
        raise RuntimeError('production release differs from reviewed candidate')
    return hashes


def build(folder):
    game = wt.entry()
    exe, data = locations(game)
    source = Path(game['source'])
    before_save = save_state(data / 'save')
    library_before = json.loads(wt.LIBRARY.read_text())
    if not source.is_file():
        raise RuntimeError('real library source is missing')
    run = bs.BuildRun(folder, seconds=1800)
    error = None
    try:
        run.observe(prelaunch=True)
        before = dict(game=game, release=verified_release(), artifacts=artifact_hashes(game),
                      save=before_save, source=dict(path=str(source), size=source.stat().st_size,
                                                   mtime_ns=source.stat().st_mtime_ns),
                      boot=bs.hm.powershell('(Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToString("o")'),
                      root_target_absent=not (ROOT / 'target').exists(),
                      shared_stop=digest(gpu_stop.SHARED_STOP),
                      pending_stop=digest(gpu_stop.pending_path(gpu_stop.SHARED_STOP)))
        bs.write_json(folder / 'before.json', before)
        bs.write_json(folder / 'library-before.json', library_before)
        shutil.copyfile(exe.with_suffix('.build.log'), folder / 'previous-build.log')
        clang = Path(r'C:\Program Files\LLVM\bin\clang.exe')
        if not clang.is_file():
            raise RuntimeError('installed MSVC-compatible clang is missing')
        run.env.update(NXRS_BUILD=str(ROOT / 'scripts/wonder_engine.cmd'),
                       NXRS_CLANG=str(ROOT / 'scripts/wonder_clang.cmd'), NXRS_TOOLCHAIN='msvc',
                       NXRS_ENGINE_LIB=str(wt.RELEASE / 'nxrs-engine/nxfront.lib'),
                       NXRS_RUNTIME_LIB=str(wt.RELEASE / 'nxrs-engine/nxrt.lib'),
                       NXSHADERS_XLATE=str(ROOT / 'scripts/wonder_xlate.cmd'),
                       NXSHADERS_SPIRV_VAL=str(ROOT / 'scripts/wonder_spirv.cmd'),
                       NXRS_WONDER_BUILD_JOB=run.job.name, NXRS_WONDER_EVIDENCE=str(folder),
                       NXRS_WONDER_DEADLINE=str(run.deadline), NXRS_WONDER_REAL_CLANG=str(clang),
                       NXRS_WONDER_TOOL_SLOT='Local\\nxrs-wonder-tool-'+uuid.uuid4().hex)
        run.execute('adapter-contract', [os.environ['COMSPEC'], '/d', '/c',
                    str(ROOT / 'scripts/wonder_engine.cmd'), 'contract'])
        if (folder / 'adapter-contract.log').read_text().strip() != 'nxrs-contract 2':
            raise RuntimeError('engine adapter did not forward the real contract')
        command = [str(wt.RELEASE / 'nxrs.exe'), '--library', str(wt.LIBRARY),
                   '--select-title', game['title'], '--build']
        bs.write_json(folder / 'command.json', dict(command=command, environment={k: v for k, v in
                      run.env.items() if k.startswith(('NXRS_', 'NXSHADERS_'))}, **run.status()))
        run.execute('production-build', command)
        after = wt.entry()
        library_after = json.loads(wt.LIBRARY.read_text())
        if after['buildIncomplete'] or after['builtSource'] != game['source']:
            raise RuntimeError('normal launcher build metadata does not report completion')
        if any(after[k] != game[k] for k in ('id', 'source', 'buildDir', 'outputName', 'workers')):
            raise RuntimeError('unexpected title configuration change')
        if [g for g in library_after['games'] if g['id'] != wt.TITLE] != [
                g for g in library_before['games'] if g['id'] != wt.TITLE]:
            raise RuntimeError('unrelated library entries changed')
        if library_after['jobs'] != library_before['jobs']:
            raise RuntimeError('library worker setting was changed')
        hashes = artifact_hashes(after)
        dll = str(exe.with_name(exe.stem+'_code.dll'))
        if hashes[dll] == before['artifacts'][dll]:
            raise RuntimeError('code DLL unchanged; relink with new release unproven')
        log = exe.with_suffix('.build.log').read_text(encoding='utf-8', errors='replace')
        if '1 at a time' not in log or 'Linking ' not in log or 'nxfront.lib' not in log:
            raise RuntimeError('single-worker/link provenance missing')
        shutil.copyfile(exe.with_suffix('.build.log'), folder / 'game-build.log')
        bs.write_json(folder / 'after.json', dict(game=after, artifacts=hashes, release=verified_release()))
        if digest(gpu_stop.SHARED_STOP) != before['shared_stop'] or digest(
                gpu_stop.pending_path(gpu_stop.SHARED_STOP)) != before['pending_stop']:
            raise RuntimeError('shared stops changed during build')
    except BaseException as exc:
        error = '%s: %s' % (type(exc).__name__, exc)
        run.job.kill()
        bs.write_json(folder / 'BUILD-STOP.json', dict(error=error, sample=run.sample))
        raise
    finally:
        cleanup = hj.CleanupTasks()
        cleanup.attempt('title build cleanup', run.close)
        if cleanup.errors:
            error = (error or '') + '; ' + '; '.join(cleanup.errors)
            bs.write_json(folder / 'BUILD-STOP.json', dict(error=error, sample=run.sample))
        after_save = save_state(data / 'save')
        bs.write_json(folder / 'save-audit.json', dict(before=before_save, after=after_save,
                                                     unchanged=before_save == after_save))
        bs.write_json(folder / 'result.json', dict(ok=error is None, error=error,
                      cleanup_confirmed=run.state.cleanup_confirmed, **run.status()))
        if after_save != before_save:
            raise RuntimeError('production save changed during build')
        cleanup.require()


def play(folder, lease):
    game = wt.entry()
    exe, data = locations(game)
    built = json.loads((EVIDENCE / 'build/after.json').read_text())
    if artifact_hashes(game) != built['artifacts']:
        raise RuntimeError('game artifacts changed since verified rebuild')
    verified_release()
    before = save_state(data / 'save')
    shadow = folder / 'shadow-save'
    seed = data / 'save' if before['exists'] else HISTORICAL_SAVE
    if not seed.is_dir():
        raise RuntimeError('reviewed external shadow save unavailable')
    save_state(seed)  # reject reparse paths before copying
    shutil.copytree(seed, shadow)
    bs.write_json(folder / 'save-before.json', dict(production=before, seed=str(seed), shadow=save_state(shadow)))
    sc = copy.deepcopy(ps.SCENARIOS['wonder_gameplay'])
    sc['save'] = False  # production SaveTransaction must never rename/restore the owner's save
    sc['steps'] = sc['steps'][:-2] + [('replay', 2, 'jump', 'wonder_gameplay.txt'),
                                    ('shot', 58, 'gameplay'), ('shot', 90, 'gameplay end')]
    sc['acceptance'].update(required_screenshot_names=['shot-course', 'replay-jump', 'shot-gameplay',
                                                     'shot-gameplay end'], require_non_silent_audio=True)
    bs.write_json(folder / 'scenario.json', dict(schema='nxrs.scenario.v1', scenario='wonder_gameplay',
                  steps=sc['steps'], acceptance=sc['acceptance'], save_protected=True,
                  shadow_save=str(shadow), production_save_read_only=True))
    options = SimpleNamespace(data=str(data), env=['NXRS_AUTO_CAPTURE=0', 'NXRS_LANE=thermal-wonder',
                              'NXOS_SAVE='+str(shadow), 'NXGFX_NDVE=off'], option=[], seconds=330,
                              capture_interval=0, diagnostics='baseline', window_value=(115, 260),
                              slow=1, presentmon='')
    try:
        with wonder_resume.WonderResumption(lease, folder) as scope:
            ps._scenario(options, sc, folder, exe, lease)
            result = json.loads((folder / 'result.json').read_text())
            scope.outcome = result
    finally:
        after = save_state(data / 'save')
        bs.write_json(folder / 'save-after.json', dict(production=after, unchanged=before == after,
                      shadow=save_state(shadow)))
        if before != after:
            raise RuntimeError('production save changed; no automatic restore authorized')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('phase', choices=('build', 'play'))
    args = parser.parse_args()
    if not EVIDENCE.parent.is_dir():
        raise SystemExit('evidence parent is missing')
    with hj.MachineLease() as lease:
        EVIDENCE.mkdir(exist_ok=True)
        folder = EVIDENCE / ('build' if args.phase == 'build' else 'run')
        folder.mkdir()  # no automatic retry / overwrite of any attempt
        if args.phase == 'build':
            build(folder)
        else:
            play(folder, lease)


if __name__ == '__main__':
    main()
