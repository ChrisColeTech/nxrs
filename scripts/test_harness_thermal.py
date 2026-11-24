"""Bounded CPU-only failures; no game, graphics API, real library or save accesses."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import harness_job as hj
import harness_monitor as hm
import gamectl
import run_game

FIXTURE = str(Path(__file__).with_name('harness_idle_fixture.py'))


def sample(at=0, **values):
    defaults = dict(cpu_c=40, gpu_c=35, cpu_load=100, gpu_load=100, vram_mb=500)
    defaults.update(values)
    return dict({k: hm.reading(v, 'injected CPU fixture', at, 'ok') for k, v in defaults.items()},
                vram_total_mb=8000)


class MonitorTests(unittest.TestCase):
    def setUp(self):
        self.now = 0
        self.monitor = hm.Monitor(clock=lambda: self.now)

    def test_absent_stale_hot_and_nan_refuse(self):
        for values, expected in (({}, 'unknown'), (sample(-20), 'stale'),
                                 (sample(cpu_c=86), 'cpu_c cap'),
                                 (sample(gpu_c=81), 'gpu_c cap'),
                                 (sample(cpu_c=float('nan')), 'unknown')):
            with self.subTest(expected=expected):
                self.assertIn(expected, self.monitor.check(values))

    def test_cached_cpu_value_without_acquisition_timestamp_is_unknown(self):
        values = sample()
        values['cpu_c'] = hm.reading(40, 'root/LibreHardwareMonitor', detail='no timestamp')
        self.assertIn('unknown', self.monitor.check(values))

    def test_utilization_alone_is_not_a_leak(self):
        for at in (0, 15, 31, 45):
            self.now = at
            self.assertIsNone(self.monitor.check(sample(at), 1000, frames=at+1))

    def test_stalled_frames_and_zero_fps_do_not_renew_progress(self):
        self.monitor.check(sample(), 1000, frames=0)
        self.now = 31
        self.assertIn('progress absent/stalled', self.monitor.check(sample(31), 1000, frames=0))

    def test_memory_and_vram_growth_and_absolute_caps(self):
        self.assertIn('private memory cap', self.monitor.check(sample(), 20000, 1))
        self.assertIn('VRAM cap', self.monitor.check(sample(vram_mb=7300)))
        for field in ('private', 'vram'):
            monitor = hm.Monitor(clock=lambda: self.now)
            self.now = 0
            monitor.check(sample(), 1000, 1)
            self.now = 31
            reason = monitor.check(sample(31, vram_mb=4000 if field == 'vram' else 500),
                                   5000 if field == 'private' else 1000, 1)
            self.assertIn('sustained growth', reason)

    def test_log_fault_split_across_reads_and_backlog(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'run.log'
            path.write_text('VK_ERROR_DEVICE_')
            log = hm.RunLog(path)
            self.assertIsNone(log.fault())
            with path.open('a') as stream:
                stream.write('LOST')
            self.assertIn('direct log fault', log.fault())
            path.write_bytes(b'x' * (300 * 1024))
            self.assertIn('backlog', hm.RunLog(path).fault())

    def test_cpu_sensor_absence_is_explicit_and_acpi_is_never_queried(self):
        with mock.patch.object(hm, 'powershell', side_effect=OSError('provider absent')) as query, \
                mock.patch.object(hm, 'bounded_run', side_effect=OSError('GPU absent')):
            result = hm.sensors()
        self.assertEqual(result['cpu_c']['status'], 'unknown')
        self.assertNotIn('ACPI', str(query.call_args_list))


class NativeContainmentTests(unittest.TestCase):
    def wait_text(self, path, text):
        until = time.monotonic() + 1.5
        while time.monotonic() < until:
            data = path.read_text() if path.exists() else ''
            if text in data:
                return data
            time.sleep(.01)
        self.fail('idle fixture did not become ready: ' + str(path))

    def assert_gone(self, pid):
        handle = hj.open_process(0x100000, False, pid)
        if handle:
            try:
                self.assertEqual(hj.wait(handle, 1000), 0, 'still alive: %d' % pid)
            finally:
                hj.close(handle)

    def test_blocked_telemetry_is_bounded_and_reaped(self):
        started = time.monotonic()
        with self.assertRaises(subprocess.TimeoutExpired):
            hj.bounded_run([sys.executable, FIXTURE, 'leaf'], timeout=.3)
        self.assertLess(time.monotonic()-started, 2)

    def test_deadline_kills_child_and_grandchild_during_blocked_observer(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'idle.log'
            job = hj.Job(.7, stale_seconds=2)
            child = None
            try:
                with path.open('w') as output:
                    child = job.launch([sys.executable, FIXTURE, 'tree'], output)
                    data = self.wait_text(path, 'leaf ')
                    leaf = int(data.split()[1])
                    self.assertGreater(job.private_mb(), 0)
                    self.assertGreater(hj.free_commit_mb(), 0)
                    # No runner polling/heartbeat/telemetry is needed for the deadline.
                    with mock.patch.object(time, 'time', return_value=-1000000000):
                        self.assertEqual(child.wait(timeout=2), 124)
                    self.assert_gone(leaf)
            finally:
                helper = job.worker.pid
                job.close()
                if child:
                    child.close()
                self.assert_gone(helper)

    def test_monitor_heartbeat_expiry_precedes_hard_deadline(self):
        job = hj.Job(3, stale_seconds=.3)
        child = None
        try:
            with tempfile.TemporaryFile() as output:
                child = job.launch([sys.executable, FIXTURE, 'leaf'], output)
                self.assertEqual(child.wait(timeout=1.5), 124)
        finally:
            job.close()
            if child:
                child.close()

    def test_runner_death_kills_tree_and_watchdog(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'idle.log'
            runner = subprocess.Popen([sys.executable, FIXTURE, 'runner', str(path)])
            try:
                data = self.wait_text(path, 'leaf ')
                pids = [int(line.split()[1]) for line in data.splitlines()]
                runner.kill()  # retained handle to our own CPU-idle runner
                runner.wait(timeout=1)
                for pid in pids:
                    self.assert_gone(pid)
            finally:
                if runner.poll() is None:
                    runner.kill()
                runner.wait(timeout=1)

    def test_runner_death_inside_suspended_launch_leaves_no_process(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'idle.log'
            runner = subprocess.Popen([sys.executable, FIXTURE, 'runner-gap', str(path)])
            try:
                data = self.wait_text(path, 'watchdog ')
                self.assertEqual(runner.wait(timeout=1), 0)
                for line in data.splitlines():
                    self.assert_gone(int(line.split()[1]))
            finally:
                if runner.poll() is None:
                    runner.kill()
                runner.wait(timeout=1)
    def test_connected_control_pipe_that_never_replies_is_bounded(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'idle.log'
            job = hj.Job(3)
            child = None
            try:
                with path.open('w') as output:
                    child = job.launch([sys.executable, FIXTURE, 'pipe'], output)
                    self.wait_text(path, 'ready')
                    with mock.patch.object(gamectl, 'check_owned', return_value=job.name):
                        with self.assertRaises(TimeoutError):
                            gamectl.pipe(child.pid, 'status', timeout=.3)
            finally:
                job.close()
                if child:
                    self.assert_gone(child.pid)
                    child.close()

    def test_unowned_pid_cannot_be_signalled_or_followed(self):
        job = hj.Job(2)
        try:
            self.assertFalse(job.owns(os.getpid()))
            with mock.patch.dict(run_game.CURRENT, job=job), \
                    self.assertRaisesRegex(OSError, 'not in the owned job'):
                run_game.kill_pid(os.getpid())
        finally:
            job.close()

    def test_descendant_remains_owned_after_root_exits(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'idle.log'
            job = hj.Job(2)
            child = None
            try:
                with path.open('w') as output:
                    child = job.launch([sys.executable, FIXTURE, 'tree-exit'], output)
                    leaf = int(self.wait_text(path, 'leaf ').split()[1])
                    self.assertEqual(child.wait(timeout=1), 0)
                    self.assertTrue(job.owns(leaf))
                job.close()
                self.assert_gone(leaf)
            finally:
                job.close()
                if child:
                    child.close()

    def test_assignment_failure_never_executes_child_code(self):
        job = hj.Job(2)
        try:
            with tempfile.TemporaryFile() as output:
                with mock.patch.object(hj, 'update_attribute', return_value=0):
                    with self.assertRaises(OSError):
                        job.launch([sys.executable, '-c', "print('must not execute')"], output)
                output.seek(0)
                self.assertEqual(output.read(), b'')
        finally:
            job.close()

    def test_atomic_machine_lease_blocks_competing_process(self):
        with hj.MachineLease():
            with self.assertRaisesRegex(OSError, 'another harness owns'):
                hj.bounded_run([sys.executable, FIXTURE, 'lease'], timeout=1)
        self.assertEqual(hj.bounded_run([sys.executable, FIXTURE, 'lease'], timeout=1), 'acquired')

    def test_atomic_machine_lease_blocks_competing_thread(self):
        results = []
        def compete():
            try:
                with hj.MachineLease():
                    results.append('incorrectly acquired')
            except SystemExit:
                results.append('refused')
        with hj.MachineLease():
            thread = threading.Thread(target=compete)
            thread.start()
            thread.join(timeout=1)
        self.assertEqual(results, ['refused'])
        with hj.MachineLease():
            pass


if __name__ == '__main__':
    unittest.main()
