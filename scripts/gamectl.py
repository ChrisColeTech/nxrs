"""gamectl.py: talk to a running game (the front end's control pipe and its window).

py scripts/gamectl.py <pid> <command...>        one control-pipe command (\\\\.\\pipe\\nxrs_ctl_<pid>), prints the reply:
                                               status, press A~0.2, P2 B, pads, shot [path], fullres [path], get KEY,
                                               set KEY VALUE, run KEY VALUE, record start|stop, fullscreen 0|1, counters,
                                               quit, help
py scripts/gamectl.py <pid> key F3 [F4 ...]     posts hotkeys to the game's window (WM_KEYDOWN/UP; no focus change):
                                               F1 F3 F4 F5 F7 F8 F11 Esc, or letters and Enter for the keyboard pad
py scripts/gamectl.py <pid> title               the window's title (its state: FPS limit, RECORDING, LSFG)
py scripts/gamectl.py find <exe name>           the PIDs of running processes with that image name (read-only;
                                               never a target: keys and commands need GAME_LOCK held by NXRS_LANE
                                               for that PID's executable, `check_owned`)

Used by scripts/play_scenario.py; also by hand. Never touches any other program's window.
"""
import ctypes
import os
import subprocess
import sys
import time
import harness_job
from ctypes import wintypes

user32 = ctypes.WinDLL('user32', use_last_error=True)
kernel32 = ctypes.WinDLL('kernel32', use_last_error=True)
VK = dict(F1=0x70, F3=0x72, F4=0x73, F5=0x74, F7=0x76, F8=0x77, F11=0x7A, Esc=0x1B, Enter=0x0D, Space=0x20, Up=0x26, Down=0x28, Left=0x25, Right=0x27)
WM_KEYDOWN, WM_KEYUP = 0x100, 0x101
CLASS = 'NxrsGameWindow'


LOCK = r'D:\Projects\Switch2RECOMP-main\build\evidence\GAME_LOCK'


def image_of(pid):
    out = harness_job.bounded_run(['tasklist', '/FI', 'PID eq %d' % pid, '/FO', 'CSV', '/NH'], timeout=2)
    return next((l.split('","')[0].strip('"') for l in out.splitlines() if l.startswith('"')), None)


def check_owned(pid):
    """Require live job membership, plus a matching lane claim for standalone callers.

    Pending claims, executable-name matches and reused PIDs never establish ownership.
    """
    import run_game
    job = run_game.CURRENT.get('job')
    if job is not None:
        if run_game.CURRENT.get('stop') is not None and run_game.CURRENT['stop'].is_set():
            raise OSError('refused: run is stopping')
        if not job.owns(pid):
            raise OSError('refused: PID is not a member of the owned job')
        return job.name
    lane = os.environ.get('NXRS_LANE', '')
    if not lane:
        raise OSError('refused: NXRS_LANE is not set')
    try:
        f = [x.strip() for x in open(LOCK + '.' + lane).read().split('|')]
    except OSError:
        raise OSError('refused: lane %r has no game claim (the game is not running under run_game.py)' % lane)
    claimed = int(f[3]) if len(f) > 3 and f[3].isdigit() else 0
    if claimed:
        if claimed != pid:
            raise OSError('refused: pid %d is not lane %r\'s game (pid %d)' % (pid, lane, claimed))
        if len(f) > 5 and harness_job.owns_named(f[5], pid):
            return f[5]
    raise OSError('refused: claim lacks live job ownership (pending/image-only claims are not authority)')


def pipe(pid, line, timeout=30.0):
    """Bound the entire exchange, including a server that connects but never replies.

    A job-contained helper owns the blocking Windows pipe handle. Its independent
    watchdog stops it on timeout/parent exit; no abandoned reader thread survives.
    Never retry a command after connecting: an input may already have been delivered.
    """
    timeout = min(timeout, 2.0)
    job_name = check_owned(pid)
    try:
        reply = harness_job.bounded_run(
            [sys.executable, os.path.abspath(__file__), '--pipe-worker', str(pid), line, job_name],
            timeout=timeout, text=False)
    except subprocess.TimeoutExpired as e:
        raise TimeoutError('control pipe %d: %r exceeded %.2f s (delivery unknown)' % (pid, line, timeout)) from e
    return reply.decode('utf-8', 'replace')


def _pipe_exchange(pid, line, job_name):
    def verify():
        if not harness_job.owns_named(job_name, pid):
            raise OSError('refused: pipe target no longer belongs to the run job')
    verify()
    name = r'\\.\pipe\nxrs_ctl_%d' % pid
    while True:
        try:
            f = open(name, 'r+b', buffering=0)
            break
        except OSError:
            verify()
            time.sleep(0.2)
    with f:
        verify()
        f.write((line + '\n').encode('utf-8'))
        out = bytearray()
        while True:
            try:
                b = f.read(65536)
            except OSError as e:
                # CPython's CRT pipe read can report disconnect as EINVAL/EPIPE
                # without a winerror, including after a complete valid reply.
                if getattr(e, 'winerror', None) in (109, 232, 233) or e.errno in (22, 32):
                    break  # server disconnected after its reply
                raise
            if not b:
                break
            out.extend(b)
            if len(out) > 8 * 1024 * 1024:
                raise OSError('control reply exceeded 8 MiB')
        return bytes(out)


def window(pid):
    found = []
    EnumProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

    def cb(h, _):
        p = wintypes.DWORD()
        user32.GetWindowThreadProcessId(h, ctypes.byref(p))
        if p.value == pid:
            buf = ctypes.create_unicode_buffer(64)
            user32.GetClassNameW(h, buf, 64)
            if buf.value == CLASS:
                found.append(h)
        return True
    user32.EnumWindows(EnumProc(cb), 0)
    return found[0] if found else None


def title(pid):
    h = window(pid)
    if not h:
        return None
    buf = ctypes.create_unicode_buffer(512)
    user32.GetWindowTextW(h, buf, 512)
    return buf.value


def key(pid, names, hold=0.05):
    check_owned(pid)
    h = window(pid)
    if not h:
        raise SystemExit('no game window for pid %d' % pid)
    for n in names:
        vk = VK.get(n) or (ord(n.upper()) if len(n) == 1 else None)
        if vk is None:
            raise SystemExit('unknown key ' + n)
        user32.PostMessageW(h, WM_KEYDOWN, vk, 1)
        time.sleep(hold)
        user32.PostMessageW(h, WM_KEYUP, vk, 0xC0000001)
        time.sleep(0.05)


def find(image):
    """PIDs by image name: for looking only. Never send keys or commands to a PID found this way."""
    out = harness_job.bounded_run(['tasklist', '/FI', 'IMAGENAME eq %s' % image, '/FO', 'CSV', '/NH'], timeout=2)
    return [int(l.split('","')[1]) for l in out.splitlines() if l.startswith('"')]


def main():
    a = sys.argv[1:]
    if len(a) == 4 and a[0] == '--pipe-worker':
        try:
            sys.stdout.buffer.write(_pipe_exchange(int(a[1]), a[2], a[3]))
        except OSError as e:
            sys.stderr.buffer.write(str(e).encode('utf-8'))
            sys.exit(1)
        return
    if len(a) >= 2 and a[0] == 'find':
        print(' '.join(map(str, find(a[1]))))
        return
    if len(a) < 2:
        print(__doc__)
        sys.exit(2)
    pid = int(a[0])
    if a[1] == 'key':
        key(pid, a[2:])
    elif a[1] == 'title':
        print(title(pid))
    else:
        print(pipe(pid, ' '.join(a[1:])), end='')


if __name__ == '__main__':
    main()
