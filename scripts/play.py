"""Gameplay troubleshooting harness: run one built title through a scenario and report where it broke.

    py scripts/play.py <scenario> [--seconds N] [--diag pipeline|render] [--env K=V] [--option k=v]
                                  [--exe game.exe] [--real-save] [--label text] [--faults] [--ab-lanes]
    py scripts/play.py --list
    py scripts/play.py report <run folder>
    py scripts/play.py checkpoint <run folder>   # a known-good run's shots become the scenario's route references

Scenarios live in scripts/scenarios.toml; the game comes from the launcher library by the scenario's `title`
(or --exe). A run folder build/play/<time>-<label>/ gets run.log (the engine's log), metrics-*.csv and
frames-*.csv, NN-<name>.png screenshots, steps.jsonl (every step and control reply) and report.md/json.
report.md carries the anomaly timeline (what the engine flagged, near which shot), the route checks (a scenario's
`checks` against scripts/checkpoints.json: OFF-ROUTE marks the FPS invalid) and, with --faults, the frames the
engine captured when an anomaly or a one-frame white/black flash happened (<run>/faults).

Steps (one string each, run in order; `; S` waits S seconds after the step, `* N` repeats it):
    wait S                 sleep
    shot NAME              the game's picture at its resolution (control `fullres`); `shot window NAME` the window
    replay FILE            scripts/inputs/FILE becomes the input replay the engine is watching
    anything else          sent to the control pipe as is, the reply kept (press A, touch 640 360, threads, gfx...)

The game is killed by PID at the time limit or when no frame is presented for --stall seconds (after the
diagnostics `threads`, `gfx`, `counters`, `gaps` and a window shot). Saves: the title's save folder is copied
into the run folder and given to the game through NXOS_SAVE; --real-save uses the real one.
"""
import argparse
import ctypes
import json
import os
import re
import shutil
import subprocess
import sys
import threading
import time
import tomllib
from ctypes import wintypes
from pathlib import Path

import play_events
import play_report
import play_route

ROOT = Path(__file__).resolve().parents[1]
INPUTS = ROOT / 'scripts' / 'inputs'
SCENARIOS = ROOT / 'scripts' / 'scenarios.toml'
RUNS = ROOT / 'build' / 'play'
LIBRARY = Path(os.environ.get('LOCALAPPDATA', '')) / 'nxrs' / 'library.json'
SUCCESSOR = re.compile(r'exec: program \d+ -> program \d+ .*?: (.+?) started as process (\d+)')

K32 = ctypes.WinDLL('kernel32', use_last_error=True)
K32.OpenProcess.restype = wintypes.HANDLE
K32.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
K32.GetExitCodeProcess.argtypes = (wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD))
K32.CloseHandle.argtypes = (wintypes.HANDLE,)


def exit_code(pid):
    """None while the process runs, else its exit code (an unknown PID counts as exited, code -1)."""
    h = K32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not h:
        return -1
    try:
        code = wintypes.DWORD()
        if not K32.GetExitCodeProcess(h, ctypes.byref(code)):
            return -1
        return None if code.value == 259 else code.value  # STILL_ACTIVE
    finally:
        K32.CloseHandle(h)


def kill(pid):
    subprocess.run(['taskkill', '/PID', str(pid), '/T', '/F'], capture_output=True)


def pipe(pid, line, timeout=10.0):
    """One control command (one line per connection); the reply text. TimeoutError when none comes."""
    out = {}

    def exchange():
        try:
            name = r'\\.\pipe\nxrs_ctl_%d' % pid
            deadline = time.monotonic() + timeout
            while True:
                try:
                    f = open(name, 'r+b', buffering=0)
                    break
                except OSError:
                    if time.monotonic() > deadline:
                        raise
                    time.sleep(.2)
            with f:
                f.write((line + '\n').encode())
                data = bytearray()
                while True:
                    try:
                        b = f.read(65536)
                    except OSError:
                        break  # the server disconnects after its reply
                    if not b:
                        break
                    data += b
            out['reply'] = data.decode('utf-8', 'replace')
        except OSError as e:
            out['error'] = e

    t = threading.Thread(target=exchange, daemon=True)
    t.start()
    t.join(timeout)
    if 'reply' in out:
        return out['reply']
    if 'error' in out:
        raise out['error']
    raise TimeoutError('control pipe %d: no reply to %r in %.0f s' % (pid, line, timeout))


def expand(steps):
    """'press A ; 4 * 20' -> 20 x ('press A', 4.0)."""
    out = []
    for s in steps:
        m = re.fullmatch(r'\s*(.*?)\s*(?:;\s*([\d.]+))?\s*(?:\*\s*(\d+))?\s*', s)
        if not m or not m.group(1):
            raise ValueError('bad step %r' % s)
        out += [(m.group(1), float(m.group(2) or 0))] * int(m.group(3) or 1)
    return out


def shot_file(name):
    """A shot step's name as its file names it ('race later' -> 'race_later'; a window shot drops 'window ')."""
    if name.startswith('window '):
        name = name[7:]
    return re.sub(r'[^A-Za-z0-9]+', '_', name).strip('_')[:40]


def load_scenarios():
    with SCENARIOS.open('rb') as f:
        return tomllib.load(f)


def library_exe(title):
    games = json.loads(LIBRARY.read_text(encoding='utf-8-sig'))['games']
    hits = [g for g in games if g['title'] == title] or \
        [g for g in games if title.lower() in g['title'].lower()]
    if len(hits) != 1:
        raise SystemExit('library title %r matches %d entries: %s' % (title, len(hits), [g['title'] for g in hits]))
    g = hits[0]
    return Path(g['buildDir']) / (g['outputName'] + '.exe')


class Run:
    def __init__(self, folder, exe, steps, seconds, stall):
        self.folder, self.exe, self.steps = folder, exe, steps
        self.seconds, self.stall = seconds, stall
        self.pid = self.root = None
        self.log_at = 0
        self.done = threading.Event()
        self.lock = threading.Lock()
        self.events = (folder / 'steps.jsonl').open('a', encoding='utf-8')
        self.t0 = time.monotonic()
        self.steps_done = 0
        self.shots = 0
        self.step_error = None

    def note(self, **kv):
        kv['t'] = round(time.monotonic() - self.t0, 1)
        with self.lock:
            self.events.write(json.dumps(kv) + '\n')
            self.events.flush()

    def ctl(self, line, timeout=10.0, quiet=False):
        try:
            reply = pipe(self.pid, line, timeout)
        except (OSError, TimeoutError) as e:
            if not quiet:
                self.note(ctl=line, error=str(e))
            return None
        if not quiet:
            self.note(ctl=line, reply=reply.strip()[:4000])
        return reply

    def shot(self, name, command='fullres'):
        self.shots += 1
        path = self.folder / ('%02d-%s.png' % (self.shots, shot_file(name)))
        self.ctl('%s %s' % (command, path), timeout=20)
        for _ in range(100):  # fullres is taken at the next present
            if path.exists() and path.stat().st_size > 8 and path.read_bytes()[-8:-4] == b'IEND':
                return
            if self.done.wait(.1):
                return
        self.note(shot=path.name, error='no picture within 10 s (no present?)')

    def follow(self):
        """Switch to a successor process the game started (the control pipe is per process)."""
        try:
            with (self.folder / 'run.log').open('rb') as f:
                f.seek(self.log_at)
                text = f.read().decode('utf-8', 'replace')
        except OSError:
            return
        self.log_at += len(text.encode('utf-8', 'replace'))
        for m in SUCCESSOR.finditer(text):
            self.pid = int(m.group(2))
            self.note(successor=m.group(1).strip(), pid=self.pid)

    def run_steps(self):
        try:
            for i, (step, settle) in enumerate(self.steps):
                if self.done.is_set():
                    return
                word, _, rest = step.partition(' ')
                self.note(step=i + 1, of=len(self.steps), action=step)
                if word == 'wait':
                    settle += float(rest)
                elif word == 'shot':
                    window = rest.startswith('window ')
                    self.shot(rest[7:] if window else rest, 'shot' if window else 'fullres')
                elif word == 'replay':
                    shutil.copyfile(INPUTS / rest, self.folder / 'replay.txt')
                else:
                    self.ctl(step)
                self.steps_done = i + 1
                if self.done.wait(settle):
                    return
        except Exception as e:  # a broken step ends the scenario, never the watchdog
            self.step_error = '%s: %s' % (type(e).__name__, e)
            self.note(error=self.step_error)

    def watch(self, proc):
        """The outcome: completed, exited, hung or timed out."""
        steps = threading.Thread(target=self.run_steps, daemon=True)
        steps.start()
        frames, moved = -1, time.monotonic()
        while True:
            time.sleep(2)
            self.follow()
            code = exit_code(self.pid)
            if code is not None:
                if self.pid == self.root or exit_code(proc.pid) is not None:
                    return 'exited', code
                continue  # a successor ended; the log names its own successor next
            elapsed = time.monotonic() - self.t0
            if not steps.is_alive():
                self.ctl('quit')
                for _ in range(10):
                    time.sleep(1)
                    code = exit_code(self.pid)
                    if code is not None:
                        return 'completed', code
                return 'completed (quit ignored, killed)', None
            if elapsed > self.seconds:
                self.diagnose('time limit')
                return 'timed out', None
            status = self.ctl('status', timeout=5, quiet=True) or ''
            m = re.search(r'^frame (\d+)', status, re.M)
            if m:
                n = int(m.group(1))
                if n != frames:
                    frames, moved = n, time.monotonic()
            # like the engine's own stall watch: only once something was presented (boot may compile for long)
            if frames > 0 and time.monotonic() - moved > self.stall:
                self.diagnose('no frame presented for %d s' % self.stall)
                return 'hung', None

    def diagnose(self, why):
        self.note(diagnose=why)
        for command in ('threads', 'gfx', 'counters', 'gaps'):
            self.ctl(command, timeout=10)
        self.shot('diagnose', 'shot')


def start(a, sc, name):
    exe = Path(a.exe) if a.exe else library_exe(sc['title'])
    if not exe.is_file():
        raise SystemExit('no game executable at %s (build it first)' % exe)
    stamp = time.strftime('%Y%m%d-%H%M%S')
    folder = RUNS / ('%s-%s' % (stamp, a.label or name))
    folder.mkdir(parents=True)
    env = dict(os.environ, NXRS_METRICS=str(folder), NXRS_INPUT_REPLAY=str(folder / 'replay.txt'),
               NXRS_CONTROLLER_BACKGROUND='1', NXRS_KEEP_FOCUS='1', NXRS_NO_ACTIVATE='1',
               NXOS_SWKBD='auto')  # nobody types in a scripted run: the keyboard answers its default at once
    if a.diag in ('pipeline', 'render'):
        env.update(NXGFX_PIPE_LOG='1', NXGFX_WAIT_DIAG='1')
    if a.diag == 'render':
        env['NXGFX_PROFILE'] = '1'
    if a.faults:
        # fault-triggered captures: the presented frame (and the one before) at every anomaly / image-check trip
        (folder / 'faults').mkdir()
        env['NXGFX_FAULT_SHOTS'] = str(folder / 'faults')
    if not a.real_save:
        real = exe.parent / (exe.stem + '_data') / 'save'
        if real.is_dir():
            shutil.copytree(real, folder / 'save')
        else:
            (folder / 'save').mkdir()
        env['NXOS_SAVE'] = str(folder / 'save')
    env.update(dict(kv.split('=', 1) for kv in [*sc.get('env', []), *a.env]))
    args = ['--fullscreen=0', '--metrics=1'] + ['--' + o for o in [*sc.get('options', []), *a.option]]
    seconds = a.seconds or sc['seconds']
    meta = dict(scenario=name, exe=str(exe), args=args, seconds=seconds, stall=a.stall,
                window=sc.get('window'), fps=sc.get('fps', 60), diag=a.diag, real_save=a.real_save,
                env={k: v for k, v in env.items() if k.startswith('NX')},
                steps=sc['steps'], checks=sc.get('checks', {}), started=stamp)
    (folder / 'run.json').write_text(json.dumps(meta, indent=1), encoding='utf-8')
    print(folder, flush=True)
    run = Run(folder, exe, expand(sc['steps']), seconds, a.stall)
    with (folder / 'run.log').open('wb') as log:
        proc = subprocess.Popen([str(exe), *args], stdout=log, stderr=subprocess.STDOUT, cwd=exe.parent, env=env)
    run.pid = run.root = proc.pid
    run.note(started=str(exe), pid=proc.pid)
    try:
        outcome, code = run.watch(proc)
    except KeyboardInterrupt:
        outcome, code = 'interrupted', None
    finally:
        run.done.set()
        # Ask before killing: a process killed while pipelines compile has them recorded as compiler crashes
        # (`pipeline-crashed.bin`) and their draws skipped from then on.
        if exit_code(run.pid) is None:
            run.ctl('quit', timeout=5)
            for _ in range(20):
                if exit_code(run.pid) is not None:
                    break
                time.sleep(.5)
        for pid in {run.pid, proc.pid}:
            if exit_code(pid) is None:
                kill(pid)
    run.note(outcome=outcome, code=code)
    result = dict(outcome=outcome, exit_code=code, seconds=round(time.monotonic() - run.t0, 1),
                  steps_done=run.steps_done, steps=len(run.steps), step_error=run.step_error)
    (folder / 'result.json').write_text(json.dumps(result, indent=1), encoding='utf-8')
    print(play_report.write(folder))
    return folder


def ab_lanes(a, sc, name):
    """The scenario twice, translation in one piece (NXGFX_TRANSLATE_THREADS=0) then the default lanes, and ab.md
    in the second run's folder: anomaly counts, route distances and FPS side by side."""
    runs = []
    for label, env in (('serial', ['NXGFX_TRANSLATE_THREADS=0']), ('lanes', [])):
        b = argparse.Namespace(**vars(a))
        b.env = [*a.env, *env]
        b.label = '%s-%s' % (a.label or name, label)
        runs.append(start(b, sc, name))
    print(ab_report(name, runs))


def ab_report(name, runs):
    """ab.md in the second run's folder (serial run first); its text."""
    reps = [json.loads((f / 'report.json').read_text(encoding='utf-8')) for f in runs]
    kinds = [r['anomalies']['kinds'] for r in reps]
    md = ['# %s: serial vs lanes' % name, '', '- serial (NXGFX_TRANSLATE_THREADS=0): `%s`' % runs[0].name,
          '- lanes (default): `%s`' % runs[1].name, '',
          '| | serial | lanes |', '|---|---|---|',
          '| outcome | %s | %s |' % tuple(r['outcome'] for r in reps),
          '| off route | %s | %s |' % tuple(', '.join(r['off_route']) or '-' for r in reps),
          '| FPS mean (window) | %s | %s |' % tuple(r['metrics'].get('fps_mean') for r in reps)]
    for k in play_events.KIND_ORDER:
        if k in kinds[0] or k in kinds[1]:
            md.append('| %s (run / window) | %s | %s |' % (k, *('%d / %d' % (x[k]['total'], x[k]['in_window'])
                                                                if k in x else '-' for x in kinds)))
    dist = [{c['shot']: play_route.describe(c) for c in r['route']} for r in reps]
    for shot in sorted(set(dist[0]) | set(dist[1])):
        md.append('| route `%s` | %s | %s |' % (shot, dist[0].get(shot, '-'), dist[1].get(shot, '-')))
    (runs[1] / 'ab.md').write_text('\n'.join(md) + '\n', encoding='utf-8')
    return '\n'.join(md)


def checkpoint(folder, scenarios):
    """A known-good run's declared shots become its scenario's route references (scripts/checkpoints.json)."""
    folder = Path(folder)
    name = json.loads((folder / 'run.json').read_text(encoding='utf-8'))['scenario']
    checks = scenarios.get(name, {}).get('checks', {})
    if not checks:
        raise SystemExit('scenario %s declares no checks (scenarios.toml `checks = { shot = "what it shows" }`)' % name)
    stored = play_route.record(folder, name, checks)
    missing = sorted(set(checks) - set(stored))
    print('%s: references from %s for %s%s' % (name, folder.name, ', '.join(stored) or 'nothing',
                                               '; no shot for %s' % ', '.join(missing) if missing else ''))


def main():
    if len(sys.argv) > 2 and sys.argv[1] == 'report':
        print(play_report.write(Path(sys.argv[2])))
        return
    scenarios = load_scenarios()
    if len(sys.argv) > 2 and sys.argv[1] == 'checkpoint':
        checkpoint(sys.argv[2], scenarios)
        return
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('scenario', nargs='?', choices=sorted(scenarios))
    ap.add_argument('--list', action='store_true')
    ap.add_argument('--exe')
    ap.add_argument('--seconds', type=int, help='time limit (default: the scenario\'s)')
    ap.add_argument('--stall', type=int, default=30, help='seconds without a presented frame that count as hung')
    ap.add_argument('--diag', choices=['none', 'pipeline', 'render'], default='none',
                    help='pipeline: compile/wait logging; render: also CPU profiling')
    ap.add_argument('--env', action='append', default=[], help='NAME=VALUE for the game')
    ap.add_argument('--option', action='append', default=[], help='key=value ini option for this run')
    ap.add_argument('--real-save', action='store_true', help='use the title\'s real save folder')
    ap.add_argument('--faults', action='store_true',
                    help='fault-triggered captures into <run>/faults (NXGFX_FAULT_SHOTS): the presented frame and '
                         'the one before at every engine anomaly or one-frame white/black flash')
    ap.add_argument('--ab-lanes', action='store_true',
                    help='run twice, NXGFX_TRANSLATE_THREADS=0 then the default lanes; ab.md compares anomaly '
                         'counts, route checks and FPS')
    ap.add_argument('--label')
    a = ap.parse_args()
    if a.list or not a.scenario:
        for name, sc in sorted(scenarios.items()):
            print('%-14s %-38s %4d s  %s' % (name, sc['title'], sc['seconds'], sc.get('about', '')))
        return
    if a.ab_lanes:
        ab_lanes(a, scenarios[a.scenario], a.scenario)
    else:
        start(a, scenarios[a.scenario], a.scenario)


if __name__ == '__main__':
    main()
