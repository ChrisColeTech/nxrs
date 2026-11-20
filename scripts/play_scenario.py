"""play_scenario.py: scripted gameplay runs with frame-time statistics (Turbo's tools/play_scenario.py and
plza_probe.py, on this engine's front end), always through scripts/run_game.py's guard (GAME_LOCK, free commit,
the memory watchdog, kills by PID only).

py scripts/play_scenario.py <scenario> --exe <game.exe> [--data <dir>] [--label L] [--option KEY=VALUE ...]
                            [--env NAME=VALUE ...] [--seconds N] [--window FROM,TO] [--slow X] [--presentmon EXE]
py scripts/play_scenario.py stats <run folder> [--window FROM,TO]

A run folder build/I/runs/<time>-<label>/ gets: run.log (the game's log), metrics-*.csv and frames-*.csv (metrics=1,
NXRS_METRICS=<run folder>), NNNN-<name>.png (screenshots at the game's resolution through the control pipe's
`fullres`, each with a black audit), actions.jsonl, replay.txt (the input replay the game watches,
NXRS_INPUT_REPLAY), presentmon.csv (with --presentmon), result.json.

Per-run ini: every --option KEY=VALUE goes to the game as --KEY=VALUE (this run only; the ini is not written), plus
--fullscreen=0. The numbers over the window (seconds since start): FPS and frame-time p50 / p95 / p99 / longest from
the pacing lines ([vk] pacing:, which must come at least every 5 s: the largest gap is reported), from metrics and
from PresentMon when given. Scenarios: smoke (boot, two screenshots), mk8d, dkb, mkw, plza, zelda (Turbo's).
The game's save is not touched here: copy <data>/save aside first.
"""
import argparse
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import time
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
import gamectl  # noqa: E402
import run_game  # noqa: E402

RUNS = ROOT / 'build/I/runs'
INPUTS = ROOT / 'scripts/inputs'
PRESENTMON = ROOT / 'build/tools/presentmon/PresentMon-2.6.0-x64.exe'   # Intel's (MIT), GitHub release v2.6.0
KEYS = dict(a='A', b='B', x='X', y='Y', lr='L+R', plus='Plus', up='Up', down='Down', left='Left', right='Right')
# (action, settle seconds, observed[, replay file])
SCENARIOS = {
    'smoke': dict(seconds=60, window=(20, 50), steps=[('shot', 20, 'boot'), ('shot', 25, 'later')]),
    'dkb': dict(seconds=200, window=(100, 150), steps=[
        ('shot', 75, 'boot'), ('a', 6, 'title'), ('a', 25, 'Continue'), ('replay', 14, 'gameplay', 'dkb_walk.txt'),
        ('replay', 6, 'walked', 'dkb_punch.txt'), ('replay', 14, 'punched', 'dkb_walk.txt'), ('shot', 10, 'end')]),
    'mkw': dict(seconds=230, window=(150, 185), steps=[
        ('shot', 44, 'boot'), ('lr', 5, 'title')] + [('a', 6, 'menu')] * 8 + [
        ('replay', 42, 'race', 'mkw_drive.txt'), ('replay', 42, 'race', 'mkw_drive.txt')]),
    # MKW to a race the way mkw-recomp's tools/run/race.sh does (press-race.txt: A every 1.5 s from 12 s, the Grand
    # Prix of the save's last cup; A also accelerates), with the presses from the game's own input script: run with
    # --env NXRS_KEYS=12:A*400/1.5. A picture every 20 s (lane K)
    'mkw-race': dict(seconds=600, window=(300, 580), steps=[('shot', 20, 'race')] * 29),
    'plza': dict(seconds=150, window=(45, 84), steps=[
        ('shot', 12, 'boot'), ('a', 30, 'title'), ('shot', 8, 'station'), ('replay', 10, 'walk', 'plza_walk.txt'),
        ('shot', 5, 'walked'), ('replay', 10, 'walk', 'plza_walk.txt'), ('shot', 5, 'walked')]),
    'zelda': dict(seconds=140, window=(45, 100), steps=[
        ('shot', 40, 'boot'), ('shot', 25, 'title'), ('a', 15, 'menu'), ('a', 15, 'menu'), ('shot', 10, 'end')]),
    'mk8d': dict(seconds=240, window=(100, 140), steps=[
        ('shot', 9, 'boot'), ('lr', 4, 'title'), ('right', 1.5, 'Select a Mii: off Create a Mii'), ('a', 3, 'first Mii'), ('a', 3, 'menu1'),
        ('a', 3, 'menu2'), ('a', 5, 'menu3'), ('a', 4, 'char'), ('a', 2, 'k1'), ('a', 2, 'k2'), ('a', 4, 'k3'), ('a', 4, 'cup'), ('a', 30, 'gp-ok'),
        ('replay', 20, 'driving', 'mkw_drive.txt'), ('shot', 18, 'driving +20s')]),
    # the mk8d steps, then a minute more of the race with a picture every 5 s (lane G3: the picture over a whole lap)
    'mk8d-lap': dict(seconds=230, window=(100, 190), steps=[
        ('shot', 9, 'boot'), ('lr', 4, 'title'), ('right', 1.5, 'Select a Mii: off Create a Mii'), ('a', 3, 'first Mii'), ('a', 3, 'menu1'),
        ('a', 3, 'menu2'), ('a', 5, 'menu3'), ('a', 4, 'char'), ('a', 2, 'k1'), ('a', 2, 'k2'), ('a', 4, 'k3'), ('a', 4, 'cup'), ('a', 30, 'gp-ok'),
        ('replay', 20, 'driving', 'mk8d_gp.txt')] + [('shot', 5, 'lap')] * 14),
}
PM = []
# 'fullres' (graphics: the game's picture at its resolution, at the next present) or 'shot' (the front end: the
# window as shown, now); NXRS_SHOT=shot picks the window, NXRS_SHOT=none takes none (lane W: a pending fullres can hang a present, INTERFACES)
SHOT_CMD = os.environ.get('NXRS_SHOT', 'fullres')
PACING = re.compile(r'^\[\s*([\d.]+)\] \[vk\] pacing: ([\d.]+) FPS \| p95 ([\d.]+) p99 ([\d.]+) max ([\d.]+) ms')


def png_pixels(path):
    """(w, h, rows of RGB tuples) of an 8-bit RGB / RGBA PNG; None otherwise."""
    d = Path(path).read_bytes()
    if d[:8] != b'\x89PNG\r\n\x1a\n':
        return None
    i, idat, w = 8, b'', 0
    while i < len(d):
        n, t = struct.unpack('>I4s', d[i:i + 8])
        c = d[i + 8:i + 8 + n]
        if t == b'IHDR':
            w, h, depth, ctype = struct.unpack('>IIBB', c[:10])
            if depth != 8 or ctype not in (2, 6):
                return None
            bpp = 3 if ctype == 2 else 4
        elif t == b'IDAT':
            idat += c
        i += 12 + n
    raw = zlib.decompress(idat)
    stride = w * bpp
    prev = bytearray(stride)
    rows = []
    o = 0
    for _ in range(h):
        f = raw[o]
        line = bytearray(raw[o + 1:o + 1 + stride])
        o += 1 + stride
        for x in range(stride):
            a = line[x - bpp] if x >= bpp else 0
            b = prev[x]
            c = prev[x - bpp] if x >= bpp else 0
            if f == 1:
                line[x] = (line[x] + a) & 255
            elif f == 2:
                line[x] = (line[x] + b) & 255
            elif f == 3:
                line[x] = (line[x] + (a + b) // 2) & 255
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        rows.append(bytes(line))
        prev = line
    return w, h, bpp, rows


def black_audit(path):
    """Near-black fraction (every channel < 16) and the fully black cells of an 8x8 grid (Turbo's audit)."""
    try:
        r = png_pixels(path)
    except Exception as e:
        return dict(audit_error=str(e))
    if not r:
        return dict(audit_error='not an 8-bit RGB(A) PNG')
    w, h, bpp, rows = r
    step = max(1, min(w, h) // 256)
    black = total = 0
    cells = [[0, 0] for _ in range(64)]
    for y in range(0, h, step):
        row = rows[y]
        for x in range(0, w, step):
            p = row[x * bpp:x * bpp + 3]
            dark = p[0] < 16 and p[1] < 16 and p[2] < 16
            black += dark
            total += 1
            c = cells[(y * 8 // h) * 8 + x * 8 // w]
            c[0] += dark
            c[1] += 1
    return dict(black_fraction=round(black / max(total, 1), 4), black_cells=sum(1 for b, t in cells if t and b == t))


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
    t0 = time.time()
    count = [0]

    def record(**f):
        f['elapsed'] = round(time.time() - t0, 2)
        with (run / 'actions.jsonl').open('a', encoding='utf-8') as fh:
            fh.write(json.dumps(f) + '\n')

    def steps(pid):
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
                time.sleep(0.1)
            record(screenshot=None, name=name, note='no present within 10 s')

        try:
            if a.presentmon and Path(a.presentmon).is_file():
                # unelevated, --terminate_on_proc_exit does not fire: the harness stops it by its PID after the run
                PM.append(subprocess.Popen([a.presentmon, '--process_id', str(pid), '--output_file', str(run / 'presentmon.csv'), '--v1_metrics', '--no_console_stats',
                                  '--session_name', 'nxrs_%d' % pid, '--stop_existing_session', '--terminate_on_proc_exit'],
                                 stdout=(run / 'presentmon.log').open('wb'), stderr=subprocess.STDOUT))
            for st in sc['steps']:
                action, settle, observed = st[0], st[1] * a.slow, st[2]
                record(action=action, observed_before=observed)
                if action == 'replay':
                    shutil.copyfile(INPUTS / st[3], run / 'replay.txt')
                elif action == 'ctl':
                    # ('ctl', settle, name, command): a control-pipe command, its reply recorded (threads, peek, wtrace...)
                    record(ctl_cmd=st[3], reply=gamectl.pipe(run_game.current(pid), st[3], timeout=20))
                    time.sleep(settle)
                    continue
                elif action in KEYS:
                    record(ctl=gamectl.pipe(run_game.current(pid), 'press %s~0.2' % KEYS[action]).strip())
                time.sleep(settle)
                shot('%s-%s' % (action, observed))
            record(ctl=gamectl.pipe(run_game.current(pid), 'quit').strip())
        except Exception as e:
            record(error='%s: %s' % (type(e).__name__, e))
    env = dict(os.environ, NXRS_METRICS=str(run), NXRS_INPUT_REPLAY=str(run / 'replay.txt'))
    env.update(dict(x.split('=', 1) for x in a.env))
    args = ([a.data] if a.data else []) + ['--fullscreen=0', '--metrics=1'] + ['--' + o for o in a.option]
    r = run_game.run(str(exe), str(run / 'run.log'), a.seconds or sc['seconds'], args, env=env, during=steps, say=lambda s: print(s, flush=True))
    for pm in PM:
        if pm.poll() is None:
            subprocess.run(['taskkill', '/PID', str(pm.pid), '/F'], capture_output=True)
            pm.wait(timeout=10)
    (run / 'result.json').write_text(json.dumps(r, indent=1))


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
    a = ap.parse_args()
    win = tuple(float(x) for x in a.window.split(',')) if a.window else None
    if a.scenario == 'stats':
        print(json.dumps(stats(a.run, win or (0, 1e9)), indent=1))
        return
    if not a.exe:
        ap.error('--exe <game.exe> is required')
    exe = Path(a.exe).resolve()
    sc = SCENARIOS[a.scenario]
    RUNS.mkdir(parents=True, exist_ok=True)
    run = RUNS / (time.strftime('%Y%m%d-%H%M%S-') + (a.label or a.scenario))
    run.mkdir()
    print(run, flush=True)
    try:
        scenario(a, sc, run, exe)
    except SystemExit as e:
        print('not started: %s' % e)
        sys.exit(1)
    print(json.dumps(stats(run, win or sc['window']), indent=1))


if __name__ == '__main__':
    main()
