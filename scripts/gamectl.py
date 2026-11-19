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
from ctypes import wintypes

user32 = ctypes.WinDLL('user32', use_last_error=True)
kernel32 = ctypes.WinDLL('kernel32', use_last_error=True)
VK = dict(F1=0x70, F3=0x72, F4=0x73, F5=0x74, F7=0x76, F8=0x77, F11=0x7A, Esc=0x1B, Enter=0x0D, Space=0x20, Up=0x26, Down=0x28, Left=0x25, Right=0x27)
WM_KEYDOWN, WM_KEYUP = 0x100, 0x101
CLASS = 'NxrsGameWindow'


LOCK = r'D:\Projects\Switch2RECOMP-main\build\evidence\GAME_LOCK'


def image_of(pid):
    out = subprocess.run(['tasklist', '/FI', 'PID eq %d' % pid, '/FO', 'CSV', '/NH'], capture_output=True, text=True).stdout
    return next((l.split('","')[0].strip('"') for l in out.splitlines() if l.startswith('"')), None)


def check_owned(pid):
    """Keys and pipe commands go only to the game this lane started and holds GAME_LOCK for: the lock's lane is
    NXRS_LANE and its executable is the image of `pid` (a PID from `find` or another lane's game is refused)."""
    lane = os.environ.get('NXRS_LANE', '')
    try:
        f = [x.strip() for x in open(LOCK).read().split('|')]
    except OSError:
        raise OSError('refused: no GAME_LOCK (the game is not running under run_game.py)')
    if not lane or f[0] != lane:
        raise OSError('refused: GAME_LOCK is %r, not this lane (NXRS_LANE=%r)' % (f[0], lane))
    img = image_of(pid)
    if img is None or len(f) < 2 or img.lower() != f[1].lower():
        raise OSError('refused: pid %d is %r, the locked game is %r' % (pid, img, f[1] if len(f) > 1 else None))


def pipe(pid, line, timeout=30.0):
    check_owned(pid)
    name = r'\\.\pipe\nxrs_ctl_%d' % pid
    t0 = time.time()
    while True:
        try:
            with open(name, 'r+b', buffering=0) as f:
                f.write((line + '\n').encode())
                out = b''
                while True:
                    try:
                        b = f.read(65536)
                    except OSError:
                        break  # the server disconnected after its reply
                    if not b:
                        break
                    out += b
                return out.decode(errors='replace')
        except OSError:
            if time.time() - t0 > timeout:
                raise
            time.sleep(0.2)


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
    out = subprocess.run(['tasklist', '/FI', 'IMAGENAME eq %s' % image, '/FO', 'CSV', '/NH'], capture_output=True, text=True).stdout
    return [int(l.split('","')[1]) for l in out.splitlines() if l.startswith('"')]


def main():
    a = sys.argv[1:]
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
