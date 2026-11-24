"""H1-H4 counterexamples: real contained CPU-idle processes and temporary saves.

All telemetry/preflight sources are injected. No game, graphics API or real save is used.
"""
from contextlib import ExitStack
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import time
import unittest
from types import SimpleNamespace
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import gpu_stop
import harness_job as hj
import harness_lifecycle as life
import harness_monitor as hm
import play_scenario as ps
import run_game as rg

FIXTURE = str(Path(__file__).with_name('harness_idle_fixture.py'))


class Integration(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.run = self.root / 'run'
        self.run.mkdir()
        self.save = self.root / 'data/save'
        self.save.mkdir(parents=True)
        self.slot = self.save / 'slot'
        self.slot.write_bytes(b'original')
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.jobs, self.workers, self.states = [], [], []
        self.addCleanup(self.resources_released)
        self.calls, self.clock_shift = 0, 0
        self.sample_override = None
        self.patch_dict = self.stack.enter_context(mock.patch.dict(os.environ, NXRS_GPU_STOP_PATH=''))
        self.patch(gpu_stop, 'SHARED_STOP', self.root / 'stop.json')
        self.patch(gpu_stop, '_FAILED_WRITES', {})
        self.patch(rg, 'EVIDENCE', str(self.root))
        self.patch(rg, 'LEGACY_LOCK', str(self.root / 'GAME_LOCK'))
        self.patch(rg, 'CURRENT', {'pid': None, 'job': None, 'stop': None})
        self.patch(rg, 'POLL_SECONDS', .02)
        for name, value in (('gpu_resets_since', []), ('compilers', []), ('other_games', []),
                            ('free_commit_mb', 50000), ('priv_mb', 100)):
            self.stack.enter_context(mock.patch.object(rg, name, return_value=value))
        self.stack.enter_context(mock.patch.object(rg.run_triage, 'provenance'))
        self.stack.enter_context(mock.patch.object(rg.observability, 'generate'))
        monitor_type = hm.Monitor
        self.monitor = monitor_type(provider=self.sample, clock=self.clock)
        self.stack.enter_context(mock.patch.object(hm, 'Monitor', return_value=self.monitor))
        job_type = hj.Job
        def job(*args, **kwargs):
            result = job_type(*args, **kwargs)
            self.jobs.append(result)
            return result
        self.stack.enter_context(mock.patch.object(hj, 'Job', side_effect=job))
        join = life.stop_and_join
        def track(job, children, workers, state):
            self.workers.extend(t for _, t in workers if t is not None)
            return join(job, children, workers, state)
        self.stack.enter_context(mock.patch.object(life, 'stop_and_join', side_effect=track))

    def patch(self, target, name, value):
        return self.stack.enter_context(mock.patch.object(target, name, value))

    def clock(self):
        return time.monotonic() + self.clock_shift

    def sample(self):
        self.calls += 1
        result = {key: hm.reading(value, 'injected CPU-only test', self.clock(), 'ok')
                  for key, value in dict(cpu_c=40, gpu_c=35, cpu_load=1, gpu_load=1, vram_mb=100).items()}
        result['vram_total_mb'] = 8000
        return self.sample_override(self.calls, result) if self.sample_override else result

    def resources_released(self):
        for thread in self.workers:
            thread.join(timeout=1)
            self.assertFalse(thread.is_alive(), 'callback left alive')
        for job in self.jobs:
            if not job.closed:
                job.close()
            self.assertIsNotNone(job.worker.poll(), 'watchdog left alive')
            for child in job.children:
                handle = hj.open_process(0x100000, False, child.pid)
                if handle:
                    try:
                        self.assertEqual(hj.wait(handle, 1000), 0, 'idle child left alive')
                    finally:
                        hj.close(handle)

    def guarded(self, *args, during=None, env=None, state=None):
        return rg.run(sys.executable, str(self.run / 'run.log'), 1.5, [FIXTURE, *args],
                      env=dict(os.environ, NXRS_AUTO_CAPTURE='0', NXRS_LANE='agent',
                               NXRS_ARTIFACT_DIR=str(self.run), **(env or {})),
                      during=during, say=lambda text: None, state=state)

    def scenario(self, during=None, capture=False):
        real_run = rg.run
        def idle_run(exe, log, hard, args, **kwargs):
            self.states.append(kwargs['state'])
            kwargs['during'] = during
            if capture:
                kwargs['env'].update(NXRS_AUTO_CAPTURE='1', NXRS_CAPTURE_INTERVAL_S='.01')
            return real_run(sys.executable, log, 1.5, [FIXTURE, 'write-save', str(self.slot)], **kwargs)
        options = SimpleNamespace(data=str(self.save.parent), env=['NXRS_AUTO_CAPTURE=0'], option=[],
                                  seconds=1.5, capture_interval=0, diagnostics='baseline', window_value=None)
        with mock.patch.object(rg, 'run', side_effect=idle_run):
            ps.scenario(options, dict(save=True, seconds=1.5, steps=[]), self.run, Path(sys.executable))

    def faults(self):
        return json.loads(gpu_stop.SHARED_STOP.read_text())['faults']

    def assert_backup_withheld(self):
        self.assertEqual((self.run / 'save-before/slot').read_bytes(), b'original')
        self.assertEqual(self.slot.read_bytes(), b'child write')
        self.assertFalse((self.run / 'save-restoration.json').exists())
        self.assertTrue(self.states[-1].launched)
        self.assertFalse(self.states[-1].cleanup_confirmed)


class CleanupProofTests(Integration):
    def blocked_worker(self, capture):
        release, finished = threading.Event(), threading.Event()
        def blocked(*args, **kwargs):
            try:
                release.wait(1)
            finally:
                finished.set()
        self.patch(life, 'WORKER_JOIN_SECONDS', .03)
        if capture:
            self.patch(rg, 'diagnostic_snapshot', blocked)
        try:
            with self.assertRaises(hj.CleanupError):
                self.scenario(during=None if capture else blocked, capture=capture)
            self.assert_backup_withheld()
            self.assertEqual(self.faults()[-1]['kind'], 'cleanup-uncertain')
            # Even a new artifact folder cannot inspect the save after uncertain cleanup.
            self.run = self.root / 'next-run'
            self.run.mkdir()
            with mock.patch.object(ps, 'SaveTransaction') as transaction:
                with self.assertRaisesRegex(SystemExit, 'GPU STOP'):
                    self.scenario()
                transaction.assert_not_called()
        finally:
            release.set()
            self.assertTrue(finished.wait(1))

    def test_actual_unjoined_scenario_callback_withholds_restore(self):
        self.blocked_worker(False)

    def test_actual_unjoined_capture_callback_withholds_restore(self):
        self.blocked_worker(True)

    def native_error(self, operation):
        original = getattr(hj, operation)
        failed = []
        def inject(*args):
            target = self.jobs[0] if self.jobs else None
            handles = [c.handle for c in target.children] if target else []
            selected = (target is not None and self.slot.read_bytes() == b'child write' and
                        ((operation == 'terminate_job' and args[0] == target.handle) or
                         (operation == 'query_job' and args[0] == target.handle and args[1] == 1) or
                         (operation == 'wait' and args[0] in handles) or
                         (operation == 'close' and args[0] in handles)))
            if selected and not failed:
                failed.append(True)
                # Real termination still runs; the injected reported failure must remain sticky.
                if operation != 'close':
                    original(*args)
                return 0xffffffff if operation == 'wait' else 0
            return original(*args)
        with mock.patch.object(hj, operation, side_effect=inject):
            with self.assertRaises(hj.CleanupError):
                self.scenario()
        self.assertTrue(failed)
        self.assert_backup_withheld()
        self.assertTrue(all(c.handle is None for c in self.jobs[0].children))

    def test_termination_error_remains_uncertain_after_retry_succeeds(self):
        self.native_error('terminate_job')

    def test_plain_fault_termination_error_cannot_be_erased_by_later_cleanup(self):
        factory = hj.Job.side_effect
        failed = []
        def job(*args, **kwargs):
            result = factory(*args, **kwargs)
            kill = result.kill
            def fail_once():
                kill()
                if not failed:
                    failed.append(True)
                    raise OSError('injected termination wrapper error')
            result.kill = fail_once
            return result
        hj.Job.side_effect = job
        def low_commit():
            until = time.monotonic() + .5
            while self.slot.read_bytes() != b'child write' and time.monotonic() < until:
                time.sleep(.01)
            return 1
        rg.free_commit_mb.side_effect = [50000]
        # Switch to the failing in-run observation at the real resume notification.
        resumed = life.RunState.resumed
        def mark(state):
            resumed(state)
            rg.free_commit_mb.side_effect = low_commit
        with mock.patch.object(life.RunState, 'resumed', mark):
            with self.assertRaises(hj.CleanupError):
                self.scenario()
        self.assertTrue(failed)
        self.assertTrue(self.jobs[0].cleanup_confirmed)  # native retry did succeed
        self.assertTrue(self.states[-1].uncertain)       # run-level uncertainty still vetoes restore
        self.assert_backup_withheld()

    def test_job_query_error_withholds_restore(self):
        self.native_error('query_job')

    def test_process_wait_error_withholds_restore(self):
        self.native_error('wait')

    def test_child_close_error_withholds_restore_and_retries_handle_cleanup(self):
        self.native_error('close')

    def test_plain_callback_join_error_is_cleanup_error(self):
        original = threading.Thread.join
        def fail_join(thread, *args, **kwargs):
            if thread in self.workers:
                raise OSError('injected join error')
            return original(thread, *args, **kwargs)
        with mock.patch.object(threading.Thread, 'join', fail_join):
            with self.assertRaises(hj.CleanupError):
                self.scenario(during=lambda pid: None)
        self.assert_backup_withheld()

    def test_probe_closes_child_even_when_job_close_raises_plain_error(self):
        child = mock.Mock()
        child.wait.return_value = 0
        job = mock.Mock(deadline=time.monotonic() + 5)
        job.launch.return_value = child
        job.close.side_effect = OSError('injected close error')
        with mock.patch.object(hj, 'Job', return_value=job):
            with self.assertRaises(hj.CleanupError):
                hj.bounded_run(['never-executed'])
        child.close.assert_called_once()


class LeaseAndResumeTests(Integration):
    def test_recursive_acquisition_requires_explicit_borrow(self):
        with hj.MachineLease() as lease:
            with lease.borrow():
                self.assertTrue(lease.held)
            with self.assertRaisesRegex(SystemExit, 'explicit borrowing'):
                with hj.MachineLease():
                    self.fail('implicit recursive acquisition was allowed')
        with self.assertRaisesRegex(RuntimeError, 'not held'):
            with lease.borrow():
                self.fail('released lease was borrowed')

    def competing_lease(self):
        outcomes = []
        def compete():
            try:
                with hj.MachineLease():
                    outcomes.append('acquired')
            except SystemExit:
                outcomes.append('refused')
        thread = threading.Thread(target=compete)
        thread.start()
        thread.join(timeout=1)
        self.assertFalse(thread.is_alive())
        return outcomes

    def test_second_scenario_cannot_inspect_backup_or_finish_existing_save(self):
        identity = self.slot.stat().st_ino
        outcomes = []
        def second():
            try:
                self.scenario()
            except SystemExit:
                outcomes.append('refused')
        with hj.MachineLease(), mock.patch.object(ps, 'SaveTransaction') as transaction:
            thread = threading.Thread(target=second)
            thread.start()
            thread.join(timeout=1)
            self.assertFalse(thread.is_alive())
            self.assertEqual(outcomes, ['refused'])
            transaction.assert_not_called()
        self.assertEqual(self.slot.stat().st_ino, identity)
        self.assertEqual(self.slot.read_bytes(), b'original')

    def test_same_lease_is_held_through_actual_save_finish(self):
        finish = ps.SaveTransaction.finish
        def restore(tx):
            self.assertEqual(self.competing_lease(), ['refused'])
            result = finish(tx)
            self.assertEqual(self.competing_lease(), ['refused'])
            return result
        with mock.patch.object(ps.SaveTransaction, 'finish', restore):
            self.scenario()
        self.assertEqual(self.competing_lease(), ['acquired'])
        self.assertEqual(self.slot.read_bytes(), b'original')
        self.assertTrue(self.states[-1].cleanup_confirmed)

    def refusal_does_not_replace(self, latch):
        identity = (self.save.stat().st_ino, self.slot.stat().st_ino, self.slot.stat().st_mtime_ns)
        if latch:
            gpu_stop.record_fault((gpu_stop.SHARED_STOP,), dict(kind='fixture-hold'))
        else:
            def unknown(n, sample):
                sample['cpu_c'] = hm.reading(detail='missing CPU sensor')
                return sample
            self.sample_override = unknown
        with mock.patch.object(ps.SaveTransaction, 'finish') as finish:
            with self.assertRaises(SystemExit):
                self.scenario()
            finish.assert_not_called()
        self.assertEqual(identity, (self.save.stat().st_ino, self.slot.stat().st_ino,
                                    self.slot.stat().st_mtime_ns))
        self.assertEqual(self.slot.read_bytes(), b'original')
        self.assertFalse(self.jobs)

    def test_prelaunch_latch_refusal_does_not_replace_save(self):
        self.refusal_does_not_replace(True)

    def test_prelaunch_missing_sensor_refusal_does_not_replace_save(self):
        self.refusal_does_not_replace(False)

    def thermal_gate(self, at_resume, stale):
        reached = []
        def samples(n, sample):
            if n >= (3 if at_resume else 2):
                reached.append(n)
                if stale:
                    sample['cpu_c']['at'] -= 13
                else:
                    sample['cpu_c']['value'] = 90
            return sample
        self.sample_override = samples
        # Model slow preflight without sleeping/spinning: provider time advances by 13 seconds.
        rg.run_triage.provenance.side_effect = lambda *a: setattr(self, 'clock_shift', 13)
        state = life.RunState()
        with self.assertRaisesRegex(SystemExit, 'thermal monitoring required'):
            self.guarded('write-save', str(self.slot), state=state)
        self.assertTrue(reached)
        self.assertEqual(self.slot.read_bytes(), b'original')
        self.assertFalse(state.launched)
        self.assertEqual(len(self.jobs[0].children), 1 if at_resume else 0)

    def test_hot_after_delayed_preflight_prevents_creation(self):
        self.thermal_gate(False, False)

    def test_stale_after_delayed_preflight_prevents_creation(self):
        self.thermal_gate(False, True)

    def test_hot_at_suspended_resume_prevents_any_child_code(self):
        self.thermal_gate(True, False)

    def test_stale_at_suspended_resume_prevents_any_child_code(self):
        self.thermal_gate(True, True)

    def test_fresh_resume_sample_is_rechecked_after_last_latch_io(self):
        gate = gpu_stop.require_clear
        def delayed(paths):
            gate(paths)
            if self.calls == 3:
                self.clock_shift += 13
        state = life.RunState()
        with mock.patch.object(gpu_stop, 'require_clear', side_effect=delayed):
            with self.assertRaisesRegex(SystemExit, 'at resume: cpu_c stale'):
                self.guarded('write-save', str(self.slot), state=state)
        self.assertFalse(state.launched)
        self.assertEqual(self.slot.read_bytes(), b'original')


class FatalStatusTests(Integration):
    def test_immediate_root72_without_log_or_event_latches_before_completed_intent(self):
        persist = gpu_stop.record_fault
        def record(paths, event):
            self.assertEqual(json.loads((self.run / 'run-intent.json').read_text())['state'], 'intent')
            return persist(paths, event)
        with mock.patch.object(gpu_stop, 'record_fault', side_effect=record):
            result = self.guarded('exit-code', '72')
        self.assertEqual(result['exit'], 72)
        self.assertEqual(self.faults()[0]['kind'], 'exit-72')
        self.assertEqual(self.faults()[0]['exit_pid'], self.jobs[0].children[0].pid)
        self.assertEqual((self.run / 'run.log').read_bytes(), b'')
        self.assertEqual(json.loads((self.run / 'run-intent.json').read_text())['state'], 'completed')
        with self.assertRaisesRegex(SystemExit, 'GPU STOP'):
            self.guarded('exit-code', '0')

    def test_final_log_watchdog_and_device_lost_spellings_with_fast_exit(self):
        # Force the running-loop predicate to observe already-exited roots: only final scan can catch these.
        launch = hj.Job  # factory already installed; wrap each native instance launch below
        factory = launch.side_effect
        def job(*args, **kwargs):
            result = factory(*args, **kwargs)
            native_launch = result.launch
            def immediate(*a, **kw):
                child = native_launch(*a, **kw)
                child.wait(timeout=1)
                return child
            result.launch = immediate
            return result
        launch.side_effect = job
        result = self.guarded('fatal-text', 'GPU completion watchdog: Driver(ERROR_DEVICE_LOST)')
        self.assertEqual(result['exit'], 0)
        self.assertEqual(self.faults()[0]['kind'], 'final-log-fault')
        self.assertIn('GPU completion watchdog', result['why'])
        self.assertIsNotNone(hm.RunLog.FAULT.search('Driver(ERROR_DEVICE_LOST)'))

    def successor_status(self, code):
        result = self.guarded('handover', str(code))
        root, successor = self.jobs[0].children
        self.assertEqual(root.returncode, 0)
        self.assertEqual(successor.returncode, code)
        self.assertEqual(result['exit'], code)
        self.assertEqual(self.faults()[0]['kind'], 'exit-%d' % code)
        self.assertEqual(self.faults()[0]['exit_pid'], successor.pid)
        self.assertNotEqual(root.pid, successor.pid)

    def test_actual_successor72_after_root0_without_reset(self):
        self.successor_status(72)

    def test_actual_successor71_after_root0_without_reset(self):
        self.successor_status(71)

    def test_adoption_never_grants_authority_to_an_unowned_pid(self):
        job = hj.Job(1)
        with self.assertRaisesRegex(OSError, 'unowned successor'):
            job.adopt(os.getpid())
        self.assertFalse(job.children)
        job.close()

    def test_harness_stop_is_not_relabelled_engine_origin_from_status72_alone(self):
        rg.free_commit_mb.side_effect = [50000, 1]
        get_exit = hj._winapi.GetExitCodeProcess
        def ambiguous(handle):
            actual = get_exit(handle)
            return 72 if actual == 124 else actual
        with mock.patch.object(hj._winapi, 'GetExitCodeProcess', side_effect=ambiguous):
            result = self.guarded('leaf')
        self.assertIn('WATCHDOG: private', result['why'])
        kinds = [event['kind'] for event in self.faults()]
        self.assertIn('fatal-status-origin-unknown', kinds)
        self.assertNotIn('exit-72', kinds)


class RetainedHelperTests(Integration):
    def test_confirmed_closed_child_repeated_wait_preserves_zero_and_nonzero_exit(self):
        for code in (0, 37):
            with self.subTest(code=code), tempfile.TemporaryFile() as output:
                job = hj.Job(1.5)
                child = job.launch([sys.executable, FIXTURE, 'exit-code', str(code)], output)
                # Let Job.close obtain/cache the actual terminal status, rather than seeding it.
                self.assertEqual(hj.wait(child.handle, 1000), 0)
                self.assertIsNone(child.returncode)
                self.assertTrue(job.close())
                self.assertIsNone(child.handle)
                self.assertEqual(child.returncode, code)
                with mock.patch.object(hj, 'wait', side_effect=AssertionError('closed handle wait')), \
                        mock.patch.object(hj, 'close', side_effect=AssertionError('double close')):
                    for _ in range(2):
                        self.assertEqual(child.wait(timeout=0), code)
                        child.close()
                    self.assertTrue(job.close())
                self.assertTrue(job.cleanup_confirmed)
                self.assertFalse(job.cleanup_errors)

    def test_missing_handle_and_missing_status_remain_uncertain(self):
        job = hj.Job(1)
        child = hj.Child(None, 0, job)
        with mock.patch.object(hj, 'wait') as native_wait:
            with self.assertRaises(hj.CleanupError):
                child.wait()
        with self.assertRaises(hj.CleanupError):
            job.close()
        native_wait.assert_not_called()
        self.assertIsNone(child.returncode)
        self.assertFalse(job.cleanup_confirmed)
        self.assertTrue(job.cleanup_errors)

    def test_pm_like_helper_finalizer_restores_save_after_confirmed_job_close(self):
        helpers, cached_waits = [], []
        self.patch(ps, 'PM', [])
        def start_idle_helper(pid):
            with (self.run / 'idle-helper.log').open('wb') as output:
                child = rg.CURRENT['job'].launch([sys.executable, FIXTURE, 'leaf'], output)
            helpers.append(child)
            ps.PM.append(child)  # the actual scenario finalizer owns this second reference
        wait = hj.Child.wait
        def observe_wait(child, *args, **kwargs):
            if child in helpers and child.handle is None:
                self.assertTrue(child.job.cleanup_confirmed)
                cached_waits.append(child.returncode)
            return wait(child, *args, **kwargs)
        finish = ps.SaveTransaction.finish
        def restore(tx):
            self.assertEqual(len(helpers), 1)
            self.assertEqual(cached_waits, [124])  # the idle helper was stopped by its job
            self.assertTrue(self.states[-1].cleanup_confirmed)
            self.assertEqual(LeaseAndResumeTests.competing_lease(self), ['refused'])
            return finish(tx)
        with mock.patch.object(hj.Child, 'wait', observe_wait), \
                mock.patch.object(ps.SaveTransaction, 'finish', restore):
            self.scenario(during=start_idle_helper)
        self.assertEqual(self.slot.read_bytes(), b'original')
        self.assertEqual((self.run / 'save-after/slot').read_bytes(), b'child write')
        self.assertTrue(json.loads((self.run / 'save-restoration.json').read_text())['restored'])
        self.assertTrue(self.states[-1].cleanup_confirmed)
        self.assertFalse(ps.PM)
        self.assertIsNone(helpers[0].handle)
        self.assertFalse(self.jobs[0].cleanup_errors)


if __name__ == '__main__':
    unittest.main()
