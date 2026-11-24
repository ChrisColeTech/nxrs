"""One authorized CPU production build: monitored lease/job, no game-runner bypass.

Game/GPU latches are retained and never cleared or used as build permission. This
entry point runs only the fixed build/provenance commands, never a user executable.
"""
import argparse
import datetime as dt
import json
import math
import os
from pathlib import Path
import sys
import time

import harness_job as hj
import harness_lifecycle as life
import harness_monitor as hm

ROOT = Path(__file__).resolve().parents[1]
PROVENANCE = ROOT / 'scripts/build_provenance.py'
COMPILERS = {'cargo.exe', 'rustc.exe', 'clang.exe', 'clang++.exe', 'lld-link.exe', 'ld.lld.exe',
             'link.exe', 'cl.exe', 'ninja.exe', 'msbuild.exe', 'gcc.exe', 'g++.exe',
             'cc1.exe', 'cc1plus.exe', 'nxrs.exe', 'nxrs-build.exe', 'nxshaders.exe', 'dmsh_xlate.exe'}


def write_json(path, value):
    def evidence(item):
        if isinstance(item, float) and not math.isfinite(item):
            return 'nonfinite:' + repr(item)
        if isinstance(item, dict):
            return {k: evidence(v) for k, v in item.items()}
        if isinstance(item, (list, tuple)):
            return [evidence(v) for v in item]
        return item
    temporary = path.with_suffix(path.suffix + '.tmp')
    with temporary.open('w', encoding='utf-8') as stream:
        json.dump(evidence(value), stream, indent=2, allow_nan=False)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)


def external_workloads(job):
    rows = json.loads(hm.powershell(
        '@(Get-CimInstance Win32_Process | Select-Object ProcessId,Name,ExecutablePath) | '
        'ConvertTo-Json -Compress') or '[]')
    if isinstance(rows, dict):
        rows = [rows]
    busy = []
    for row in rows:
        path = (row.get('ExecutablePath') or '').lower()
        game = ('switchrecomp builds' in path or '\\switch2recomp' in path and '\\build\\' in path
                or '\\recomplauncherengine\\build\\' in path and '\\build\\tools\\' not in path)
        if (row['Name'].lower() in COMPILERS or game) and live_external_process(job, row['ProcessId']):
            busy.append(row)
    return busy


def live_external_process(job, pid):
    """CIM snapshots lag short-lived cl.exe workers; absence is not non-membership.

    Query liveness and membership on one retained handle. Access/query failures
    still refuse; only a proven exit or ERROR_INVALID_PARAMETER (vanished PID) skips.
    """
    handle = hj.open_process(0x101000, False, pid)
    if not handle:
        error = hj.C.get_last_error()
        if error == 87:
            return False
        raise hj.C.WinError(error)
    try:
        state = hj.wait(handle, 0)
        if state == 0:
            return False
        if state != 258:
            raise hj.C.WinError(hj.C.get_last_error())
        member = hj.W.BOOL()
        queried = hj.in_job(handle, job.handle, hj.C.byref(member))
        query_error = hj.C.get_last_error() if not queried else None
        if not queried or not member.value:
            # Exit can race IsProcessInJob itself. Only a positive exit on this
            # same handle supersedes a non-member/error result; live errors refuse.
            state = hj.wait(handle, 0)
            if state == 0:
                return False
            if state != 258:
                raise hj.C.WinError(hj.C.get_last_error())
            if not queried:
                raise hj.C.WinError(query_error)
        return not member.value
    finally:
        hj.checked(hj.close(handle))


def build_environment():
    env = dict(os.environ)
    # These are process-local; Cargo's jobserver also governs cc-rs native compilation.
    env.update(PYTHONDONTWRITEBYTECODE='1', CARGO_BUILD_JOBS='1', CMAKE_BUILD_PARALLEL_LEVEL='1',
               MAKEFLAGS='-j1', NUM_JOBS='1', RAYON_NUM_THREADS='1', OMP_NUM_THREADS='1',
               CARGO_PROFILE_RELEASE_CODEGEN_UNITS='1',
               CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_CODEGEN_UNITS='1', CARGO_INCREMENTAL='0')
    if env.get('RUSTFLAGS') or env.get('CARGO_ENCODED_RUSTFLAGS'):
        raise RuntimeError('review existing Rust flags before the single-worker build')
    env['RUSTFLAGS'] = '-Clink-arg=/CGTHREADS:1'
    return env


class BuildRun:
    def __init__(self, folder, seconds=1800, monitor=None, overlap=external_workloads, poll_seconds=1):
        self.folder, self.overlap = Path(folder), overlap
        self.env = build_environment()
        self.monitor = monitor if monitor is not None else hm.Monitor(env=self.env)
        self.job = hj.Job(seconds)
        self.deadline = self.job.deadline
        self.retired_jobs = []
        self.state = life.RunState(workers_confirmed=True)
        self.started, self.last_observation = time.monotonic(), None
        self.sample = None
        self.summary = dict(samples=0, peak_cpu_c=0, peak_gpu_c=0, peak_owned_mb=0, max_sample_gap_s=0)
        self.phase, self.last_print = 'preflight', 0
        self.child = None
        self.poll_seconds = poll_seconds

    def status(self):
        return dict(runner_pid=os.getpid(), job=self.job.name, watchdog_pid=self.job.worker.pid,
                    child_pid=self.child.pid if self.child else None, phase=self.phase,
                    deadline_monotonic=self.deadline, elapsed_s=time.monotonic()-self.started,
                    completed_jobs=[dict(job=j.name, watchdog_pid=j.worker.pid,
                                         cleanup_confirmed=j.cleanup_confirmed) for j in self.retired_jobs],
                    observed=dt.datetime.now().astimezone().isoformat(), **self.summary)

    def observe(self, prelaunch=False):
        self.sample = self.monitor.sample()
        reason = self.monitor.check(self.sample)  # no game frames/progress policy in a compiler job
        private = self.job.private_mb()
        commit = hj.free_commit_mb()
        now = time.monotonic()
        self.summary['samples'] += 1
        for field, key in (('peak_cpu_c', 'cpu_c'), ('peak_gpu_c', 'gpu_c')):
            value = self.sample.get(key, {}).get('value')
            if isinstance(value, (int, float)) and math.isfinite(value):
                self.summary[field] = max(self.summary[field], value)
        self.summary['peak_owned_mb'] = max(self.summary['peak_owned_mb'], private)
        if self.last_observation is not None:
            self.summary['max_sample_gap_s'] = max(self.summary['max_sample_gap_s'], now-self.last_observation)
        self.last_observation = now
        if reason:
            raise RuntimeError('thermal refusal: ' + reason)
        if private >= 20000 or commit < (40000 if prelaunch else 25000):
            raise RuntimeError('memory refusal: owned %d MiB, free commit %d MiB' % (private, commit))
        busy = self.overlap(self.job)
        if busy:
            raise RuntimeError('external workload overlap: ' + json.dumps(busy))
        # Includes query and evidence I/O time in the final age decision.
        compact = {key: {k: v for k, v in item.items() if k != 'sensors'}
                   if isinstance(item, dict) else item for key, item in self.sample.items()}
        with (self.folder / 'thermal.jsonl').open('a', encoding='utf-8') as stream:
            stream.write(json.dumps(dict(at=now, phase=self.phase, owned_mb=private,
                                         free_commit_mb=commit, sample=compact), allow_nan=False) + '\n')
        write_json(self.folder / 'status.json', self.status())
        reason = self.monitor.check(self.sample)
        if reason:
            raise RuntimeError('late acquisition-age refusal: ' + reason)
        self.job.heartbeat()
        if now-self.last_print >= 30:
            print(json.dumps(self.status()), flush=True)
            self.last_print = now

    def execute(self, phase, command):
        if self.job.closed:
            if not self.state.cleanup_confirmed:
                raise hj.CleanupError('previous build phase cleanup unconfirmed')
            old = self.job
            # No deadline renewal: reserve a tenth-second margin for job creation.
            self.job = hj.Job(self.deadline-time.monotonic()-.1)
            self.retired_jobs.append(old)
            self.state = life.RunState(workers_confirmed=True)
            self.child = None
        self.phase = phase
        self.observe(prelaunch=True)
        with (self.folder / (phase + '.log')).open('wb') as output:
            self.child = self.job.launch(command, output, cwd=str(ROOT), env=self.env,
                                         before_resume=lambda: self.observe(prelaunch=True),
                                         on_resume=self.state.resumed)
            write_json(self.folder / 'status.json', self.status())
            while self.child.poll() is None:
                time.sleep(self.poll_seconds)
                self.observe()
            code = self.child.wait(timeout=2)
            if code == 0 and phase != 'production-build':
                self.job.drain()
        self.observe()  # also validate monitoring on a fast exit / packaging completion
        if code:
            raise RuntimeError('%s exited %d; see %s.log' % (phase, code, phase))
        if phase == 'production-build':
            # Compiler helpers may outlive a successful root. Stop them before
            # proving drain; verification cannot race an owned artifact writer.
            self.close()

    def close(self):
        if self.job.closed:
            life.finish_job(self.job, self.state)
            return
        cleanup = life.stop_and_join(self.job, self.job.children, [], self.state)
        try:
            life.finish_job(self.job, self.state)
        finally:
            cleanup.require()


def rollback(folder):
    """Only after positive drain. Rename our validated publication paths; never delete old release."""
    path = folder / 'publication.json'
    if not path.exists():
        return 'no publication attempted'
    journal = json.loads(path.read_text(encoding='utf-8-sig'))
    live, previous, stage = (Path(journal[k]) for k in ('live', 'previous', 'stage'))
    if (live != ROOT / 'dist/nxrs' or previous.parent != live.parent or stage.parent != live.parent
            or not previous.name.startswith('nxrs-previous-') or not stage.name.startswith('nxrs-stage-')):
        raise RuntimeError('unexpected publication paths; manual recovery required')
    if previous.exists():
        if live.exists():
            live.rename(stage)  # stage was moved, so should be absent; refuse if not
        previous.rename(live)
        return 'previous release restored; candidate retained at ' + str(stage)
    return 'previous release was not moved'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--evidence', type=Path, required=True)
    args = parser.parse_args()
    folder = args.evidence.resolve()
    if not folder.parent.is_dir():
        raise SystemExit('evidence parent must already exist')
    with hj.MachineLease():
        folder.mkdir()  # refusal on any prior attempt; no automatic retry / stop reuse
        run, error, recovery = None, None, None
        try:
            run = BuildRun(folder)
            write_json(folder / 'intent.json', dict(authorization='one monitored CPU production rebuild only',
                       command=['pwsh', '-NoProfile', '-File', str(ROOT / 'scripts/build/build.ps1'),
                                '-Jobs', '1', '-SkipGnu', '-RetainPrevious',
                                '-PublicationJournal', str(folder / 'publication.json')],
                       environment={k: v for k, v in run.env.items() if k in (
                           'CARGO_BUILD_JOBS', 'CMAKE_BUILD_PARALLEL_LEVEL', 'MAKEFLAGS', 'NUM_JOBS',
                           'RAYON_NUM_THREADS', 'OMP_NUM_THREADS', 'CARGO_PROFILE_RELEASE_CODEGEN_UNITS',
                           'CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_CODEGEN_UNITS', 'RUSTFLAGS')},
                       **run.status()))
            run.execute('preflight', [sys.executable, str(PROVENANCE), 'preflight', str(folder)])
            run.execute('production-build', ['pwsh', '-NoProfile', '-NonInteractive', '-File',
                        str(ROOT / 'scripts/build/build.ps1'), '-Jobs', '1', '-SkipGnu',
                        '-RetainPrevious', '-PublicationJournal', str(folder / 'publication.json')])
            run.execute('verify', [sys.executable, str(PROVENANCE), 'verify', str(folder)])
        except BaseException as exc:
            error = '%s: %s' % (type(exc).__name__, exc)
            if run:
                # Termination always precedes fault evidence, even when storage is broken.
                tasks = hj.CleanupTasks()
                tasks.attempt('build fault termination', run.job.kill)
                run.state.uncertain |= bool(tasks.errors)
                error += ('; ' + '; '.join(tasks.errors)) if tasks.errors else ''
            write_json(folder / 'BUILD-STOP.json', dict(reason=error, sample=run.sample if run else None))
        finally:
            if run:
                try:
                    run.close()
                except BaseException as exc:
                    error = (error or '') + '; cleanup unconfirmed: ' + str(exc)
                    write_json(folder / 'BUILD-STOP.json', dict(reason=error, sample=run.sample))
                if error and run.state.cleanup_confirmed:
                    try:
                        recovery = rollback(folder)
                    except BaseException as exc:
                        recovery = 'recovery required: ' + str(exc)
                result = dict(ok=error is None, error=error, recovery=recovery,
                              cleanup_confirmed=run.state.cleanup_confirmed,
                              exit=run.child.returncode if run.child else None, **run.status())
                write_json(folder / 'result.json', result)
                print(json.dumps(result), flush=True)
        return 0 if error is None else 1


if __name__ == '__main__':
    sys.exit(main())
