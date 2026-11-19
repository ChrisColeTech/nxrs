"""run_game.py <exe> <log> <hard timeout s> [args...]: the machine's game rules for one run.

Waits until no build runs (cargo, rustc, clang, nxshaders, nxrs: a run overlapping a build crashed the PC), takes
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


COMPILERS = ('cargo.exe', 'rustc.exe', 'clang.exe', 'clang++.exe', 'lld-link.exe', 'ld.lld.exe', 'nxshaders.exe', 'nxrs.exe', 'dmsh_xlate.exe')


def compilers():
    """Build tools running now (a game run must never overlap a build: the PC bugchecked when one did, 2026-11-19)."""
    out = subprocess.run(['tasklist', '/FO', 'CSV', '/NH'], capture_output=True, text=True).stdout.lower()
    return sorted({c for c in COMPILERS if '"%s"' % c in out})


def priv_mb(pid):
    v = ps("(Get-Process -Id %d -ErrorAction SilentlyContinue).PrivateMemorySize64" % pid)
    return int(v) // (1 << 20) if v else 0


def kill_pid(pid):
    ps('Stop-Process -Id %d -Force' % pid)


def run(exe, logp, hard, args=(), env=None, during=None, say=print):
    """One guarded run. `during(pid)` (optional) runs on a thread while the game runs (a scenario's steps).
    Returns dict(why, exit, peak_mb, seconds) or raises SystemExit with the reason it could not start."""
    g = other_games()
    if g:
        raise SystemExit('another game runs: %s' % g)
    for i in range(61):   # wait up to 10 minutes for builds to finish
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
            if during:
                threading.Thread(target=during, args=(p.pid,), daemon=True).start()
            while p.poll() is None:
                time.sleep(2)
                m = priv_mb(p.pid)
                peak = max(peak, m)
                fc = free_commit_mb()
                if m > PRIVATE_LIMIT_MB or fc < COMMIT_MIN_RUN_MB:
                    why = 'WATCHDOG: private %d MB, free commit %d MB' % (m, fc)
                    kill_pid(p.pid)
                    break
                if time.time() - t0 > hard:
                    why = 'hard timeout %.0f s' % hard
                    kill_pid(p.pid)
                    break
            p.wait()
            rc = p.returncode
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
