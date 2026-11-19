"""gamectl.py: talk to a running game (the front end's control pipe and its window).

py scripts/gamectl.py <pid> <command...>        one control-pipe command (\\\\.\\pipe\\nxrs_ctl_<pid>), prints the reply:
                                               status, press A~0.2, P2 B, pads, shot [path], fullres [path], get KEY,
                                               set KEY VALUE, run KEY VALUE, record start|stop, fullscreen 0|1, counters,
                                               quit, help
py scripts/gamectl.py <pid> key F3 [F4 ...]     posts hotkeys to the game's window (WM_KEYDOWN/UP; no focus change):
                                               F1 F3 F4 F5 F7 F8 F11 Esc, or letters and Enter for the keyboard pad
py scripts/gamectl.py <pid> title               the window's title (its state: FPS limit, RECORDING, LSFG)
py scripts/gamectl.py find <exe name>           the PIDs of running processes with that image name (read-only)

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


def pipe(pid, line, timeout=30.0):
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
