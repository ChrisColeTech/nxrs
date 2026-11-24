"""play_scenario.py: scripted gameplay runs with frame-time statistics (Turbo's tools/play_scenario.py and
plza_probe.py, on this engine's front end), always through scripts/run_game.py's guard (GAME_LOCK, free commit,
the memory watchdog, kills by PID only).

py scripts/play_scenario.py <scenario> --exe <game.exe> [--data <dir>] [--label L] [--option KEY=VALUE ...]
                            [--env NAME=VALUE ...] [--seconds N] [--window FROM,TO] [--slow X] [--presentmon EXE]
py scripts/play_scenario.py stats <run folder> [--window FROM,TO]

A run folder build/I/runs/<time>-<label>/ gets: run.log (the game's log), metrics-*.csv and frames-*.csv (metrics=1,
NXRS_METRICS=<run folder>), NNNN-<name>.png (screenshots at the game's resolution through the control pipe's
`fullres`, each with a black audit), periodic auto-*.png captures and diagnostics-*.txt snapshots, actions.jsonl,
replay.txt (the input replay the game watches, NXRS_INPUT_REPLAY), presentmon.csv (with --presentmon), result.json,
summary.json and artifact-manifest.json.

Per-run ini: every --option KEY=VALUE goes to the game as --KEY=VALUE (this run only; the ini is not written), plus
--fullscreen=0. The numbers over the window (seconds since start): FPS and frame-time p50 / p95 / p99 / longest from
the pacing lines ([vk] pacing:, which must come at least every 5 s: the largest gap is reported), from metrics and
from PresentMon when given. Scenarios marked `save=True` automatically retain before/after evidence and restore
<data>/save after the run.
"""
import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
import gamectl  # noqa: E402
import observability  # noqa: E402
import run_game  # noqa: E402
import run_triage  # noqa: E402

RUNS = ROOT / 'build/I/runs'
INPUTS = ROOT / 'scripts/inputs'
PRESENTMON = ROOT / 'build/tools/presentmon/PresentMon-2.6.0-x64.exe'   # Intel's (MIT), GitHub release v2.6.0
KEYS = dict(a='A', b='B', x='X', y='Y', lr='L+R', plus='Plus', up='Up', down='Down', left='Left', right='Right')


def acceptance(fps, window, screenshots, stutters=20, memory_slope=100, **extra):
    return dict(target_fps=fps, gameplay_window=list(window), max_stutters=stutters, allowed_gaps=5.5,
                memory_slope_threshold=memory_slope, max_red_errors=0,
                required_screenshot_names=screenshots, **extra)


# (action, settle seconds, observed[, replay file])
SCENARIOS = {
    'smoke': dict(seconds=60, window=(20, 50), steps=[('shot', 20, 'boot'), ('shot', 25, 'later')]),
    'dkb': dict(seconds=200, window=(100, 150), save=True,
        acceptance=acceptance(60, (100, 150), ['replay-gameplay', 'replay-punched', 'shot-end'], stutters=10,
                              require_non_silent_audio=True, image_thresholds={'screenshots': {'replay-gameplay': {
                                  'max_black_fraction': .9, 'max_dominant_purple_fraction': .25}}}), steps=[
        ('shot', 75, 'boot'), ('a', 6, 'title'), ('a', 25, 'Continue'), ('replay', 14, 'gameplay', 'dkb_walk.txt'),
        ('replay', 6, 'walked', 'dkb_punch.txt'), ('replay', 14, 'punched', 'dkb_walk.txt'), ('shot', 10, 'end')]),
    'mkw': dict(seconds=230, window=(150, 185), save=True,
        acceptance=acceptance(60, (150, 185), ['replay-race'], stutters=10, require_non_silent_audio=True,
                              image_thresholds={'screenshots': {'replay-race': {
                                  'max_black_fraction': .8, 'max_near_white_fraction': .8}}}), steps=[
        ('shot', 44, 'boot'), ('lr', 5, 'title')] + [('a', 6, 'menu')] * 8 + [
        ('replay', 42, 'race', 'mkw_drive.txt'), ('replay', 42, 'race', 'mkw_drive.txt')]),
    # MKW to a race the way mkw-recomp's tools/run/race.sh does (press-race.txt: A every 1.5 s from 12 s, the Grand
    # Prix of the save's last cup; A also accelerates), with the presses from the game's own input script: run with
    # --env NXRS_KEYS=12:A*400/1.5. A picture every 20 s (lane K)
    'mkw-race': dict(seconds=600, window=(300, 580), save=True,
        acceptance=acceptance(60, (300, 580), ['shot-race'], stutters=30, require_non_silent_audio=True,
                              image_thresholds={'screenshots': {'shot-race': {
                                  'max_black_fraction': .8, 'max_near_white_fraction': .8}}}),
        steps=[('shot', 20, 'race')] * 29),
    'plza': dict(seconds=150, window=(45, 84), save=True,
        acceptance=acceptance(60, (45, 84), ['shot-station', 'replay-walk', 'shot-walked'], stutters=12,
                              require_non_silent_audio=True), steps=[
        ('shot', 12, 'boot'), ('a', 30, 'title'), ('shot', 8, 'station'), ('replay', 10, 'walk', 'plza_walk.txt'),
        ('shot', 5, 'walked'), ('replay', 10, 'walk', 'plza_walk.txt'), ('shot', 5, 'walked')]),
    'zelda': dict(seconds=140, window=(45, 100), save=True,
        acceptance=acceptance(59, (45, 100), ['shot-title', 'shot-end'], stutters=0,
                              max_frame_ms=100, max_memory_peak_mb=16384, require_non_silent_audio=True,
                              image_thresholds={'screenshots': {'shot-end': {'max_black_fraction': .8}}}), steps=[
        ('shot', 40, 'boot'), ('shot', 25, 'title'), ('a', 15, 'menu'), ('a', 15, 'menu'), ('shot', 10, 'end')]),
    'mk8d': dict(seconds=240, window=(100, 140), save=True,
        acceptance=acceptance(60, (100, 140), ['replay-driving', 'shot-driving +20s'], stutters=12,
                              memory_slope=60, require_non_silent_audio=True,
                              image_thresholds={'screenshots': {'replay-driving': {'max_black_fraction': .8}}}), steps=[
        ('shot', 9, 'boot'), ('lr', 4, 'title'), ('right', 1.5, 'Select a Mii: off Create a Mii'), ('a', 3, 'first Mii'), ('a', 3, 'menu1'),
        ('a', 3, 'menu2'), ('a', 5, 'menu3'), ('a', 4, 'char'), ('a', 2, 'k1'), ('a', 2, 'k2'), ('a', 4, 'k3'), ('a', 4, 'cup'), ('a', 30, 'gp-ok'),
        ('replay', 20, 'driving', 'mkw_drive.txt'), ('shot', 18, 'driving +20s')]),
    # the mk8d steps, then a minute more of the race with a picture every 5 s (lane G3: the picture over a whole lap)
    'mk8d-lap': dict(seconds=230, window=(100, 190), save=True,
        acceptance=acceptance(60, (100, 190), ['replay-driving', 'shot-lap'], memory_slope=60,
                              require_non_silent_audio=True, image_thresholds={'screenshots': {'shot-lap': {
                                  'max_black_fraction': .8, 'max_near_white_fraction': .8}}}), steps=[
        ('shot', 9, 'boot'), ('lr', 4, 'title'), ('right', 1.5, 'Select a Mii: off Create a Mii'), ('a', 3, 'first Mii'), ('a', 3, 'menu1'),
        ('a', 3, 'menu2'), ('a', 5, 'menu3'), ('a', 4, 'char'), ('a', 2, 'k1'), ('a', 2, 'k2'), ('a', 4, 'k3'), ('a', 4, 'cup'), ('a', 30, 'gp-ok'),
        ('replay', 20, 'driving', 'mk8d_gp.txt')] + [('shot', 5, 'lap')] * 14),
}
PM = []
# 'fullres' (graphics: the game's picture at its resolution, at the next present) or 'shot' (the front end: the
# window as shown, now); NXRS_SHOT=shot picks the window, NXRS_SHOT=none takes none (lane W: a pending fullres can hang a present, INTERFACES)
SHOT_CMD = os.environ.get('NXRS_SHOT', 'fullres')
PACING = re.compile(r'^\[\s*([\d.]+)\] \[vk\] pacing: ([\d.]+) FPS \| p95 ([\d.]+) p99 ([\d.]+) max ([\d.]+) ms')


def save_manifest(path):
    """Stable relative-path to SHA-256 map for save evidence and restoration tests."""
    path = Path(path)
    if not path.is_dir():
        return {}
    return {p.relative_to(path).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(path.rglob('*')) if p.is_file()}


class SaveTransaction:
    """Snapshot a save, retain the run's writes as evidence, then restore the snapshot."""
    def __init__(self, save, run):
        self.save = Path(save)
        self.run = Path(run)
        self.before = self.run / 'save-before'
        self.after = self.run / 'save-after'
        self.existed = self.save.is_dir()
        self.before_manifest = save_manifest(self.save)

    def begin(self):
        if self.existed:
            shutil.copytree(self.save, self.before)
        return self

    def finish(self):
        changes = None
        evidence_error = None
        try:
            after_manifest = save_manifest(self.save)
            if self.save.is_dir():
                shutil.copytree(self.save, self.after)
            before_keys, after_keys = set(self.before_manifest), set(after_manifest)
            changes = {
                'save_path': str(self.save),
                'original_existed': self.existed,
                'changed': self.before_manifest != after_manifest,
                'added': sorted(after_keys - before_keys),
                'removed': sorted(before_keys - after_keys),
                'modified': sorted(k for k in before_keys & after_keys
                                   if self.before_manifest[k] != after_manifest[k]),
                'before': self.before_manifest,
                'after': after_manifest,
            }
            (self.run / 'save-changes.json').write_text(json.dumps(changes, indent=1), encoding='utf-8')
        except Exception as e:
            evidence_error = e

        # Restoration must not depend on successfully hashing or copying evidence.
        suffix = '.scenario-%d-%d' % (os.getpid(), time.time_ns())
        displaced = self.save.with_name(self.save.name + suffix + '-displaced')
        restored = self.save.with_name(self.save.name + suffix + '-restored')
        if self.existed:
            shutil.copytree(self.before, restored)
        if self.save.exists():
            self.save.rename(displaced)
        try:
            if self.existed:
                restored.rename(self.save)
        except Exception:
            if restored.exists():
                shutil.rmtree(restored)
            if displaced.exists():
                displaced.rename(self.save)
            raise
        if displaced.exists():
            shutil.rmtree(displaced)
        restored_manifest = save_manifest(self.save)
        audit = dict(restored=restored_manifest == self.before_manifest and self.save.is_dir() == self.existed,
                     original_existed=self.existed, before=self.before_manifest, restored_manifest=restored_manifest)
        (self.run / 'save-restoration.json').write_text(json.dumps(audit, indent=1), encoding='utf-8')
        if not audit['restored']:
            raise RuntimeError('save restoration verification failed: %s' % self.save)
        if evidence_error:
            raise evidence_error
        return changes


def png_pixels(path):
    """(w, h, rows of RGB tuples) of an 8-bit RGB / RGBA PNG; None otherwise."""
    return observability.png_pixels(path)


def black_audit(path):
    """Near-black fraction (every channel < 16) and the fully black cells of an 8x8 grid (Turbo's audit)."""
    audit = observability.image_diagnostics(path)
    if audit.get('status') != 'ok':
        return dict(audit_error=audit.get('error', 'PNG audit failed'))
    return dict(black_fraction=round(audit['black_fraction'], 4), black_cells=audit['black_cells'])


def capture_milestone(action, observed):
    """Return the evidence milestone for steps that intentionally capture a frame."""
    return '%s-%s' % (action, observed) if action in ('shot', 'replay') else None


def pct(v, q):
    return v[min(len(v) - 1, int(len(v) * q))] if v else 0.0


def stats(run, window):
    run = Path(run)
    lo, hi = window
    out = dict(run=str(run), window=[lo, hi])
    log = run / 'run.log'
    if log.exists():
        pac = []
        for line in log.read_text(errors='replace').splitlines():
            m = PACING.match(line)
            if m:
                pac.append(tuple(float(x) for x in m.groups()))
        gaps = [b[0] - a[0] for a, b in zip(pac, pac[1:])]
        win = [p for p in pac if lo <= p[0] <= hi]
        out.update(pacing_lines=len(pac), pacing_gap_max_s=round(max(gaps), 2) if gaps else None, pacing_every_5s=bool(gaps) and max(gaps) <= 5.5)
        if win:
            out.update(fps_pacing=round(sum(p[1] for p in win) / len(win), 2), p95_ms=max(p[2] for p in win), p99_ms=max(p[3] for p in win), max_ms=max(p[4] for p in win))
    for mt in run.glob('metrics-*.csv'):
        fps = []
        for line in mt.read_text().splitlines():
            if line.startswith('#'):
                continue
            f = line.split(',')
            try:
                t = float(f[0])
            except ValueError:
                continue
            if lo <= t <= hi:
                fps.append(float(f[2]))
        if fps:
            out.update(fps_metrics=round(sum(fps) / len(fps), 2), seconds=len(fps), seconds_58=sum(1 for x in fps if x >= 58), fps_min=min(fps))
    pm = run / 'presentmon.csv'
    if pm.exists():
        lines = pm.read_text(errors='replace').splitlines()
        if lines:
            head = lines[0].split(',')
            col = next((head.index(c) for c in ('MsBetweenPresents', 'FrameTime', 'msBetweenPresents') if c in head), None)
            tcol = next((head.index(c) for c in ('TimeInSeconds', 'CPUStartTime') if c in head), None)
            if col is not None:
                iv = []
                for l in lines[1:]:
                    f = l.split(',')
                    try:
                        t = float(f[tcol]) if tcol is not None else 0
                        if tcol is None or lo <= t <= hi:
                            iv.append(float(f[col]))
                    except (ValueError, IndexError):
                        pass
                iv.sort()
                if iv:
                    out.update(presentmon_frames=len(iv), presentmon_fps=round(1000 * len(iv) / sum(iv), 2), presentmon_p50=round(pct(iv, .5), 2),
                               presentmon_p95=round(pct(iv, .95), 2), presentmon_p99=round(pct(iv, .99), 2), presentmon_longest=round(iv[-1], 1))
    shots = []
    ac = run / 'actions.jsonl'
    if ac.exists():
        for line in ac.read_text(encoding='utf-8').splitlines():
            j = json.loads(line)
            if 'screenshot' in j:
                shots.append((j['screenshot'], j.get('black_fraction'), j.get('black_cells')))
    out['screenshots'] = shots
    rs = run / 'result.json'
    if rs.exists():
        out['result'] = json.loads(rs.read_text())
    return out


def scenario(a, sc, run, exe):
    # The same lease spans save inspection, backup, callbacks, cleanup and restoration.
    # run() borrows it explicitly; standalone run() still acquires its own lease.
    with run_game.harness_job.MachineLease() as lease:
        return _scenario(a, sc, run, exe, lease)


def _scenario(a, sc, run, exe, lease):
    t0 = time.monotonic()
    count = [0]

    def record(**f):
        f['elapsed'] = round(time.monotonic() - t0, 2)
        with (run / 'actions.jsonl').open('a', encoding='utf-8') as fh:
            fh.write(json.dumps(f) + '\n')

    def steps(pid):
        stop = run_game.CURRENT['stop']
        def settle_wait(seconds):
            if stop.wait(seconds):
                raise OSError('refused: run stopped')

        def shot(name):
            if SHOT_CMD == 'none':
                return
            count[0] += 1
            p = run / ('%04d-%s.png' % (count[0], re.sub(r'[^A-Za-z0-9]+', '_', name)[:40]))
            try:
                gamectl.pipe(run_game.current(pid), '%s %s' % (SHOT_CMD, p), timeout=20)
            except OSError as e:
                record(screenshot=None, name=name, note='control pipe: %s' % e)
                if str(e).startswith('refused'):
                    raise   # not our game any more (it exited, or the lock is another lane's): the scenario stops
                return
            for _ in range(100):
                if p.exists() and p.stat().st_size > 0 and p.read_bytes()[-8:-4] == b'IEND':
                    record(screenshot=p.name, name=name, **black_audit(p))
                    return
                settle_wait(0.1)
            record(screenshot=None, name=name, note='no present within 10 s')

        try:
            if a.presentmon and Path(a.presentmon).is_file():
                # unelevated, --terminate_on_proc_exit does not fire: the harness stops it by its PID after the run
                with (run / 'presentmon.log').open('wb') as output:
                    PM.append(run_game.CURRENT['job'].launch(
                        [a.presentmon, '--process_id', str(pid), '--output_file', str(run / 'presentmon.csv'),
                         '--v1_metrics', '--no_console_stats', '--session_name', 'nxrs_%d' % pid,
                         '--terminate_on_proc_exit'], stdout=output))
            for st in sc['steps']:
                settle_wait(0)
                action, settle, observed = st[0], st[1] * a.slow, st[2]
                record(action=action, observed_before=observed)
                if action == 'replay':
                    shutil.copyfile(INPUTS / st[3], run / 'replay.txt')
                elif action == 'probe':
                    reply = gamectl.pipe(run_game.current(pid), st[3], timeout=20)
                    passed = bool(re.search(st[4], reply, re.DOTALL))
                    record(probe=st[3], assertion=st[4], passed=passed, reply=reply)
                    if not passed:
                        if sc.get('capture_probe_failures'):
                            shot('probe-failed-%s' % observed)
                        raise AssertionError('%s did not match %r' % (st[3], st[4]))
                    settle_wait(settle)
                    continue
                elif action == 'ctl':
                    # ('ctl', settle, name, command): a control-pipe command, its reply recorded (threads, peek, wtrace...)
                    record(ctl_cmd=st[3], reply=gamectl.pipe(run_game.current(pid), st[3], timeout=20))
                    settle_wait(settle)
                    continue
                elif action in KEYS:
                    record(ctl=gamectl.pipe(run_game.current(pid), 'press %s~0.2' % KEYS[action]).strip())
                settle_wait(settle)
                milestone = capture_milestone(action, observed)
                if milestone:
                    shot(milestone)
            run_game.diagnostic_snapshot(pid, run, 'final', capture=bool(sc.get('capture_final')))
            record(ctl=gamectl.pipe(run_game.current(pid), 'quit').strip())
        except Exception as e:
            if stop.is_set():
                return
            stop.set()
            run_game.CURRENT['job'].kill()
            record(error='%s: %s' % (type(e).__name__, e))
    env = dict(os.environ, NXRS_METRICS=str(run), NXRS_ARTIFACT_DIR=str(run),
               NXRS_INPUT_REPLAY=str(run / 'replay.txt'), NXRS_CAPTURE_INTERVAL_S=str(a.capture_interval),
               NXRS_CONTROLLER_BACKGROUND='1', NXRS_KEEP_FOCUS='1', NXRS_NO_ACTIVATE='1')
    if getattr(a, 'diagnostics', 'baseline') in ('pipeline', 'render'):
        env.update(NXGFX_PIPE_LOG='1', NXGFX_WAIT_DIAG='1')
    if getattr(a, 'diagnostics', 'baseline') == 'render':
        env['NXGFX_PROFILE'] = '1'
    env.update(dict(x.split('=', 1) for x in a.env))
    args = ([a.data] if a.data else []) + ['--fullscreen=0', '--metrics=1'] + ['--' + o for o in a.option]
    # A previous unconfirmed cleanup must block even save inspection/backup, not just launch.
    run_game.gpu_stop.require_clear(run_game.gpu_stop.paths(env))
    save_tx = None
    state = run_game.harness_lifecycle.RunState()
    if sc.get('save'):
        data = Path(a.data).resolve() if a.data else exe.parent / (exe.stem + '_data')
        save_tx = SaveTransaction(data / 'save', run).begin()
    try:
        r = run_game.run(str(exe), str(run / 'run.log'), a.seconds or sc['seconds'], args,
                         env=env, during=steps, say=lambda s: print(s, flush=True), lease=lease, state=state)
    finally:
        helpers = run_game.harness_job.CleanupTasks()
        try:
            for pm in PM:
                try:
                    helpers.attempt('PresentMon wait', lambda pm=pm: pm.wait(timeout=2))
                finally:
                    helpers.attempt('PresentMon handle close', pm.close)
        finally:
            PM.clear()
            if helpers.errors:
                state.cleanup_confirmed = False
            helpers.require()
            if state.launched and not state.cleanup_confirmed:
                raise run_game.harness_job.CleanupError(
                    'save recovery required: positive process/helper/callback cleanup proof missing')
            if save_tx and state.launched and state.cleanup_confirmed:
                save_tx.finish()
            # No launch means no save replacement. Uncertain cleanup retains save-before.
    (run / 'result.json').write_text(json.dumps(r, indent=1))
    try:
        observability.generate(run, r, a.window_value)
    except Exception as e:
        print('diagnostic summary failed: %s: %s' % (type(e).__name__, e), flush=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('scenario', choices=[*SCENARIOS, 'stats'])
    ap.add_argument('run', nargs='?', help='with stats: the run folder')
    ap.add_argument('--exe')
    ap.add_argument('--data')
    ap.add_argument('--label')
    ap.add_argument('--env', action='append', default=[])
    ap.add_argument('--option', action='append', default=[], help='KEY=VALUE ini option for this run, e.g. docked=1')
    ap.add_argument('--window', help='seconds from,to for the numbers')
    ap.add_argument('--seconds', type=int)
    ap.add_argument('--slow', type=float, default=1, help='every wait times this')
    ap.add_argument('--presentmon', default=str(PRESENTMON), help='PresentMon.exe (default build/tools/presentmon, when there; "" for none): presentmon.csv for the game process')
    ap.add_argument('--capture-interval', type=float, default=float(os.environ.get('NXRS_CAPTURE_INTERVAL_S', '30')),
                    help='seconds between automatic full-frame captures and diagnostic snapshots (0 disables)')
    ap.add_argument('--diagnostics', choices=['baseline', 'pipeline', 'render'], default='baseline',
                    help='pipeline: compile brackets/wait ledger; render: also detailed CPU profiling (timing overhead)')
    a = ap.parse_args()
    win = tuple(float(x) for x in a.window.split(',')) if a.window else None
    a.window_value = win
    if a.scenario == 'stats':
        print(json.dumps(stats(a.run, win or (0, 1e9)), indent=1))
        return
    if not a.exe:
        ap.error('--exe <game.exe> is required')
    exe = Path(a.exe).resolve()
    if a.data:
        a.data = str(Path(a.data).resolve())
    sc = SCENARIOS[a.scenario]
    a.window_value = win or sc['window']
    RUNS.mkdir(parents=True, exist_ok=True)
    run = RUNS / (time.strftime('%Y%m%d-%H%M%S-') + (a.label or a.scenario))
    run.mkdir()
    (run / 'scenario.json').write_text(json.dumps({
        'schema': 'nxrs.scenario.v1',
        'scenario': a.scenario,
        'acceptance': sc.get('acceptance'),
        'save_protected': bool(sc.get('save')),
        'diagnostics': a.diagnostics,
        'steps': sc['steps'],
        'slow': a.slow,
        'input_files': [run_triage.fingerprint(INPUTS / name)
                        for name in sorted({st[3] for st in sc['steps'] if st[0] == 'replay'})],
    }, indent=1), encoding='utf-8')
    print(run, flush=True)
    try:
        scenario(a, sc, run, exe)
    except SystemExit as e:
        print('not started: %s' % e)
        sys.exit(1)
    print(json.dumps(stats(run, win or sc['window']), indent=1))


if __name__ == '__main__':
    main()
