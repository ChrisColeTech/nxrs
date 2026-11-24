"""CPU-idle subprocess fixtures only; every branch self-exits within four seconds."""
import ctypes as C
import os
from pathlib import Path
import subprocess
import sys
import time

import harness_job as hj


def main():
    mode = sys.argv[1]
    if mode == 'write-save':
        Path(sys.argv[2]).write_bytes(b'child write')
        time.sleep(min(3, max(0, float(sys.argv[3]))) if len(sys.argv) > 3 else .15)
    elif mode == 'exit-code':
        time.sleep(float(sys.argv[3]) if len(sys.argv) > 3 else 0)
        os._exit(int(sys.argv[2]))
    elif mode == 'fatal-text':
        print(sys.argv[2], flush=True)
    elif mode == 'handover':
        child = subprocess.Popen([sys.executable, __file__, 'exit-code', sys.argv[2], '.35'])
        print('exec: program 0 -> program 1 fixture: python started as process %d' % child.pid, flush=True)
    elif mode in ('tree', 'tree-exit'):
        leaf = subprocess.Popen([sys.executable, __file__, 'leaf'])
        print('leaf', leaf.pid, flush=True)
        if mode == 'tree':
            time.sleep(3)
            leaf.wait(timeout=.5)
    elif mode in ('runner', 'runner-gap'):
        job = hj.Job(2, stale_seconds=2)
        with Path(sys.argv[2]).open('w') as output:
            if mode == 'runner-gap':
                create = hj.create_suspended
                def die_before_resume(*args, **kwargs):
                    hp, ht, pid = create(*args, **kwargs)
                    output.write('root %d\nwatchdog %d\n' % (pid, job.worker.pid))
                    output.flush()
                    os._exit(0)  # death in the launch window, while the child is suspended
                hj.create_suspended = die_before_resume
            child = job.launch([sys.executable, __file__, 'tree'], output)
            output.write('root %d\nwatchdog %d\n' % (child.pid, job.worker.pid))
            output.flush()
            time.sleep(3)
            job.close()
            child.close()
    elif mode == 'lease':
        with hj.MachineLease():
            print('acquired')
    elif mode in ('pipe', 'pipe-reply'):
        create = hj.api('CreateNamedPipeW', hj.W.HANDLE, hj.W.LPCWSTR, hj.W.DWORD,
                        hj.W.DWORD, hj.W.DWORD, hj.W.DWORD, hj.W.DWORD, hj.W.DWORD, C.c_void_p)
        connect = hj.api('ConnectNamedPipe', hj.W.BOOL, hj.W.HANDLE, C.c_void_p)
        handle = create(r'\\.\pipe\nxrs_ctl_%d' % os.getpid(), 3, 0, 1, 4096, 4096, 0, None)
        if handle == C.c_void_p(-1).value:
            raise C.WinError(C.get_last_error())
        print('ready', flush=True)
        try:
            connect(handle, None)
            if mode == 'pipe':
                time.sleep(3)  # connected server never replies
            else:
                write = hj.api('WriteFile', hj.W.BOOL, hj.W.HANDLE, C.c_void_p, hj.W.DWORD,
                               C.POINTER(hj.W.DWORD), C.c_void_p)
                flush = hj.api('FlushFileBuffers', hj.W.BOOL, hj.W.HANDLE)
                disconnect = hj.api('DisconnectNamedPipe', hj.W.BOOL, hj.W.HANDLE)
                response = b'frame 42\nseconds 1.0\n'
                written = hj.W.DWORD()
                hj.checked(write(handle, response, len(response), C.byref(written), None))
                flush(handle)
                disconnect(handle)
        finally:
            hj.close(handle)
    else:
        time.sleep(3)


if __name__ == '__main__':
    main()
