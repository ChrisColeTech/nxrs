"""run_game.py <exe> <log> <hard timeout s> [args...]: the machine's game rules for one run.
Takes GAME_LOCK atomically, checks no other game exe runs and free commit > 40 GB, starts the exe, watches it BY PID
(kill if private bytes > 20 GB or free commit < 25 GB, or past the hard timeout), deletes the lock once it exited."""
import sys, os, subprocess, time, datetime, json
LOCK = r'D:\Projects\Switch2RECOMP-main\build\evidence\GAME_LOCK'
def ps(cmd):
    return subprocess.run(['pwsh', '-NoProfile', '-Command', cmd], capture_output=True, text=True).stdout.strip()
def free_commit_mb():
    return int(ps("[int]((Get-CimInstance Win32_OperatingSystem).FreeVirtualMemory/1024)") or 0)
def other_games():
    t = ps(r"Get-Process | Where-Object { $_.Path -and ($_.Path -like '*SwitchRecomp Builds*' -or $_.Path -like '*\build\games\*' -or $_.Path -like '*Switch2RECOMP*\build\*.exe') } | Select-Object Id,Name,Path | ConvertTo-Json -Compress")
    if not t: return []
    r = json.loads(t); return r if isinstance(r, list) else [r]
def priv_mb(pid):
    v = ps("(Get-Process -Id %d -ErrorAction SilentlyContinue).PrivateMemorySize64" % pid)
    return int(v) // (1 << 20) if v else 0
exe, logp, hard = sys.argv[1], sys.argv[2], float(sys.argv[3])
args = sys.argv[4:]
g = other_games()
if g: print('another game runs:', g); sys.exit(2)
fc = free_commit_mb()
if fc < 40000: print('free commit', fc, 'MB < 40 GB'); sys.exit(3)
try: fd = os.open(LOCK, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
except FileExistsError: print('locked:', open(LOCK).read()); sys.exit(1)
os.write(fd, ('lane A (os) agent | Mario Kart 8 Deluxe | %s\n' % datetime.datetime.now().isoformat(timespec='seconds')).encode()); os.close(fd)
print('lock taken; free commit %d MB' % fc, flush=True)
peak = 0; why = 'exited'
try:
    with open(logp, 'wb') as lf:
        p = subprocess.Popen([exe] + args, stdout=lf, stderr=subprocess.STDOUT, cwd=os.path.dirname(exe))
        t0 = time.time()
        while p.poll() is None:
            time.sleep(2)
            m = priv_mb(p.pid); peak = max(peak, m); fc = free_commit_mb()
            if m > 20000 or fc < 25000:
                why = 'WATCHDOG: private %d MB, free commit %d MB' % (m, fc); ps('Stop-Process -Id %d -Force' % p.pid); break
            if time.time() - t0 > hard:
                why = 'hard timeout %.0f s' % hard; ps('Stop-Process -Id %d -Force' % p.pid); break
        p.wait()
        rc = p.returncode
finally:
    for _ in range(20):
        if not other_games(): break
        time.sleep(1)
    if os.path.exists(LOCK) and 'lane A (os)' in open(LOCK).read(): os.remove(LOCK)
print('run done: %s, exit %s, peak private %d MB; lock released' % (why, rc, peak))
