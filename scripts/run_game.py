"""run_game.py <exe> <log> <hard timeout s> [args...]: the machine's game rules for one run.

One contained workload holds an atomic machine lease. Each run writes its lane's claim file GAME_LOCK.<lane> (lane, exe, time,
pid), checks free commit > 40 GB, starts the exe, watches it BY PID (kill if private bytes > 20 GB or free commit
< 25 GB, or past the hard timeout) and deletes its claim once it exited. The claim is what gamectl.py checks before it
sends a key or command, so a lane only ever drives its own game.
One game at a time (no environment opt-out): it starts only when no other game
runs, and other runs refuse to start while it runs. Device loss or a display-driver reset persistently blocks new
starts via build/GPU-STOP.json; cooldown expiry cannot resume. NXRS_GPU_STOP_PATH adds a caller-specific latch,
never replaces the shared one. The old single GAME_LOCK, if another tool holds it, counts as exclusive.
Every started run enables engine metrics and writes harness-memory.csv, periodic full-frame PNGs and control-pipe
snapshots, summary.json and artifact-manifest.json beside the log (or in NXRS_ARTIFACT_DIR). NXRS_CAPTURE_INTERVAL_S
(default 30) sets the interval; NXRS_AUTO_CAPTURE=0 disables periodic captures. NXRS_LANE names the lane.
scripts/play_scenario.py runs games through `run()` here.
"""
import datetime
import glob
import json
import os
import subprocess
import sys
import threading
import time

import observability
import run_triage
import gpu_stop
import harness_job
import harness_monitor
import harness_lifecycle
from pathlib import Path

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


def other_claims(lane=None):
    """Live claims of other lanes (claims whose process is gone are stale and removed), plus the legacy lock."""
    out = []
    mine = claim_path(lane)
    for p in glob.glob(os.path.join(EVIDENCE, 'GAME_LOCK.*')):
        if os.path.normcase(p) == os.path.normcase(mine):
            continue
        c = read_claim(p)
        # stale: its game exited, or it never got a pid (its runner died while waiting) within 30 minutes
        stale_pending = c and not c['pid'] and time.time() - os.path.getmtime(p) > 1800
        if c and ((c['pid'] and not alive(c['pid'])) or stale_pending):
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
POLL_SECONDS = 2


def ps(cmd):
    return harness_monitor.powershell(cmd)


def free_commit_mb():
    return harness_job.free_commit_mb()


def other_games():
    """Running game executables: the launcher's builds, Turbo's, and this engine's (build\\ except build\\tools\\)."""
    t = ps(r"Get-Process | Where-Object { $_.Path -and ($_.Path -like '*SwitchRecomp Builds*' -or "
           r"($_.Path -like '*\RecompLauncherEngine\build\*.exe' -and $_.Path -notlike '*\build\tools\*') -or "
           r"$_.Path -like '*Switch2RECOMP*\build\*.exe') } | Select-Object Id,Name,Path | ConvertTo-Json -Compress")
    if not t:
        return []
    r = json.loads(t)
    return r if isinstance(r, list) else [r]


COMPILERS = ('cargo.exe', 'rustc.exe', 'clang.exe', 'clang++.exe', 'lld-link.exe', 'ld.lld.exe',
             'nxshaders.exe', 'nxrs-build.exe', 'nxrs.exe', 'dmsh_xlate.exe', 'cl.exe', 'link.exe',
             'ninja.exe', 'msbuild.exe', 'gcc.exe', 'g++.exe', 'cc1.exe', 'cc1plus.exe')


def compilers():
    """Build tools running now (a game run must never overlap a build: the PC bugchecked when one did, 2026-11-19)."""
    out = harness_job.bounded_run(['tasklist', '/FO', 'CSV', '/NH'], timeout=2).lower()
    return sorted({c for c in COMPILERS if '"%s"' % c in out})


def priv_mb(pid):
    job = CURRENT.get('job')
    if job is None:
        raise OSError('private memory query requires an owned job')
    return job.private_mb()


def kill_pid(pid):
    job = CURRENT.get('job')
    if job is None or not job.owns(pid):
        raise OSError('refused: PID is not in the owned job')
    job.kill()


def _setting(env, name, default=None):
    return (env or os.environ).get(name, os.environ.get(name, default))


def diagnostic_snapshot(pid, folder, label, capture=True, stop=None):
    """Best-effort control-pipe evidence while the process is still responsive."""
    try:
        import gamectl
    except Exception:
        return
    folder = os.path.abspath(folder)
    os.makedirs(folder, exist_ok=True)
    target = current(pid)
    requests = []
    for command in ('status', 'threads', 'gaps', 'counters'):
        if stop is not None and stop.is_set():
            break
        started = time.monotonic()
        event = dict(command=command, pid=target, started_unix=time.time())
        try:
            reply = gamectl.pipe(target, command, timeout=2)
            with open(os.path.join(folder, 'diagnostics-%s-%s.txt' % (label, command)), 'w', encoding='utf-8') as f:
                f.write(reply)
            event.update(status='ok', bytes=len(reply.encode('utf-8')))
        except (OSError, ValueError) as e:
            event.update(status='timeout' if isinstance(e, TimeoutError) else 'error', error=str(e))
        event['elapsed_seconds'] = round(time.monotonic() - started, 3)
        requests.append(event)
    if capture and not (stop is not None and stop.is_set()):
        path = os.path.join(folder, 'auto-%s.png' % label)
        started = time.monotonic()
        event = dict(command='fullres', pid=target, started_unix=time.time(), status='no-present')
        try:
            gamectl.pipe(target, 'fullres %s' % path, timeout=2)
            for _ in range(30):
                if stop is not None and stop.is_set():
                    event['status'] = 'cancelled'
                    break
                if os.path.isfile(path) and os.path.getsize(path) > 0:
                    event['status'] = 'written'
                    break
                time.sleep(0.1)
        except OSError as e:
            event.update(status='timeout' if isinstance(e, TimeoutError) else 'error', error=str(e))
        event['elapsed_seconds'] = round(time.monotonic() - started, 3)
        requests.append(event)
    with open(os.path.join(folder, 'diagnostics-%s-requests.json' % label), 'w', encoding='utf-8') as f:
        json.dump(requests, f, indent=2)
    return requests


def graceful_stop(pid, folder, label):
    """Safety stops are kill-first; never wait for a control pipe or evidence."""
    kill_pid(pid)


# The process being watched: the one run() started, or the successor it handed over to (oe::ExecuteProgram: the game
# logs `exec: ... started as process N` and exits; the watch, the lock and the scenario's pipe follow process N).
CURRENT = {'pid': None, 'job': None, 'stop': None}


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
    handle = harness_job.open_process(0x100000, False, pid)
    if not handle:
        return False
    try:
        return harness_job.wait(handle, 0) == 258
    finally:
        harness_job.close(handle)


def successor_started(pid, attempts=25, delay=0.2):
    """A logged ExecuteProgram child may not be queryable until process startup settles."""
    for _ in range(attempts):
        if alive(pid):
            return True
        time.sleep(delay)
    return False


GPU_COOLDOWN_S = 600


def gpu_resets_since(t):
    """Times of NVIDIA driver resets (nvlddmkm event 153, the TDR that precedes 'display driver stopped responding')
    logged since epoch time `t`. Repeated GPU hangs preceded every hard crash of this PC (2026-11-18, -19, -20)."""
    since = datetime.datetime.fromtimestamp(t).strftime('%Y-%m-%dT%H:%M:%S')
    out = ps("try { Get-WinEvent -FilterHashtable @{LogName='System'; ProviderName='nvlddmkm'; Id=153; StartTime=[datetime]'%s'} "
             "-ErrorAction Stop | ForEach-Object { $_.TimeCreated.ToString('HH:mm:ss') } } "
             "catch { if ($_.FullyQualifiedErrorId -notlike 'NoMatchingEventsFound*') { throw } }" % since)
    return [l for l in out.splitlines() if l.strip()]


def _reap_owned(p, pid):
    """Exceptions (including evidence I/O failures) must not leave a save-writing child alive."""
    if p is None:
        return
    try:
        p.kill()  # the whole job, even when the root exited before its descendants
        p.job.drain()
        p.wait(timeout=2)
    except BaseException as exc:
        raise harness_job.CleanupError('owned process exit unconfirmed; retain save backup') from exc
    finally:
        try:
            p.close()
        except BaseException as exc:
            raise harness_job.CleanupError('owned process handle close unconfirmed') from exc


def run(exe, logp, hard, args=(), env=None, during=None, say=print, *, lease=None, state=None):
    """Persist intent before launch; interrupted intent requires explicit evidence review."""
    env = dict(os.environ if env is None else env)
    gpu_stop.require_clear(gpu_stop.paths(env))
    state = state if state is not None else harness_lifecycle.RunState()
    with (lease.borrow() if lease is not None else harness_job.MachineLease()):
        folder = Path(_setting(env, 'NXRS_ARTIFACT_DIR', os.path.dirname(logp) or '.')).resolve()
        folder.mkdir(parents=True, exist_ok=True)
        intent = folder / 'run-intent.json'
        if intent.exists():
            previous = json.loads(intent.read_text(encoding='utf-8'))
            if previous.get('state') not in ('completed', 'refused'):
                raise SystemExit('interrupted run intent requires review: %s (no PID will be killed)' % intent)
        record = dict(state='intent', exe=os.path.abspath(exe), runner_pid=os.getpid(),
                      started_unix=time.time(), hard_seconds=hard,
                      recovery='If no completed state: interrupted/unconfirmed; review logs and save-before; '
                               'never infer success, restore saves automatically or kill a recorded PID.')
        def persist():
            with intent.open('w', encoding='utf-8') as stream:
                json.dump(record, stream, indent=2, allow_nan=False)
                stream.flush()
                os.fsync(stream.fileno())
        persist()
        monitor = harness_monitor.Monitor(env=env)
        initial = monitor.sample()
        reason = monitor.check(initial)
        (folder / 'thermal-preflight.json').write_text(json.dumps(initial, indent=2), encoding='utf-8')
        if reason:
            record.update(state='refused', reason=reason)
            persist()
            raise SystemExit('thermal monitoring required: ' + reason)
        job = harness_job.Job(hard)
        CURRENT['job'] = job
        state.workers_confirmed = True  # no callback has been started
        try:
            result = _run(exe, logp, hard, args, env, during, say, monitor, job, state)
        finally:
            try:
                try:
                    harness_lifecycle.finish_job(job, state)
                except BaseException as exc:
                    state.cleanup_confirmed = False
                    message = 'cleanup unconfirmed; recovery required: %s' % exc
                    try:
                        gpu_stop.record_fault(gpu_stop.paths(env), dict(
                            kind='cleanup-uncertain', run=str(folder), reason=message))
                    except BaseException as persistence:
                        message += '; stop persistence failed: %s' % persistence
                    raise harness_job.CleanupError(message) from exc
            finally:
                CURRENT.update(job=None, pid=None, stop=None)
        record.update(state='completed', result=result)
        persist()
        return result


def _run(exe, logp, hard, args, env, during, say, monitor, job, state):
    """One guarded run. `during(pid)` (optional) runs on a thread while the game runs (a scenario's steps).
    Returns dict(why, exit, peak_mb, seconds) or raises SystemExit with the reason it could not start."""
    stop_paths = gpu_stop.paths(env)
    gpu_stop.require_clear(stop_paths)
    # MachineLease serializes all lanes, including same-lane simultaneous invocations.
    exclusive = True
    # The cooldown grows with the resets of the last hour (each reset counted once; a TDR logs two events seconds
    # apart): 10 minutes per reset, so a burst of hangs (what preceded every hard crash) spaces the next runs out.
    hour = gpu_resets_since(time.time() - 3600)
    distinct = []
    for t in sorted(hour):
        if not distinct or (datetime.datetime.strptime(t, '%H:%M:%S') - datetime.datetime.strptime(distinct[-1], '%H:%M:%S')).seconds > 30:
            distinct.append(t)
    cooldown = GPU_COOLDOWN_S * max(1, len(distinct))
    recent = gpu_resets_since(time.time() - cooldown)
    if recent:
        raise SystemExit('GPU cooldown: %d display-driver reset(s) in the last hour (latest %s); no game starts until '
                         'the driver has been quiet for %d minutes' % (len(distinct), max(recent), cooldown // 60))
    lane = _setting(env, 'NXRS_LANE', 'agent')
    others = other_claims(lane)
    if any(c['exclusive'] for c in others):
        raise SystemExit('locked: an exclusive run holds the machine: %s' % [c for c in others if c['exclusive']])
    busy = compilers()
    if busy:
        raise SystemExit('build overlap refused: %s' % busy)
    fc = free_commit_mb()
    if fc < COMMIT_MIN_START_MB:
        raise SystemExit('free commit %d MB < 40 GB' % fc)
    lane = _setting(env, 'NXRS_LANE', 'agent')
    if not lane or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_' for c in lane):
        raise ValueError('invalid lane name')
    env['NXRS_LANE'] = lane
    LOCK = claim_path(lane)
    old = read_claim(LOCK)
    if old and old['pid'] and alive(old['pid']):
        raise SystemExit('this lane already runs a game: %s' % old)
    mode = ' | exclusive' if exclusive else ''

    def claim_text(name, pid):
        return '%s | %s | %s | %d%s | %s\n' % (lane, name, datetime.datetime.now().isoformat(timespec='seconds'), pid, mode, job.name)
    mine = claim_text(os.path.basename(exe), 0)
    # A build/memory check may have taken time since the initial gate.
    gpu_stop.require_clear(stop_paths)
    with open(LOCK, 'w') as fl:
        fl.write(mine)
    # Legacy claims still block; no polling/tie-breaking claim race or twenty-minute wait.
    try:
        busy = other_claims(lane) + other_games()
        if busy:
            raise SystemExit('exclusive run refused: other games/claims: %s' % busy)
    except BaseException:
        if os.path.exists(LOCK):
            os.remove(LOCK)
        raise
    say('claim taken (%s%s); free commit %d MB' % (lane, ', exclusive' if exclusive else '', fc))
    peak = 0
    why = 'exited'
    rc = None
    t0 = time.time()
    started = time.monotonic()
    artifact_dir = os.path.abspath(_setting(env, 'NXRS_ARTIFACT_DIR', os.path.dirname(logp) or '.'))
    env = dict(os.environ if env is None else env)
    # Enable engine telemetry without adding a command-line option an older executable might reject.
    env.setdefault('NXRS_METRICS', artifact_dir)
    env.setdefault('NXRS_ARTIFACT_DIR', artifact_dir)
    memory_path = os.path.join(artifact_dir, 'harness-memory.csv')
    capture_stop = threading.Event()
    CURRENT['stop'] = capture_stop
    capture_thread = None
    during_thread = None
    p, pid = None, None
    watched = None
    children = []
    log_watch = None
    log_opened = False
    fault_errors = []
    reset_recorded = False
    fatal_recorded = set()
    log_fault_recorded = False
    observed_sample = None

    def fault(kind, resets=(), source=None):
        capture_stop.set()
        # Failure to terminate must not prevent durable fault recording, nor be hidden by it.
        termination = harness_job.CleanupTasks()
        termination.attempt('fault termination', job.kill)
        state.uncertain |= bool(termination.errors)
        event = dict(kind=kind, run=artifact_dir, log=os.path.abspath(logp), exe=os.path.abspath(exe),
                     lane=lane, pid=pid, exit=source.returncode if source else rc,
                     exit_pid=source.pid if source else (watched.pid if watched and rc is not None else None),
                     why=why, resets=list(resets), started_unix=t0,
                     monitoring=observed_sample,
                     provenance=os.path.join(artifact_dir, 'provenance.json'))
        try:
            gpu_stop.record_fault(stop_paths, event)
        except (OSError, ValueError) as e:
            # Defer reporting/raising until the actual child is reaped. Scenario's
            # outer finally can then restore saves even if storage is broken.
            fault_errors.append(str(e))
        finally:
            termination.require()

    def engine_status(child):
        nonlocal why, rc
        if child.returncode in (71, 72) and child.pid not in fatal_recorded:
            fatal_recorded.add(child.pid)
            if child.termination_requested:
                fault('fatal-status-origin-unknown', source=child)
                return
            rc = child.returncode
            if why == 'exited':
                why = ('GPU DEVICE LOST: engine exited 71' if rc == 71 else
                       'GPU COMPLETION/RESOURCE SAFETY FATAL: engine exited 72')
            fault('exit-%d' % rc, source=child)

    def fresh_thermal():
        nonlocal observed_sample
        observed_sample = monitor.sample()
        reason = monitor.check(observed_sample)
        if reason:
            raise SystemExit('thermal monitoring required at launch: ' + reason)
        return observed_sample

    try:
        os.makedirs(artifact_dir, exist_ok=True)
        run_triage.provenance(artifact_dir, exe, args, env)
        with open(logp, 'wb') as lf, open(memory_path, 'w', encoding='ascii') as mf:
            log_opened = True
            mf.write('seconds,private_mb,free_commit_mb\n')
            mf.flush()
            # The exclusive claim may have waited on an earlier workload that faulted.
            gpu_stop.require_clear(stop_paths)
            busy = compilers()
            if busy:
                raise SystemExit('build overlap refused at launch: %s' % busy)
            fresh_thermal()  # after provenance/expensive preflight; heartbeat is not thermal evidence
            def resume_gate():
                sample = fresh_thermal()  # creation can consume the freshness budget or change temperature
                gpu_stop.require_clear(stop_paths)
                reason = monitor.check(sample)  # revalidate age after the last gate's I/O
                if reason:
                    raise SystemExit('thermal monitoring required at resume: ' + reason)
            p = job.launch([exe] + list(args), stdout=lf, cwd=os.path.dirname(exe), env=env,
                           before_resume=resume_gate, on_resume=state.resumed)
            children.append(p)
            watched = p
            pid = p.pid
            CURRENT['pid'] = p.pid
            mine = claim_text(os.path.basename(exe), p.pid)
            with open(LOCK, 'w') as fl:
                fl.write(mine)
            if during:
                state.workers_confirmed = False
                during_thread = threading.Thread(target=during, args=(p.pid,), daemon=True)
                during_thread.start()
            try:
                interval = float(_setting(env, 'NXRS_CAPTURE_INTERVAL_S', '30') or 0)
            except ValueError:
                interval = 30.0
            if _setting(env, 'NXRS_AUTO_CAPTURE', '1') != '0' and interval > 0:
                state.workers_confirmed = False
                def auto_evidence():
                    # Five seconds gives the control pipe and first presents time to appear.
                    if capture_stop.wait(min(5.0, interval)):
                        return
                    n = 1
                    while not capture_stop.is_set():
                        diagnostic_snapshot(p.pid, artifact_dir, 'periodic-%04d' % n, stop=capture_stop)
                        n += 1
                        if capture_stop.wait(interval):
                            return
                capture_thread = threading.Thread(target=auto_evidence, name='run evidence', daemon=True)
                capture_thread.start()
            pid, seen = p.pid, 0
            last_gpu_check = time.monotonic()
            log_watch = harness_monitor.RunLog(logp)
            while True:
                while watched.poll() is None:
                    time.sleep(POLL_SECONDS)
                    if time.monotonic() >= job.deadline or job.worker.poll() is not None:
                        why = 'independent watchdog: hard deadline or stale monitor'
                        job.kill()
                        break
                    busy = compilers()
                    direct_fault = log_watch.fault()
                    if busy or direct_fault:
                        why = 'build overlap: %s' % busy if busy else direct_fault
                        fault('build-overlap' if busy else 'log-fault')
                        log_fault_recorded = bool(direct_fault)
                        break
                    if time.monotonic() - last_gpu_check >= 10:
                        last_gpu_check = time.monotonic()
                        resets = gpu_resets_since(t0)
                        if resets:
                            why = 'GPU RESET: the display driver reset at %s; game stopped (a GPU hang to fix)' % ', '.join(resets)
                            reset_recorded = True
                            fault('display-reset', resets)
                            try:
                                with open(os.path.join(EVIDENCE, 'GPU_RESETS.log'), 'a') as gl:
                                    gl.write('%s | %s | %s | %s\n' % (datetime.datetime.now().isoformat(timespec='seconds'),
                                                                     lane, os.path.basename(exe), ', '.join(resets)))
                            except OSError:
                                pass
                            break
                    if not os.path.exists(LOCK):
                        # another lane's stale-claim sweep can read the claim just before a hand-over rewrites it
                        # (the old pid is gone) and remove it after: write it again while the game runs
                        with open(LOCK, 'w') as fl:
                            fl.write(mine)
                    m = priv_mb(pid)
                    peak = max(peak, m)
                    fc = free_commit_mb()
                    if m >= PRIVATE_LIMIT_MB or fc < COMMIT_MIN_RUN_MB:
                        why = 'WATCHDOG: private %d MB, free commit %d MB' % (m, fc)
                        fault('memory-stop')
                        break
                    sample = monitor.sample()
                    frames = harness_monitor.progress(artifact_dir)
                    sample.update(private_mb=m, free_commit_mb=fc, frames=frames)
                    observed_sample = sample
                    unsafe = monitor.check(sample, m, frames)
                    if unsafe:
                        why = 'WATCHDOG: ' + unsafe
                        fault('monitor-stop')
                        break
                    if time.monotonic() - started > hard:
                        why = 'hard timeout %.0f s' % hard
                        job.kill()
                        break
                    job.heartbeat()  # only a complete successful observation renews the lease
                    mf.write('%.3f,%d,%d\n' % (time.monotonic() - started, m, fc))
                    mf.flush()
                    with open(os.path.join(artifact_dir, 'harness-thermal.jsonl'), 'a', encoding='utf-8') as tf:
                        tf.write(json.dumps(sample) + '\n')
                watched.wait(timeout=2)
                rc = watched.returncode
                engine_status(watched)
                if why != 'exited':
                    break
                lf.flush()
                nxt = successor(logp, seen)
                if not nxt or nxt[1] == pid:
                    break
                try:
                    adopted = job.adopt(nxt[1])
                except OSError:
                    why = 'refused unowned successor or unavailable status for PID %d' % nxt[1]
                    fault('successor-status-unknown')
                    break
                children.append(adopted)
                watched = adopted
                rc = None  # the root's status is not the successor's status
                seen = os.path.getsize(logp)
                pid = nxt[1]
                CURRENT['pid'] = pid
                mine_new = claim_text(os.path.basename(nxt[0]), pid)
                with open(LOCK, 'w') as fl:
                    fl.write(mine_new)
                mine = mine_new
                say('the game handed over to %s (process %d); watching it' % (os.path.basename(nxt[0]), pid))
    finally:
        capture_stop.set()
        cleanup = harness_lifecycle.stop_and_join(
            job, children, [('capture', capture_thread), ('scenario', during_thread)], state)
        CURRENT['pid'] = None
        # Persist final observations before releasing the exclusive claim, also on
        # exceptional exits. A refused prelaunch must not query the GPU here.
        try:
            try:
                if watched is not None:
                    rc = watched.returncode
                    for child in children:
                        engine_status(child)
            finally:
                if log_opened:
                    # Attempt on every exit, even if terminal-status persistence/termination failed.
                    try:
                        final_fault = (log_watch or harness_monitor.RunLog(logp)).fault()
                    except (OSError, ValueError) as exc:
                        final_fault = 'final log unavailable: %s' % exc
                    if final_fault and not log_fault_recorded:
                        if why == 'exited':
                            why = final_fault
                        fault('final-log-fault')
            if state.launched:
                if not reset_recorded:
                    late = gpu_resets_since(t0)
                    if late:
                        why = 'GPU RESET: the display driver reset at %s (exit %s)' % (', '.join(late), rc)
                        fault('late-display-reset', late)
                        try:
                            with open(os.path.join(EVIDENCE, 'GPU_RESETS.log'), 'a') as gl:
                                gl.write('%s | %s | %s | %s\n' % (datetime.datetime.now().isoformat(timespec='seconds'),
                                                                 lane, os.path.basename(exe), ', '.join(late)))
                        except OSError:
                            pass
        finally:
            try:
                if os.path.exists(LOCK):
                    with open(LOCK) as fl:
                        owned_claim = fl.read() == mine
                    if owned_claim:
                        os.remove(LOCK)
                for error in fault_errors:
                    say(error)
            finally:
                cleanup.require()
    if why == 'exited' and rc == 124:
        why = 'independent watchdog: hard deadline or stale monitor'
    r = dict(why=why, exit=rc, peak_mb=peak, seconds=round(time.monotonic() - started, 1))
    if fault_errors:
        r['gpu_stop_errors'] = fault_errors
    try:
        observability.generate(artifact_dir, r)
    except Exception as e:
        say('diagnostic summary failed: %s: %s' % (type(e).__name__, e))
    if fault_errors:
        raise gpu_stop.PersistenceError('; '.join(fault_errors))
    say('run done: %s, exit %s, peak private %d MB; claim released' % (why, rc, peak))
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
