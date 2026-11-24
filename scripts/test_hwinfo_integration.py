"""Actual runner/scenario boundaries, synthetic CSVs and real job-owned idle children."""
import json
import os
from pathlib import Path
import sys
import threading
import time
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import test_harness_review as review
from test_hwinfo_csv import Feed
import gpu_stop
import harness_job as hj
import harness_lifecycle as life
import harness_monitor as hm
import hwinfo_csv as hw
import run_game as rg

MONITOR = hm.Monitor


class HwinfoIntegration(review.Integration):
    def setUp(self):
        super().setUp()
        self.feed = Feed(self.root / 'sensors.csv')
        self.source = self.feed.proven()
        self.monitor.clock = lambda: self.feed.now
        provider = hm.SensorProvider({'NXRS_HWINFO_CSV': str(self.feed.path)},
                                     clock=lambda: self.feed.now)
        provider.cpu, provider.first = self.source, False
        self.provider = provider
        self.monitor.provider = provider
        self.stack.enter_context(mock.patch.object(hm, 'powershell', return_value='1'))
        self.stack.enter_context(mock.patch.object(hm, 'bounded_run', return_value='40,1,100,8000'))

    def after_save_write(self, mutation):
        """Mutate the synthetic sensor after the real idle child changed its temp save."""
        def sample():
            self.calls += 1
            if self.calls == 4:
                until = time.monotonic()+.5
                while self.slot.read_bytes() != b'child write' and time.monotonic() < until:
                    time.sleep(.01)
                self.assertEqual(self.slot.read_bytes(), b'child write')
                mutation()
            return self.provider()
        self.monitor.provider = sample

    def assert_stopped_restored(self, reason):
        self.scenario()
        result = json.loads((self.run / 'result.json').read_text())
        self.assertIn(reason, result['why'])
        self.assertEqual(result['exit'], 124)
        self.assertTrue(self.states[-1].cleanup_confirmed)
        self.assertEqual(self.slot.read_bytes(), b'original')
        self.assertEqual((self.run / 'save-after/slot').read_bytes(), b'child write')
        self.assertEqual(self.faults()[0]['kind'], 'monitor-stop')
        self.assertIn('HWiNFO CSV', self.faults()[0]['monitoring']['cpu_c']['source'])
        with self.assertRaisesRegex(SystemExit, 'GPU STOP'):
            self.guarded('leaf')

    def test_stopped_logging_kills_idle_child_and_restores_after_drain(self):
        def freeze():
            self.feed.now += 13
            self.feed.wall += 13
            os.utime(self.feed.path, None)
        self.after_save_write(freeze)
        self.assert_stopped_restored('stale')

    def test_hot_duplicate_sensor_stops_even_if_other_package_is_cool(self):
        self.after_save_write(lambda: self.feed.append(2, c2=90, c4=40))
        self.assert_stopped_restored('cpu_c cap')

    def test_critical_flag_stops_below_temperature_cap(self):
        self.after_save_write(lambda: self.feed.append(2, c8='Yes'))
        self.assert_stopped_restored('protective flag')

    def test_throttling_stops_below_temperature_cap(self):
        self.after_save_write(lambda: self.feed.append(2, c7='Yes'))
        self.assert_stopped_restored('protective flag')

    def test_truncation_during_run_stops_and_preserves_save_contract(self):
        self.after_save_write(lambda: self.feed.path.write_bytes(b'Date,Time\n'))
        self.assert_stopped_restored('unknown')

    def test_malformed_required_data_stops_and_persists(self):
        self.after_save_write(lambda: self.feed.append(2, c5='NaN'))
        self.assert_stopped_restored('unknown')

    def test_blocked_csv_probe_is_bounded_and_idle_job_stops(self):
        def block():
            self.source.reader = lambda path, offset: hw.bounded_run(
                [sys.executable, review.FIXTURE, 'leaf'], timeout=.15)
        self.after_save_write(block)
        create = hj.create_suspended
        def longer_writer(command, *args):
            if 'write-save' in command:
                command = [*command, '1']
            return create(command, *args)
        with mock.patch.object(hj, 'create_suspended', side_effect=longer_writer):
            self.assert_stopped_restored('unknown')

    def test_delayed_preflight_rejects_frozen_csv_before_creation(self):
        def delay(*args):
            self.feed.now += 13
            self.feed.wall += 13
        rg.run_triage.provenance.side_effect = delay
        state = life.RunState()
        with self.assertRaisesRegex(SystemExit, 'at launch: cpu_c stale'):
            self.guarded('write-save', str(self.slot), state=state)
        self.assertFalse(state.launched)
        self.assertFalse(self.jobs[0].children)
        self.assertEqual(self.slot.read_bytes(), b'original')

    def test_csv_turns_hot_while_child_suspended_never_executes(self):
        native_create = hj.create_suspended
        def create(*args):
            result = native_create(*args)
            self.feed.append(2, c6=91)
            return result
        state = life.RunState()
        with mock.patch.object(hj, 'create_suspended', side_effect=create):
            with self.assertRaisesRegex(SystemExit, 'at launch: cpu_c cap'):
                self.guarded('write-save', str(self.slot), state=state)
        self.assertFalse(state.launched)
        self.assertEqual(len(self.jobs[0].children), 1)
        self.assertEqual(self.slot.read_bytes(), b'original')

    def test_csv_acquisition_age_expires_during_final_latch_io(self):
        gate = gpu_stop.require_clear
        def delayed(paths):
            gate(paths)
            if self.jobs and self.jobs[0].children:
                self.feed.now += 13
                self.feed.wall += 13
        state = life.RunState()
        with mock.patch.object(gpu_stop, 'require_clear', side_effect=delayed):
            with self.assertRaisesRegex(SystemExit, 'at resume: cpu_c stale'):
                self.guarded('write-save', str(self.slot), state=state)
        self.assertFalse(state.launched)
        self.assertEqual(self.slot.read_bytes(), b'original')

    def test_normal_configured_provider_establishes_progression_in_real_probe_jobs(self):
        # Exercise normal initial warm-up/configuration and actual snapshot worker, not a seeded source.
        self.feed.path.unlink()
        self.feed = Feed(self.feed.path, wall=time.time()-.1)
        self.feed.append()
        stop = threading.Event()
        def append():
            for _ in range(10):
                if stop.wait(.2):
                    return
                self.feed.wall = time.time()-.01
                self.feed.append()
        writer = threading.Thread(target=append)
        writer.start()
        try:
            with mock.patch.object(hm, 'Monitor', MONITOR):
                result = self.guarded('exit-code', '0',
                                      env={'NXRS_HWINFO_CSV': str(self.feed.path)})
        finally:
            stop.set()
            writer.join(timeout=1)
        self.assertFalse(writer.is_alive())
        self.assertEqual(result['exit'], 0)
        preflight = json.loads((self.run / 'thermal-preflight.json').read_text())
        self.assertTrue(preflight['cpu_c']['observed_progression'])
        self.assertEqual(preflight['cpu_c']['status'], 'ok')
        self.assertTrue(all(job.cleanup_confirmed for job in self.jobs))

    def test_preflight_stale_csv_never_replaces_temporary_save(self):
        self.feed.now += 13
        self.feed.wall += 13
        identity = self.slot.stat().st_ino
        with self.assertRaisesRegex(SystemExit, 'cpu_c stale'):
            self.scenario()
        self.assertEqual(self.slot.stat().st_ino, identity)
        self.assertEqual(self.slot.read_bytes(), b'original')
        self.assertFalse((self.run / 'save-restoration.json').exists())
        self.assertFalse(self.jobs)

    def test_missing_csv_refuses_before_child_creation(self):
        self.feed.path.unlink()
        with self.assertRaisesRegex(SystemExit, 'cpu_c unknown'):
            self.scenario()
        self.assertEqual(self.slot.read_bytes(), b'original')
        self.assertFalse(self.jobs)
        self.assertFalse((self.run / 'save-restoration.json').exists())

    def test_csv_fault_with_uncertain_cleanup_withholds_save_restoration(self):
        self.after_save_write(lambda: self.feed.append(2, c2=90))
        terminate = hj.terminate_job
        failed = []
        def uncertain(handle, status):
            result = terminate(handle, status)
            if self.calls >= 4 and not failed:
                failed.append(True)
                return 0  # actual termination succeeded; its reported uncertainty stays sticky
            return result
        with mock.patch.object(hj, 'terminate_job', side_effect=uncertain):
            with self.assertRaises(hj.CleanupError):
                self.scenario()
        self.assertTrue(failed)
        self.assert_backup_withheld()
        self.assertEqual([f['kind'] for f in self.faults()], ['monitor-stop', 'cleanup-uncertain'])
