"""run_game.py <exe> <log> <hard timeout s> [args...]: the machine's game rules for one run.

Several games may run at once (one per lane). Each run writes its lane's claim file GAME_LOCK.<lane> (lane, exe, time,
pid), checks free commit > 40 GB, starts the exe, watches it BY PID (kill if private bytes > 20 GB or free commit
< 25 GB, or past the hard timeout) and deletes its claim once it exited. The claim is what gamectl.py checks before it
sends a key or command, so a lane only ever drives its own game.
NXRS_EXCLUSIVE=1: the run needs the machine to itself (FPS measurements): it starts only when no other game runs, and
other lanes refuse to start while it runs. The old single GAME_LOCK, if another tool holds it, counts as exclusive.
Environment: NXRS_LANE names the lane. scripts/play_scenario.py runs games through `run()` here.
"""
import datetime
import glob
import json
import os
import subprocess
import sys
import threading
import time

EVIDENCE = r'D:\Projects\Switch2RECOMP-main\build\evidence'
LEGACY_LOCK = os.path.join(EVIDENCE, 'GAME_LOCK')


def claim_path(lane=None):
    return os.path.join(EVIDENCE, 'GAME_LOCK.' + (lane or os.environ.get('NXRS_LANE', 'agent')))


def read_claim(path):
    """dict(lane, exe, time, pid, exclusive) of a claim file, or None."""
    try:
        f = [x.strip() for x in open(path).read().split('|')]
    except OSError:
        return None
    return dict(lane=f[0], exe=f[1] if len(f) > 1 else '', time=f[2] if len(f) > 2 else '',
                pid=int(f[3]) if len(f) > 3 and f[3].isdigit() else 0, exclusive=len(f) > 4 and f[4] == 'exclusive')


def other_claims():
    """Live claims of other lanes (claims whose process is gone are stale and removed), plus the legacy lock."""
    out = []
    mine = claim_path()
    for p in glob.glob(os.path.join(EVIDENCE, 'GAME_LOCK.*')):
        if os.path.normcase(p) == os.path.normcase(mine):
            continue
        c = read_claim(p)
        if c and c['pid'] and not alive(c['pid']):
            try:
                os.remove(p)
            except OSError:
                pass
            continue
        if c:
            out.append(c)
    if os.path.exists(LEGACY_LOCK):
        out.append(dict(lane='legacy GAME_LOCK', exe=open(LEGACY_LOCK).read().strip(), time='', pid=0, exclusive=True))
    return out
PRIVATE_LIMIT_MB = 20000
COMMIT_MIN_START_MB = 40000
COMMIT_MIN_RUN_MB = 25000


def ps(cmd):
    return subprocess.run(['pwsh', '-NoProfile', '-Command', cmd], capture_output=True, text=True).stdout.strip()


def free_commit_mb():
    return int(ps("[int]((Get-CimInstance Win32_OperatingSystem).FreeVirtualMemory/1024)") or 0)


def other_games():
    """Running game executables: the launcher's builds, Turbo's, and this engine's (build\\ except build\\tools\\)."""
    t = ps(r"Get-Process | Where-Object { $_.Path -and ($_.Path -like '*SwitchRecomp Builds*' -or "
           r"($_.Path -like '*\RecompLauncherEngine\build\*.exe' -and $_.Path -notlike '*\build\tools\*') -or "
           r"$_.Path -like '*Switch2RECOMP*\build\*.exe') } | Select-Object Id,Name,Path | ConvertTo-Json -Compress")
    if not t:
        return []
    r = json.loads(t)
    return r if isinstance(r, list) else [r]


COMPILERS = ('cargo.exe', 'rustc.exe', 'clang.exe', 'clang++.exe', 'lld-link.exe', 'ld.lld.exe', 'nxshaders.exe', 'nxrs-build.exe', 'nxrs.exe', 'dmsh_xlate.exe')


def compilers():
    """Build tools running now (a game run must never overlap a build: the PC bugchecked when one did, 2026-11-19)."""
    out = subprocess.run(['tasklist', '/FO', 'CSV', '/NH'], capture_output=True, text=True).stdout.lower()
    return sorted({c for c in COMPILERS if '"%s"' % c in out})


def priv_mb(pid):
    v = ps("(Get-Process -Id %d -ErrorAction SilentlyContinue).PrivateMemorySize64" % pid)
    return int(v) // (1 << 20) if v else 0


def kill_pid(pid):
    ps('Stop-Process -Id %d -Force' % pid)


# The process being watched: the one run() started, or the successor it handed over to (oe::ExecuteProgram: the game
# logs `exec: ... started as process N` and exits; the watch, the lock and the scenario's pipe follow process N).
CURRENT = {'pid': None}


def current(pid):
    return CURRENT['pid'] or pid


def successor(logp, after):
    """The successor process the game's log names after byte offset `after` (None if none)."""
    import re
    try:
        with open(logp, 'rb') as f:
            f.seek(after)
            t = f.read().decode('utf-8', 'replace')
    except OSError:
        return None
    m = None
    for m in re.finditer(r'exec: program \d+ -> program \d+ .*?: (.+?) started as process (\d+)', t):
        pass
    return (m.group(1).strip(), int(m.group(2))) if m else None


def alive(pid):
    return ps("if (Get-Process -Id %d -ErrorAction SilentlyContinue) { 1 } else { 0 }" % pid) == '1'


def run(exe, logp, hard, args=(), env=None, during=None, say=print):
    """One guarded run. `during(pid)` (optional) runs on a thread while the game runs (a scenario's steps).
    Returns dict(why, exit, peak_mb, seconds) or raises SystemExit with the reason it could not start."""
    exclusive = os.environ.get('NXRS_EXCLUSIVE') == '1'
    others = other_claims()
    if any(c['exclusive'] for c in others):
        raise SystemExit('locked: an exclusive run holds the machine: %s' % [c for c in others if c['exclusive']])
    if exclusive:
        g = other_games()
        if others or g:
            raise SystemExit('exclusive run refused: other games run: %s %s' % (others, g))
    # builds may overlap a run (owner, 2026-11-19): NXRS_WAIT_FOR_BUILDS=1 restores the wait if crashes under load return
    if os.environ.get('NXRS_WAIT_FOR_BUILDS') == '1':
        for i in range(61):
            busy = compilers()
            if not busy:
                break
            if i == 60:
                raise SystemExit('builds still running after 10 minutes: %s' % busy)
            if i == 0:
                say('waiting for builds to finish: %s' % busy)
            time.sleep(10)
    fc = free_commit_mb()
    if fc < COMMIT_MIN_START_MB:
        raise SystemExit('free commit %d MB < 40 GB' % fc)
    lane = os.environ.get('NXRS_LANE', 'agent')
    LOCK = claim_path(lane)
    old = read_claim(LOCK)
    if old and old['pid'] and alive(old['pid']):
        raise SystemExit('this lane already runs a game: %s' % old)
    mode = ' | exclusive' if exclusive else ''

    def claim_text(name, pid):
        return '%s | %s | %s | %d%s\n' % (lane, name, datetime.datetime.now().isoformat(timespec='seconds'), pid, mode)
    mine = claim_text(os.path.basename(exe), 0)
    with open(LOCK, 'w') as fl:
        fl.write(mine)
    say('claim taken (%s%s); free commit %d MB' % (lane, ', exclusive' if exclusive else '', fc))
    peak = 0
    why = 'exited'
    rc = None
    t0 = time.time()
    try:
        with open(logp, 'wb') as lf:
            p = subprocess.Popen([exe] + list(args), stdout=lf, stderr=subprocess.STDOUT, cwd=os.path.dirname(exe), env=env)
            CURRENT['pid'] = p.pid
            mine = claim_text(os.path.basename(exe), p.pid)
            with open(LOCK, 'w') as fl:
                fl.write(mine)
            if during:
                threading.Thread(target=during, args=(p.pid,), daemon=True).start()
            pid, running, seen = p.pid, (lambda: p.poll() is None), 0
            while True:
                while running():
                    time.sleep(2)
                    if not os.path.exists(LOCK):
                        # another lane's stale-claim sweep can read the claim just before a hand-over rewrites it
                        # (the old pid is gone) and remove it after: write it again while the game runs
                        with open(LOCK, 'w') as fl:
                            fl.write(mine)
                    m = priv_mb(pid)
                    peak = max(peak, m)
                    fc = free_commit_mb()
                    if m > PRIVATE_LIMIT_MB or fc < COMMIT_MIN_RUN_MB:
                        why = 'WATCHDOG: private %d MB, free commit %d MB' % (m, fc)
                        kill_pid(pid)
                        break
                    if time.time() - t0 > hard:
                        why = 'hard timeout %.0f s' % hard
                        kill_pid(pid)
                        break
                if pid == p.pid:
                    p.wait()
                    rc = p.returncode
                if why != 'exited':
                    break
                lf.flush()
                nxt = successor(logp, seen)
                if not nxt or nxt[1] == pid or not alive(nxt[1]):
                    break
                seen = os.path.getsize(logp)
                pid = nxt[1]
                CURRENT['pid'] = pid
                mine_new = claim_text(os.path.basename(nxt[0]), pid)
                with open(LOCK, 'w') as fl:
                    fl.write(mine_new)
                mine = mine_new
                say('the game handed over to %s (process %d); watching it' % (os.path.basename(nxt[0]), pid))
                running = (lambda q=pid: alive(q))
    finally:
        for _ in range(20):
            if not CURRENT['pid'] or not alive(CURRENT['pid']):
                break
            time.sleep(1)
        if os.path.exists(LOCK) and open(LOCK).read() == mine:
            os.remove(LOCK)
    r = dict(why=why, exit=rc, peak_mb=peak, seconds=round(time.time() - t0, 1))
    say('run done: %s, exit %s, peak private %d MB; lock released' % (why, rc, peak))
    return r


def main():
    if len(sys.argv) < 4:
        print(__doc__)
        sys.exit(2)
    try:
        run(sys.argv[1], sys.argv[2], float(sys.argv[3]), sys.argv[4:], say=lambda s: print(s, flush=True))
    except SystemExit as e:
        print(e)
        sys.exit(1)


if __name__ == '__main__':
    main()
