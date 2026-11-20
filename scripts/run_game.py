"""run_game.py <exe> <log> <hard timeout s> [args...]: the machine's game rules for one run.

Takes
GAME_LOCK atomically, checks no other game exe runs and free commit > 40 GB, starts the exe, watches it BY PID
(kill if private bytes > 20 GB or free commit < 25 GB, or past the hard timeout), deletes the lock once it exited.
Environment: NXRS_LANE names the lane in the lock. scripts/play_scenario.py runs games through `run()` here.
"""
import datetime
import json
import os
import subprocess
import sys
import threading
import time

LOCK = r'D:\Projects\Switch2RECOMP-main\build\evidence\GAME_LOCK'
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
    g = other_games()
    if g:
        raise SystemExit('another game runs: %s' % g)
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
    try:
        fd = os.open(LOCK, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
    except FileExistsError:
        raise SystemExit('locked: ' + open(LOCK).read().strip())
    mine = '%s | %s | %s\n' % (os.environ.get('NXRS_LANE', 'agent'), os.path.basename(exe), datetime.datetime.now().isoformat(timespec='seconds'))
    os.write(fd, mine.encode())
    os.close(fd)
    say('lock taken; free commit %d MB' % fc)
    peak = 0
    why = 'exited'
    rc = None
    t0 = time.time()
    try:
        with open(logp, 'wb') as lf:
            p = subprocess.Popen([exe] + list(args), stdout=lf, stderr=subprocess.STDOUT, cwd=os.path.dirname(exe), env=env)
            CURRENT['pid'] = p.pid
            if during:
                threading.Thread(target=during, args=(p.pid,), daemon=True).start()
            pid, running, seen = p.pid, (lambda: p.poll() is None), 0
            while True:
                while running():
                    time.sleep(2)
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
                mine_new = '%s | %s | %s\n' % (os.environ.get('NXRS_LANE', 'agent'), os.path.basename(nxt[0]), datetime.datetime.now().isoformat(timespec='seconds'))
                with open(LOCK, 'w') as fl:
                    fl.write(mine_new)
                mine = mine_new
                say('the game handed over to %s (process %d); watching it' % (os.path.basename(nxt[0]), pid))
                running = (lambda q=pid: alive(q))
    finally:
        for _ in range(20):
            if not other_games():
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
