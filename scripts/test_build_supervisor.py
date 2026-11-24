"""Build supervision only: real idle jobs, synthetic sensors and temporary release directories."""
import contextlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import build_supervisor as bs
import harness_job as hj
import harness_monitor as hm

FIXTURE = str(Path(__file__).with_name('harness_idle_fixture.py'))
BUILD_RUN = bs.BuildRun


class BuildTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.runs, self.calls, self.shift = [], 0, 0
        self.override = None
        self.stack = contextlib.ExitStack()
        self.addCleanup(self.stack.close)
        self.stack.enter_context(mock.patch.dict(os.environ, RUSTFLAGS='', CARGO_ENCODED_RUSTFLAGS=''))
        self.stack.enter_context(mock.patch.object(hj, 'free_commit_mb', return_value=50000))
        self.stack.enter_context(contextlib.redirect_stdout(io.StringIO()))
        self.addCleanup(self.reap)

    def reap(self):
        for run in self.runs:
            if not run.job.closed:
                run.close()
            for job in [*run.retired_jobs, run.job]:
                self.assertIsNotNone(job.worker.poll())
                for child in job.children:
                    self.assertIsNone(child.handle)
                    handle = hj.open_process(0x100000, False, child.pid)
                    if handle:
                        try:
                            self.assertEqual(hj.wait(handle, 1000), 0)
                        finally:
                            hj.close(handle)

    def sample(self):
        self.calls += 1
        now = time.monotonic()+self.shift
        sample = {k: hm.reading(v, 'synthetic CPU-idle build test', now, 'ok') for k, v in
                  dict(cpu_c=40, gpu_c=40, cpu_load=1, gpu_load=1, vram_mb=100).items()}
        sample['vram_total_mb'] = 8000
        if self.override:
            self.override(sample)
        return sample

    def run_fixture(self, folder=None, seconds=2):
        run = BUILD_RUN(folder or self.root, seconds, hm.Monitor(provider=self.sample,
                        clock=lambda: time.monotonic()+self.shift), overlap=lambda job: [], poll_seconds=.02)
        self.runs.append(run)
        return run

    def test_successful_cpu_exit_has_positive_cleanup_and_no_game_progress_requirement(self):
        run = self.run_fixture()
        self.shift = 31
        run.execute('idle', [sys.executable, FIXTURE, 'exit-code', '0'])
        run.close()
        self.assertTrue(run.state.cleanup_confirmed)

    def test_lingering_owned_build_helper_stops_before_separate_verification_job(self):
        run = self.run_fixture()
        build_job, deadline, monitor = run.job, run.deadline, run.monitor
        run.execute('production-build', [sys.executable, FIXTURE, 'tree-exit'])
        self.assertTrue(build_job.closed)
        self.assertTrue(run.state.cleanup_confirmed)
        leaf = int((self.root / 'production-build.log').read_text().split()[1])
        handle = hj.open_process(0x100000, False, leaf)
        if handle:
            try:
                self.assertEqual(hj.wait(handle, 0), 0)
            finally:
                hj.close(handle)
        run.execute('verify', [sys.executable, FIXTURE, 'exit-code', '0'])
        self.assertIsNot(run.job, build_job)
        self.assertEqual(run.retired_jobs, [build_job])
        self.assertIs(run.monitor, monitor)
        self.assertLessEqual(run.job.deadline, deadline)
        run.close()
        self.assertTrue(run.state.cleanup_confirmed)

    def test_uncertain_completed_phase_cannot_start_verification(self):
        run = self.run_fixture()
        close = run.job.close
        def uncertain():
            close()
            raise hj.CleanupError('injected uncertain completion')
        with mock.patch.object(run.job, 'close', side_effect=uncertain):
            with self.assertRaises(hj.CleanupError):
                run.execute('production-build', [sys.executable, FIXTURE, 'exit-code', '0'])
        with self.assertRaisesRegex(hj.CleanupError, 'previous build phase'):
            run.execute('verify', [sys.executable, FIXTURE, 'exit-code', '0'])
        self.assertFalse(run.retired_jobs)
        self.assertFalse(run.state.cleanup_confirmed)

    def test_hot_suspended_boundary_never_executes_child(self):
        self.override = lambda s: s['cpu_c'].update(value=90 if self.calls >= 2 else 40)
        run = self.run_fixture()
        with self.assertRaisesRegex(RuntimeError, 'thermal refusal'):
            run.execute('idle', [sys.executable, FIXTURE, 'fatal-text', 'must not execute'])
        run.close()
        self.assertFalse(run.state.launched)
        self.assertEqual((self.root / 'idle.log').read_bytes(), b'')

    def test_age_rechecked_after_final_evidence_io_before_resume(self):
        run = self.run_fixture()
        write = bs.write_json
        def delay(path, value):
            write(path, value)
            if run.job.children:
                self.shift += 13
        with mock.patch.object(bs, 'write_json', side_effect=delay):
            with self.assertRaisesRegex(RuntimeError, 'late acquisition-age'):
                run.execute('idle', [sys.executable, FIXTURE, 'fatal-text', 'must not execute'])
        run.close()
        self.assertFalse(run.state.launched)
        self.assertEqual((self.root / 'idle.log').read_bytes(), b'')

    def invoke_main(self, fault=None):
        def factory(folder, jobs=4):
            self.assertEqual(jobs, 4)
            run = self.run_fixture(folder)
            execute = run.execute
            def idle(phase, command):
                command = [sys.executable, FIXTURE, 'leaf' if phase == 'production-build' else 'exit-code']
                if phase != 'production-build':
                    command.append('0')
                return execute(phase, command)
            run.execute = idle
            return run
        def sample_fault(sample):
            run = self.runs[-1]
            if (run.phase == 'production-build' and run.child is not None
                    and run.child.poll() is None):
                fault(sample)
        self.override = sample_fault
        folder = self.root / 'attempt'
        with mock.patch.object(bs, 'BuildRun', side_effect=factory), \
                mock.patch.object(sys, 'argv', ['build_supervisor.py', '--evidence', str(folder)]):
            self.assertEqual(bs.main(), 1)
        return folder, json.loads((folder / 'result.json').read_text())

    def test_live_hot_fault_kills_then_persists_build_specific_stop(self):
        folder, result = self.invoke_main(lambda s: s['cpu_c'].update(value=90))
        self.assertTrue((folder / 'BUILD-STOP.json').is_file())
        self.assertFalse(result['ok'])
        self.assertTrue(result['cleanup_confirmed'])
        self.assertEqual(result['exit'], 124)
        self.assertIn('thermal refusal', result['error'])

    def test_stale_sensor_kills_and_persists(self):
        folder, result = self.invoke_main(lambda s: s['cpu_c'].update(at=time.monotonic()-13))
        self.assertEqual(result['exit'], 124)
        self.assertIn('stale', result['error'])
        self.assertTrue(result['cleanup_confirmed'])

    def test_protective_flag_kills_below_temperature_cap(self):
        folder, result = self.invoke_main(lambda s: s['cpu_c'].update(alerts=['thermal throttling']))
        self.assertEqual(result['exit'], 124)
        self.assertIn('protective flag', result['error'])

    def test_nonfinite_sensor_cannot_prevent_durable_stop_evidence(self):
        folder, result = self.invoke_main(lambda s: s['cpu_c'].update(value=float('inf')))
        self.assertEqual(result['exit'], 124)
        evidence = json.loads((folder / 'BUILD-STOP.json').read_text())
        self.assertEqual(evidence['sample']['cpu_c']['value'], 'nonfinite:inf')

    def test_external_compiler_is_refused_without_starting_child(self):
        run = self.run_fixture()
        run.overlap = lambda job: [{'Name': 'rustc.exe', 'ProcessId': 999}]
        with self.assertRaisesRegex(RuntimeError, 'external workload overlap'):
            run.execute('idle', [sys.executable, FIXTURE, 'leaf'])
        run.close()
        self.assertFalse(run.job.children)

    def test_owned_compiler_inventory_is_not_rejected(self):
        run = self.run_fixture()
        with (self.root / 'compiler.log').open('wb') as stream:
            child = run.job.launch([sys.executable, FIXTURE, 'leaf'], stream)
        with mock.patch.object(hm, 'powershell', return_value=json.dumps([
                dict(Name='rustc.exe', ProcessId=child.pid, ExecutablePath='C:\\rustc.exe'),
                dict(Name='cargo.exe', ProcessId=os.getpid(), ExecutablePath='C:\\cargo.exe')])):
            self.assertEqual([p['ProcessId'] for p in bs.external_workloads(run.job)], [os.getpid()])
        run.close()

    def test_exited_owned_worker_in_delayed_snapshot_is_not_an_external_compiler(self):
        run = self.run_fixture()
        with (self.root / 'compiler.log').open('wb') as stream:
            child = run.job.launch([sys.executable, FIXTURE, 'exit-code', '0'], stream)
        child.wait(timeout=1)
        with mock.patch.object(hm, 'powershell', return_value=json.dumps([
                dict(Name='cl.exe', ProcessId=child.pid, ExecutablePath='C:\\cl.exe')])):
            self.assertEqual(bs.external_workloads(run.job), [])
        run.close()

    def test_vanished_pid_is_distinct_from_access_denied(self):
        job = mock.Mock()
        with mock.patch.object(hj, 'open_process', return_value=0):
            with mock.patch.object(hj.C, 'get_last_error', return_value=87):
                self.assertFalse(bs.live_external_process(job, 999))
            with mock.patch.object(hj.C, 'get_last_error', return_value=5):
                with self.assertRaises(OSError):
                    bs.live_external_process(job, 999)

    def exit_during_membership(self, query_result):
        run = self.run_fixture()
        with (self.root / 'racing-compiler.log').open('wb') as stream:
            child = run.job.launch([sys.executable, FIXTURE, 'leaf'], stream)
        queried_handles = []
        def exit_at_query(handle, job_handle, member):
            queried_handles.append(handle)
            self.assertIsNone(child.poll())
            run.job.kill()
            self.assertEqual(hj.wait(handle, 1000), 0)
            return query_result  # member stays false; model either query failure or non-member
        with mock.patch.object(hj, 'in_job', side_effect=exit_at_query), \
                mock.patch.object(hj.C, 'get_last_error', return_value=5):
            self.assertFalse(bs.live_external_process(run.job, child.pid))
        self.assertEqual(len(queried_handles), 1)
        run.close()

    def test_exit_during_membership_nonmember_result_is_not_live_external(self):
        self.exit_during_membership(1)

    def test_exit_during_membership_query_failure_is_not_live_external(self):
        self.exit_during_membership(0)

    def test_membership_query_failure_for_still_live_process_refuses(self):
        run = self.run_fixture()
        with mock.patch.object(hj, 'in_job', return_value=0), \
                mock.patch.object(hj.C, 'get_last_error', return_value=5):
            with self.assertRaises(OSError) as error:
                bs.live_external_process(run.job, os.getpid())
        self.assertEqual(error.exception.winerror, 5)
        run.close()

    def test_independent_deadline_terminates_during_blocked_sensor(self):
        run = self.run_fixture(seconds=.4)
        def block(sample):
            if self.calls >= 3:
                time.sleep(.5)
        self.override = block
        with self.assertRaisesRegex(RuntimeError, 'watchdog'):
            run.execute('idle', [sys.executable, FIXTURE, 'leaf'])
        run.close()
        self.assertEqual(run.child.returncode, 124)

    def test_uncertain_termination_never_reports_successful_cleanup(self):
        terminate = hj.terminate_job
        failed = []
        def error(handle, code):
            result = terminate(handle, code)
            if self.runs and self.runs[-1].phase == 'production-build' and not failed:
                failed.append(True)
                return 0
            return result
        with mock.patch.object(hj, 'terminate_job', side_effect=error):
            folder, result = self.invoke_main(lambda s: s['cpu_c'].update(value=90))
        self.assertFalse(result['cleanup_confirmed'])
        self.assertIn('cleanup unconfirmed', result['error'])

    def test_memory_limit_is_not_relaxed_for_compilation(self):
        run = self.run_fixture()
        with mock.patch.object(run.job, 'private_mb', return_value=20000):
            with self.assertRaisesRegex(RuntimeError, 'memory refusal'):
                run.execute('idle', [sys.executable, FIXTURE, 'leaf'])
        run.close()
        self.assertFalse(run.job.children)

    def test_interrupted_publication_restores_old_release_only_from_owned_paths(self):
        dist = self.root / 'dist'
        live, previous, stage = (dist / name for name in ('nxrs', 'nxrs-previous-test', 'nxrs-stage-test'))
        live.mkdir(parents=True)
        previous.mkdir()
        (live / 'sentinel').write_text('candidate')
        (previous / 'sentinel').write_text('original')
        (self.root / 'publication.json').write_text(json.dumps(dict(
            live=str(live), previous=str(previous), stage=str(stage))))
        with mock.patch.object(bs, 'ROOT', self.root):
            self.assertIn('previous release restored', bs.rollback(self.root))
        self.assertEqual((live / 'sentinel').read_text(), 'original')
        self.assertEqual((stage / 'sentinel').read_text(), 'candidate')


if __name__ == '__main__':
    unittest.main()
