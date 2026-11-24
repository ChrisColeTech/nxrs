"""Windows suspended launch, kill-on-close containment and independent fail-stop.

No PID discovery/kill: retained process handles and job membership establish ownership.
The watchdog only waits on handles/timers; it never queries telemetry or writes evidence.
"""
import ctypes as C
from ctypes import wintypes as W
import math
import os
import subprocess
import sys
import tempfile
import threading
import time
import uuid
import _winapi
import msvcrt
from contextlib import contextmanager

K = C.WinDLL('kernel32', use_last_error=True)


def api(name, restype, *args):
    fn = getattr(K, name)
    fn.restype, fn.argtypes = restype, args
    return fn


close = api('CloseHandle', W.BOOL, W.HANDLE)
wait = api('WaitForSingleObject', W.DWORD, W.HANDLE, W.DWORD)
terminate_job = api('TerminateJobObject', W.BOOL, W.HANDLE, W.UINT)
create_job = api('CreateJobObjectW', W.HANDLE, C.c_void_p, W.LPCWSTR)
set_job = api('SetInformationJobObject', W.BOOL, W.HANDLE, C.c_int, C.c_void_p, W.DWORD)
open_process = api('OpenProcess', W.HANDLE, W.DWORD, W.BOOL, W.DWORD)
in_job = api('IsProcessInJob', W.BOOL, W.HANDLE, W.HANDLE, C.POINTER(W.BOOL))
open_job = api('OpenJobObjectW', W.HANDLE, W.DWORD, W.BOOL, W.LPCWSTR)
query_job = api('QueryInformationJobObject', W.BOOL, W.HANDLE, C.c_int, C.c_void_p, W.DWORD, C.c_void_p)
resume = api('ResumeThread', W.DWORD, W.HANDLE)
create_event = api('CreateEventW', W.HANDLE, C.c_void_p, W.BOOL, W.BOOL, W.LPCWSTR)
signal = api('SetEvent', W.BOOL, W.HANDLE)
create_mutex = api('CreateMutexW', W.HANDLE, C.c_void_p, W.BOOL, W.LPCWSTR)
release_mutex = api('ReleaseMutex', W.BOOL, W.HANDLE)
wait_many = api('WaitForMultipleObjects', W.DWORD, W.DWORD, C.POINTER(W.HANDLE), W.BOOL, W.DWORD)
init_attributes = api('InitializeProcThreadAttributeList', W.BOOL, C.c_void_p,
                      W.DWORD, W.DWORD, C.POINTER(C.c_size_t))
update_attribute = api('UpdateProcThreadAttribute', W.BOOL, C.c_void_p, W.DWORD,
                       C.c_size_t, C.c_void_p, C.c_size_t, C.c_void_p, C.c_void_p)
delete_attributes = api('DeleteProcThreadAttributeList', None, C.c_void_p)


class StartupInfo(C.Structure):
    _fields_ = [('cb', W.DWORD), ('reserved', W.LPWSTR), ('desktop', W.LPWSTR),
                ('title', W.LPWSTR), ('x', W.DWORD), ('y', W.DWORD),
                ('width', W.DWORD), ('height', W.DWORD), ('chars_x', W.DWORD),
                ('chars_y', W.DWORD), ('fill', W.DWORD), ('flags', W.DWORD),
                ('show', W.WORD), ('reserved_size', W.WORD), ('reserved_ptr', C.c_void_p),
                ('stdin', W.HANDLE), ('stdout', W.HANDLE), ('stderr', W.HANDLE)]


class StartupInfoEx(C.Structure):
    _fields_ = [('startup', StartupInfo), ('attributes', C.c_void_p)]


class ProcessInfo(C.Structure):
    _fields_ = [('process', W.HANDLE), ('thread', W.HANDLE), ('pid', W.DWORD), ('tid', W.DWORD)]


def create_suspended(command, input_handle, output, job, cwd, env):
    """JOB_LIST assigns atomically at creation, including if our parent dies mid-launch.

    Windows 10+ is required. Fail closed if extended startup/job-list is unsupported.
    CPython's STARTUPINFO exposes HANDLE_LIST but not JOB_LIST, hence this small binding.
    """
    size = C.c_size_t()
    init_attributes(None, 2, 0, C.byref(size))
    attributes = C.create_string_buffer(size.value)
    checked(init_attributes(attributes, 2, 0, C.byref(size)))
    try:
        handles = (W.HANDLE * 2)(input_handle, output)
        jobs = (W.HANDLE * 1)(job)
        checked(update_attribute(attributes, 0, 0x20002, handles, C.sizeof(handles), None, None))
        checked(update_attribute(attributes, 0, 0x2000d, jobs, C.sizeof(jobs), None, None))
        si = StartupInfoEx()
        si.startup.cb = C.sizeof(si)
        si.startup.flags = subprocess.STARTF_USESTDHANDLES
        si.startup.stdin, si.startup.stdout, si.startup.stderr = input_handle, output, output
        si.attributes = C.addressof(attributes)
        pi = ProcessInfo()
        environment = None
        if env is not None:
            environment = C.create_unicode_buffer('\0'.join(
                '%s=%s' % (key, value) for key, value in sorted(env.items(), key=lambda kv: kv[0].upper())) + '\0\0')
        create = api('CreateProcessW', W.BOOL, W.LPCWSTR, W.LPWSTR, C.c_void_p, C.c_void_p,
                     W.BOOL, W.DWORD, C.c_void_p, W.LPCWSTR, C.c_void_p, C.c_void_p)
        checked(create(None, C.create_unicode_buffer(subprocess.list2cmdline(command)), None, None,
                       True, 0x4 | 0x400 | 0x80000 | subprocess.CREATE_NO_WINDOW,
                       environment, cwd, C.byref(si), C.byref(pi)))
        return pi.process, pi.thread, pi.pid
    finally:
        delete_attributes(attributes)


def checked(value):
    if not value:
        raise C.WinError(C.get_last_error())
    return value


class CleanupError(RuntimeError):
    """User-space termination could not confirm exit; save restoration is unsafe."""


class CleanupTasks:
    """Attempt every cleanup step; never let a later success erase uncertainty."""
    def __init__(self):
        self.errors = []

    def attempt(self, label, operation):
        try:
            return operation()
        except BaseException as exc:
            self.errors.append('%s: %s: %s' % (label, type(exc).__name__, exc))

    def require(self):
        if self.errors:
            raise CleanupError('; '.join(self.errors))
        return True


class Limits(C.Structure):
    _fields_ = [('times', C.c_longlong * 2), ('flags', W.DWORD),
                ('min_working', C.c_size_t), ('max_working', C.c_size_t),
                ('active', W.DWORD), ('affinity', C.c_size_t),
                ('priority', W.DWORD), ('scheduling', W.DWORD)]


class ExtendedLimits(C.Structure):
    _fields_ = [('basic', Limits), ('io', C.c_ulonglong * 6),
                ('process_memory', C.c_size_t), ('job_memory', C.c_size_t),
                ('peak_process', C.c_size_t), ('peak_job', C.c_size_t)]


class Accounting(C.Structure):
    _fields_ = [('times', C.c_longlong * 4), ('faults', W.DWORD),
                ('total', W.DWORD), ('active', W.DWORD), ('terminated', W.DWORD)]


class MemoryStatus(C.Structure):
    _fields_ = [('length', W.DWORD), ('load', W.DWORD), ('values', C.c_ulonglong * 7)]


class ProcessMemory(C.Structure):
    _fields_ = [('length', W.DWORD), ('faults', W.DWORD), ('values', C.c_size_t * 9)]


def free_commit_mb():
    status = MemoryStatus()
    status.length = C.sizeof(status)
    checked(api('GlobalMemoryStatusEx', W.BOOL, C.c_void_p)(C.byref(status)))
    return status.values[3] // (1 << 20)  # ullAvailPageFile (commit available to this process)


def owns_named(name, pid):
    job = open_job(4, False, name)  # JOB_OBJECT_QUERY; does not grant termination access
    process = open_process(0x1000, False, pid)
    try:
        result = W.BOOL()
        return bool(job and process and in_job(process, job, C.byref(result)) and result.value)
    finally:
        if process:
            close(process)
        if job:
            close(job)


_lease_guard = threading.Lock()
_lease_threads = set()


class MachineLease:
    """Atomic across threads/processes/sessions; abandonment is released by Windows."""
    def __enter__(self):
        owner = threading.get_ident()
        with _lease_guard:
            if owner in _lease_threads:
                raise SystemExit('locked: use explicit borrowing of the existing machine lease')
        self.handle = checked(create_mutex(None, False, 'Global\\nxrs-harness-workload-v1'))
        if wait(self.handle, 0) not in (0, 0x80):
            close(self.handle)
            raise SystemExit('locked: another harness owns the machine')
        self.owner = owner
        self.held = True
        with _lease_guard:
            _lease_threads.add(owner)
        return self

    @contextmanager
    def borrow(self):
        """Explicit same-thread borrowing: no release/reacquire or recursive OS mutex."""
        if not self.held or self.owner != threading.get_ident():
            raise RuntimeError('machine lease is not held by this thread')
        yield self

    def __exit__(self, *exc):
        tasks = CleanupTasks()
        released = tasks.attempt('release machine lease', lambda: checked(release_mutex(self.handle)))
        tasks.attempt('close machine lease', lambda: checked(close(self.handle)))
        self.held = False
        if released:
            with _lease_guard:
                _lease_threads.discard(self.owner)
        tasks.require()


_spawn_lock = threading.Lock()


class Job:
    def __init__(self, seconds, stale_seconds=15):
        if not math.isfinite(seconds) or not 0 < seconds < 86400:
            raise ValueError('hard deadline must be finite, positive and under 24 hours')
        self.name = 'Local\\nxrs-harness-' + uuid.uuid4().hex
        self.handle = checked(create_job(None, self.name))
        self.stop = self.pulse = self.parent = self.ready = None
        self.worker = None
        self.closed = False
        self.cleanup_confirmed = False
        self.cleanup_errors = []
        self.children = []
        self.stopping = False
        self.deadline = time.monotonic() + seconds
        try:
            limits = ExtendedLimits()
            limits.basic.flags = 0x2000 | 0x200  # KILL_ON_JOB_CLOSE + JOB_MEMORY; no breakaway
            limits.job_memory = 20000 * (1 << 20)
            checked(set_job(self.handle, 9, C.byref(limits), C.sizeof(limits)))
            self.stop = checked(create_event(None, True, False, None))
            self.pulse = checked(create_event(None, False, False, None))
            self.parent = checked(open_process(0x100000, False, os.getpid()))
            self.ready = checked(create_event(None, True, False, None))
            handles = [self.handle, self.parent, self.stop, self.pulse, self.ready]
            si = subprocess.STARTUPINFO()
            si.lpAttributeList = {'handle_list': handles}
            with _spawn_lock:
                try:
                    for h in handles:
                        os.set_handle_inheritable(h, True)
                    self.worker = subprocess.Popen(
                        [sys.executable, os.path.abspath(__file__), *map(str, handles),
                         str(self.deadline), str(stale_seconds)], startupinfo=si,
                        close_fds=True, creationflags=subprocess.CREATE_NO_WINDOW)
                finally:
                    for h in handles:
                        os.set_handle_inheritable(h, False)
            if wait(self.ready, 1000) != 0 or self.worker.poll() is not None:
                raise RuntimeError('independent watchdog did not become ready')
        except BaseException:
            self.close()
            raise

    def heartbeat(self):
        if self.stopping or self.worker.poll() is not None or time.monotonic() >= self.deadline:
            self.kill()
            raise RuntimeError('independent watchdog stopped (deadline, stale monitor or worker failure)')
        checked(signal(self.pulse))

    def owns(self, pid):
        h = open_process(0x1000, False, pid)
        if not h:
            return False
        try:
            result = W.BOOL()
            checked(in_job(h, self.handle, C.byref(result)))
            return bool(result.value)
        finally:
            close(h)

    def adopt(self, pid):
        """Retain the exact query/synchronize handle whose membership was verified."""
        handle = checked(open_process(0x101000, False, pid))
        try:
            member = W.BOOL()
            checked(in_job(handle, self.handle, C.byref(member)))
            if not member.value:
                raise OSError('refused unowned successor PID %d' % pid)
            child = Child(handle, pid, self)
            self.children.append(child)
            return child
        except BaseException:
            try:
                checked(close(handle))
            except BaseException as exc:
                self.uncertain('close rejected successor handle', exc)
            raise

    def uncertain(self, label, exc):
        message = '%s: %s' % (label, exc)
        self.cleanup_errors.append(message)
        raise CleanupError(message) from exc

    def launch(self, command, stdout, cwd=None, env=None, before_resume=None, on_resume=None):
        """Assign before the first instruction. No child can escape the assignment gap."""
        self.heartbeat()
        output = msvcrt.get_osfhandle(stdout.fileno())
        with open(os.devnull, 'rb') as null, _spawn_lock:
            input_handle = msvcrt.get_osfhandle(null.fileno())
            try:
                os.set_handle_inheritable(input_handle, True)
                os.set_handle_inheritable(output, True)
                hp, ht, pid = create_suspended(command, input_handle, output, self.handle, cwd, env)
            finally:
                os.set_handle_inheritable(input_handle, False)
                os.set_handle_inheritable(output, False)
        child = Child(hp, pid, self)
        self.children.append(child)
        try:
            if before_resume:
                before_resume()
            self.heartbeat()  # deadline could expire during process creation
            if resume(ht) == 0xffffffff:
                raise C.WinError(C.get_last_error())
            if on_resume:
                on_resume()
        except BaseException:
            tasks = CleanupTasks()
            tasks.attempt('stop suspended child', self.kill)
            tasks.attempt('drain suspended child', self.drain)
            try:
                tasks.attempt('wait suspended child', lambda: child.wait(timeout=2))
            finally:
                tasks.attempt('close suspended child', child.close)
            tasks.require()
            raise
        finally:
            try:
                checked(close(ht))
            except BaseException as exc:
                self.uncertain('close launch thread', exc)
        return child

    def kill(self):
        self.stopping = True
        tasks = CleanupTasks()
        for child in self.children:
            if child.handle:
                status = tasks.attempt('poll before harness stop', child.poll)
                if status is None:
                    child.termination_requested = True
        if self.handle:
            tasks.attempt('TerminateJobObject', lambda: checked(terminate_job(self.handle, 124)))
        self.cleanup_errors.extend(tasks.errors)
        tasks.require()

    def close(self):
        if self.closed:
            if not self.cleanup_confirmed:
                raise CleanupError('; '.join(self.cleanup_errors) or 'job cleanup unconfirmed')
            return True
        tasks = CleanupTasks()
        tasks.attempt('terminate job', self.kill)
        tasks.attempt('drain job', self.drain)
        if self.stop:
            tasks.attempt('signal watchdog', lambda: checked(signal(self.stop)))
        if self.worker:
            def reap_worker():
                try:
                    self.worker.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    self.worker.kill()
                    self.worker.wait(timeout=2)
            tasks.attempt('reap watchdog', reap_worker)
        for child in self.children:
            if child.handle:
                tasks.attempt('confirm child exit', lambda c=child: c.wait(timeout=2))
            tasks.attempt('close child handle', child.close)
        for h in (self.parent, self.pulse, self.stop, self.ready, self.handle):
            if h:
                tasks.attempt('CloseHandle', lambda h=h: checked(close(h)))
        self.handle = None
        self.closed = True
        self.cleanup_errors.extend(tasks.errors)
        if self.cleanup_errors:
            raise CleanupError('; '.join(self.cleanup_errors))
        self.cleanup_confirmed = True
        return True

    def drain(self):
        until = time.monotonic() + 2
        while True:
            info = Accounting()
            try:
                checked(query_job(self.handle, 1, C.byref(info), C.sizeof(info), None))
            except BaseException as exc:
                self.uncertain('query job drain', exc)
            if info.active == 0:
                return
            if time.monotonic() >= until:
                self.uncertain('job drain', TimeoutError('processes did not drain'))
            time.sleep(.01)

    def private_mb(self):
        # Bounded process list, then query retained handles. Exits during sampling are benign;
        # any other unreadable live member is a monitor failure, never zero/safe.
        buffer = C.create_string_buffer(8 + 1024 * C.sizeof(C.c_size_t))
        checked(query_job(self.handle, 3, buffer, C.sizeof(buffer), None))
        count = C.c_uint32.from_buffer(buffer, 4).value
        pids = (C.c_size_t * count).from_buffer(buffer, 8)
        total = 0
        get_memory = api('K32GetProcessMemoryInfo', W.BOOL, W.HANDLE, C.c_void_p, W.DWORD)
        for pid in pids:
            handle = open_process(0x1010, False, pid)
            if not handle:
                if C.get_last_error() == 87:  # process already exited
                    continue
                raise C.WinError(C.get_last_error())
            try:
                info = ProcessMemory()
                info.length = C.sizeof(info)
                checked(get_memory(handle, C.byref(info), C.sizeof(info)))
                total += info.values[8]  # PrivateUsage
            finally:
                close(handle)
        return total // (1 << 20)


class Child:
    def __init__(self, handle, pid, job):
        self.handle, self.pid, self.job = handle, pid, job
        self.returncode = None
        self.termination_requested = False

    def poll(self):
        if self.returncode is None:
            try:
                result = wait(self.handle, 0)
                if result == 0:
                    self.returncode = _winapi.GetExitCodeProcess(self.handle)
                elif result != 258:
                    raise C.WinError(C.get_last_error())
            except BaseException as exc:
                self.job.uncertain('process status wait/query', exc)
        return self.returncode

    def wait(self, timeout=2):
        # Job.close may already have confirmed exit and closed this retained reference.
        if self.returncode is not None:
            return self.returncode
        if not self.handle:
            self.job.uncertain('process wait', RuntimeError('closed handle without cached exit status'))
        try:
            result = wait(self.handle, int(timeout * 1000))
        except BaseException as exc:
            self.job.uncertain('process wait', exc)
        if result == 258:
            raise subprocess.TimeoutExpired(str(self.pid), timeout)
        if result != 0:
            self.job.uncertain('process wait', C.WinError(C.get_last_error()))
        return self.poll()

    def kill(self):
        self.job.kill()

    def close(self):
        if self.handle:
            try:
                checked(close(self.handle))
                self.handle = None
            except BaseException as exc:
                self.job.uncertain('close child handle', exc)


def bounded_run(command, timeout=3, text=True):
    """Read-only probes/pipe helpers: bounded lifetime, including their descendants."""
    job = Job(timeout, stale_seconds=timeout)
    child = None
    try:
        with tempfile.TemporaryFile() as output:
            child = job.launch(command, output)
            rc = child.wait(timeout=timeout + .5)
            if rc == 124 or time.monotonic() >= job.deadline:
                raise subprocess.TimeoutExpired(command, timeout)
            output.seek(0)
            data = output.read(8 * 1024 * 1024 + 1)
            if len(data) > 8 * 1024 * 1024:
                raise OSError('probe output exceeded 8 MiB')
            if rc:
                raise OSError('probe failed (%s): %s' % (rc, data[:1000]))
            return data.decode('utf-8', 'replace').strip() if text else data
    finally:
        tasks = CleanupTasks()
        try:
            tasks.attempt('probe job cleanup', job.close)
        finally:
            if child:
                tasks.attempt('probe child handle close', child.close)
        tasks.require()


def watchdog():
    job, parent, stop, pulse, ready = map(int, sys.argv[1:6])
    deadline, stale = map(float, sys.argv[6:8])
    last = time.monotonic()
    handles = (W.HANDLE * 3)(parent, stop, pulse)
    try:
        checked(signal(ready))
        while True:
            remaining = min(deadline, last + stale) - time.monotonic()
            if remaining <= 0:
                break
            result = wait_many(3, handles, False, max(1, int(remaining * 1000)))
            if result == 2:
                last = time.monotonic()
            elif result != 258:
                break
    finally:
        terminate_job(job, 124)  # kill before any handle closing or Python teardown
        for h in (job, parent, stop, pulse, ready):
            close(h)


if __name__ == '__main__':
    watchdog()
