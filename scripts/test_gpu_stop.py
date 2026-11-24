"""CPU-only latch/runner failure injection. Every path/process/GPU query is isolated."""
from contextlib import ExitStack
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import gpu_stop
import play_scenario
import run_game


class Sandbox(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.shared = self.root / 'GPU-STOP.json'
        self.extra = self.root / 'caller-stop.json'
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.stack.enter_context(mock.patch.dict(os.environ, {}, clear=True))
        self.stack.enter_context(mock.patch.object(gpu_stop, 'SHARED_STOP', self.shared))
        self.stack.enter_context(mock.patch.object(gpu_stop, '_FAILED_WRITES', {}))

    def patch(self, target, name, **kwargs):
        return self.stack.enter_context(mock.patch.object(target, name, **kwargs))

    def seed(self, path=None, kind='first-reset'):
        gpu_stop.record_fault((path or self.shared,), dict(kind=kind, run='retained-run', exit=71))

    def faults(self, path=None):
        return json.loads((path or self.shared).read_text(encoding='utf-8'))['faults']


class LatchTests(Sandbox):
    def test_absence_is_clear_but_valid_marker_stops(self):
        gpu_stop.require_clear(gpu_stop.paths())
        self.seed()
        with self.assertRaisesRegex(SystemExit, 'GPU STOP:.*first-reset'):
            gpu_stop.require_clear(gpu_stop.paths())

    def test_malformed_empty_false_and_wrong_schema_markers_refuse(self):
        for text in ('', '{', 'null', '[]', '{}',
                     '{"schema":"nxrs.gpu-stop.v1","stopped":false,"faults":[]}'):
            with self.subTest(text=text):
                self.shared.write_text(text, encoding='utf-8')
                with self.assertRaisesRegex(SystemExit, 'unreadable/invalid latch'):
                    gpu_stop.require_clear(gpu_stop.paths())

    def test_non_utf8_and_directory_markers_refuse(self):
        self.shared.write_bytes(b'\xff')
        with self.assertRaisesRegex(SystemExit, 'unreadable/invalid latch'):
            gpu_stop.require_clear(gpu_stop.paths())
        self.shared.unlink()
        self.shared.mkdir()
        with self.assertRaisesRegex(SystemExit, 'unreadable/invalid latch'):
            gpu_stop.require_clear(gpu_stop.paths())

    def test_unreadable_marker_refuses_instead_of_treating_it_as_absent(self):
        with mock.patch.object(Path, 'open', side_effect=PermissionError('denied')):
            with self.assertRaisesRegex(SystemExit, 'GPU STOP:.*denied'):
                gpu_stop.require_clear(gpu_stop.paths())

    def test_paths_are_additive_even_when_run_env_overrides_ambient(self):
        ambient = self.root / 'ambient.json'
        os.environ['NXRS_GPU_STOP_PATH'] = str(ambient)
        self.assertEqual(gpu_stop.paths({'NXRS_GPU_STOP_PATH': str(self.extra)}),
                         (self.shared, ambient, self.extra))
        self.assertEqual(gpu_stop.paths({'NXRS_GPU_STOP_PATH': ''}), (self.shared, ambient))
        self.assertEqual(gpu_stop.paths({'NXRS_GPU_STOP_PATH': str(self.shared)}),
                         (self.shared, ambient))

    def test_first_and_subsequent_fault_provenance_is_retained(self):
        self.seed()
        first = self.faults()[0]
        second = dict(kind='late-display-reset', run='second-run', resets=['14:00:01'], pid=123)
        gpu_stop.record_fault(gpu_stop.paths({'NXRS_GPU_STOP_PATH': str(self.extra)}), second)
        faults = self.faults()
        self.assertEqual(faults[0], first)
        self.assertEqual(len(faults), 2)
        self.assertEqual(faults[1]['resets'], ['14:00:01'])
        self.assertEqual(faults[1]['run'], 'second-run')
        self.assertEqual(self.faults(self.extra), [faults[1]])
        self.assertFalse(gpu_stop.pending_path(self.shared).exists())

    def test_successor_startup_grace_waits_for_logged_child(self):
        live = self.patch(run_game, 'alive', side_effect=[False, False, True])
        sleep = self.patch(run_game.time, 'sleep')
        self.assertTrue(run_game.successor_started(123, attempts=3, delay=0.2))
        self.assertEqual(live.call_count, 3)
        self.assertEqual(sleep.call_count, 2)

    def test_successor_startup_grace_is_bounded(self):
        self.patch(run_game, 'alive', return_value=False)
        sleep = self.patch(run_game.time, 'sleep')
        self.assertFalse(run_game.successor_started(123, attempts=2, delay=0.2))
        self.assertEqual(sleep.call_count, 2)

    def test_corrupt_history_is_not_overwritten_and_new_fault_is_journaled(self):
        self.shared.write_text('{broken', encoding='utf-8')
        with self.assertRaises(gpu_stop.PersistenceError):
            self.seed(kind='new-fault')
        self.assertEqual(self.shared.read_text(), '{broken')
        journal = json.loads(gpu_stop.pending_path(self.shared).read_text())
        self.assertEqual(journal['kind'], 'new-fault')

    def test_failed_replace_preserves_first_fault_and_restart_gate(self):
        self.seed()
        original = self.shared.read_bytes()
        with mock.patch.object(gpu_stop.os, 'replace', side_effect=OSError('disk full')):
            with self.assertRaisesRegex(gpu_stop.PersistenceError, 'disk full'):
                self.seed(kind='second-fault')
        self.assertEqual(self.shared.read_bytes(), original)
        self.assertEqual(json.loads(gpu_stop.pending_path(self.shared).read_text())['kind'],
                         'second-fault')
        with mock.patch.object(gpu_stop, '_FAILED_WRITES', {}):
            with self.assertRaisesRegex(SystemExit, 'pending fault journal'):
                gpu_stop.require_clear(gpu_stop.paths())

    def test_first_write_failure_still_leaves_durable_refusal(self):
        with mock.patch.object(gpu_stop.os, 'replace', side_effect=OSError('replace denied')):
            with self.assertRaises(gpu_stop.PersistenceError):
                self.seed()
        self.assertFalse(self.shared.exists())
        with mock.patch.object(gpu_stop, '_FAILED_WRITES', {}):
            with self.assertRaisesRegex(SystemExit, 'pending fault journal'):
                gpu_stop.require_clear(gpu_stop.paths())

    def test_busy_journal_is_not_overwritten_or_automatically_recovered(self):
        pending = gpu_stop.pending_path(self.shared)
        pending.write_text('retained interrupted fault', encoding='utf-8')
        with self.assertRaises(gpu_stop.PersistenceError):
            self.seed()
        self.assertEqual(pending.read_text(), 'retained interrupted fault')

    def test_optional_write_failure_cannot_prevent_shared_persistence(self):
        self.extra.mkdir()
        with self.assertRaises(gpu_stop.PersistenceError):
            gpu_stop.record_fault(gpu_stop.paths({'NXRS_GPU_STOP_PATH': str(self.extra)}),
                                  dict(kind='exit-71', exit=71))
        self.assertEqual(self.faults()[0]['exit'], 71)

    def test_total_write_failure_poison_is_explicit_in_same_interpreter(self):
        with mock.patch.object(Path, 'mkdir', side_effect=PermissionError('all storage denied')):
            with self.assertRaisesRegex(gpu_stop.PersistenceError, 'all storage denied'):
                self.seed()
        self.assertFalse(self.shared.exists())
        with self.assertRaisesRegex(SystemExit, 'earlier persistence failure'):
            gpu_stop.require_clear(gpu_stop.paths())


class FakeChild:
    pid = 123

    def __init__(self):
        self.returncode = 0
        self.termination_requested = False
        self.killed = False
        self.waits = []

    def poll(self):
        return self.returncode

    def kill(self):
        self.killed = True
        if self.returncode is None:
            self.returncode = -9

    def close(self):
        pass

    def wait(self, timeout=None):
        self.waits.append(timeout)
        self.poll()
        if self.returncode is None:
            raise AssertionError('runner waited for a live child without stopping it')
        return self.returncode


class RunnerTests(Sandbox):
    def setUp(self):
        super().setUp()
        self.exe = self.root / 'game.exe'
        self.log = self.root / 'run.log'
        self.claim = self.root / 'GAME_LOCK.agent'
        self.child = FakeChild()
        self.job = mock.Mock(name='contained job')
        self.job.name = 'fixture-job'
        self.job.worker.poll.return_value = None
        self.job.deadline = 2000
        self.job.owns.return_value = True
        self.job.close.return_value = True
        self.job.kill.side_effect = self.child.kill
        self.child.job = self.job
        self.next_child = FakeChild()
        self.next_child.pid = 456
        self.next_child.job = self.job
        def adopt(pid):
            if not self.job.owns(pid):
                raise OSError('refused unowned successor')
            return self.next_child
        self.job.adopt.side_effect = adopt
        self.now = 1000.0
        self.patch(run_game, 'EVIDENCE', new=str(self.root))
        self.patch(run_game, 'LEGACY_LOCK', new=str(self.root / 'GAME_LOCK'))
        self.patch(run_game, 'CURRENT', new={'pid': None})
        # Any unexpected platform operation fails the test, never reaches Windows.
        self.patch(run_game, 'ps', side_effect=AssertionError('unmocked platform query'))
        self.patch(run_game.subprocess, 'run', side_effect=AssertionError('unmocked subprocess'))
        self.patch(run_game.subprocess, 'Popen', side_effect=AssertionError('unmocked launch'))
        self.patch(run_game.harness_job, 'Job', return_value=self.job)
        self.patch(run_game.harness_job, 'MachineLease')
        self.monitor = self.patch(run_game.harness_monitor, 'Monitor').return_value
        self.monitor.sample.return_value = {}
        self.monitor.check.return_value = None
        self.launch = self.job.launch
        def launch(*args, **kwargs):
            kwargs['before_resume']()
            kwargs['on_resume']()
            return self.child
        self.launch.side_effect = launch
        self.resets = self.patch(run_game, 'gpu_resets_since', return_value=[])
        self.claims = self.patch(run_game, 'other_claims', return_value=[])
        self.games = self.patch(run_game, 'other_games', return_value=[])
        self.memory = self.patch(run_game, 'free_commit_mb', return_value=50000)
        self.patch(run_game, 'priv_mb', return_value=100)
        self.patch(run_game, 'compilers', return_value=[])
        self.patch(run_game, 'alive', side_effect=lambda pid: pid == self.child.pid
                   and self.child.returncode is None)
        self.kill = self.job.kill
        self.snapshot = self.patch(run_game, 'diagnostic_snapshot')
        self.patch(run_game, 'successor', return_value=None)
        self.patch(run_game.threading, 'Thread').return_value.is_alive.return_value = False
        self.patch(run_game.time, 'time', side_effect=lambda: self.now)
        self.patch(run_game.time, 'monotonic', side_effect=lambda: self.now)
        self.patch(run_game.time, 'sleep', side_effect=self.advance)
        self.patch(run_game.run_triage, 'provenance')
        self.summary = self.patch(run_game.observability, 'generate')

    def advance(self, seconds):
        self.now += seconds

    def run_guard(self, **env):
        return run_game.run(str(self.exe), str(self.log), 100,
                            env=dict(NXRS_AUTO_CAPTURE='0', NXRS_ARTIFACT_DIR=str(self.root), **env),
                            say=lambda message: None)

    def assert_clean(self):
        self.assertFalse(self.claim.exists())
        self.assertIsNone(run_game.CURRENT['pid'])
        self.assertIsNotNone(self.child.returncode)

    def test_expired_cooldown_cannot_bypass_shared_stop_with_optional_path(self):
        # Pre-fix counterexample: quiet event log, old persistent stop, alternate entrypoint.
        self.seed()
        with self.assertRaisesRegex(SystemExit, 'GPU STOP'):
            self.run_guard(NXRS_GPU_STOP_PATH=str(self.extra))
        self.launch.assert_not_called()
        self.resets.assert_not_called()
        self.claims.assert_not_called()
        self.assertFalse(self.claim.exists())

    def test_optional_marker_also_refuses_before_claim(self):
        self.seed(self.extra)
        with self.assertRaisesRegex(SystemExit, 'GPU STOP'):
            self.run_guard(NXRS_GPU_STOP_PATH=str(self.extra))
        self.launch.assert_not_called()
        self.assertFalse(self.claim.exists())

    def test_malformed_marker_refuses_before_child_or_claim(self):
        self.shared.write_text('{', encoding='utf-8')
        with self.assertRaisesRegex(SystemExit, 'unreadable/invalid latch'):
            self.run_guard()
        self.launch.assert_not_called()
        self.claims.assert_not_called()
        self.assertFalse(self.claim.exists())

    def test_marker_appearing_during_preflight_refuses_before_claim(self):
        def memory():
            self.seed()
            return 50000
        self.memory.side_effect = memory
        with self.assertRaisesRegex(SystemExit, 'GPU STOP'):
            self.run_guard()
        self.launch.assert_not_called()
        self.assertFalse(self.claim.exists())

    def test_marker_appearing_while_claim_waits_refuses_before_launch_and_releases_claim(self):
        def other_games():
            self.assertTrue(self.claim.exists())
            self.seed()
            return []
        self.games.side_effect = other_games
        with self.assertRaisesRegex(SystemExit, 'GPU STOP'):
            self.run_guard()
        self.launch.assert_not_called()
        self.assert_clean()

    def test_absent_marker_and_normal_exit_do_not_create_stop(self):
        result = self.run_guard()
        self.assertEqual(result['why'], 'exited')
        self.assertEqual(result['exit'], 0)
        self.assertFalse(self.shared.exists())
        self.assert_clean()

    def test_compiler_preflight_refuses_even_with_old_opt_out_environment(self):
        run_game.compilers.return_value = ['rustc.exe']
        with self.assertRaisesRegex(SystemExit, 'build overlap refused'):
            self.run_guard(NXRS_WAIT_FOR_BUILDS='0', NXRS_CONCURRENT='1')
        self.launch.assert_not_called()
        self.assertFalse(self.claim.exists())

    def test_compiler_starting_after_preflight_refuses_at_launch(self):
        run_game.compilers.side_effect = [[], ['clang.exe']]
        with self.assertRaisesRegex(SystemExit, 'at launch'):
            self.run_guard()
        self.launch.assert_not_called()
        self.assert_clean()

    def test_compiler_starting_mid_run_kills_only_our_job_first(self):
        self.child.returncode = None
        run_game.compilers.side_effect = [[], [], ['cargo.exe']]
        result = self.run_guard()
        self.assertIn('build overlap', result['why'])
        self.assertTrue(self.child.killed)
        self.assertEqual(self.faults()[0]['kind'], 'build-overlap')
        self.snapshot.assert_not_called()
        self.assert_clean()

    def test_absent_sensor_refuses_before_launch_and_records_status(self):
        self.monitor.check.return_value = 'cpu_c unknown: absent sensor'
        with self.assertRaisesRegex(SystemExit, 'thermal monitoring required'):
            self.run_guard()
        self.launch.assert_not_called()
        record = json.loads((self.root / 'run-intent.json').read_text())
        self.assertEqual(record['state'], 'refused')
        self.assertIn('absent sensor', record['reason'])

    def test_sensor_lost_midrun_kills_before_persisting_observation(self):
        self.child.returncode = None
        self.monitor.check.side_effect = [None] * 4 + ['cpu_c stale acquisition time']
        result = self.run_guard()
        self.assertIn('stale', result['why'])
        self.assertTrue(self.child.killed)
        self.assertEqual(self.faults()[0]['kind'], 'monitor-stop')
        self.assertIsInstance(self.faults()[0]['monitoring'], dict)
        self.assert_clean()

    def test_unowned_logged_successor_is_never_adopted(self):
        run_game.successor.return_value = ('unrelated.exe', 999)
        self.job.owns.return_value = False
        result = self.run_guard()
        self.assertIn('refused unowned successor', result['why'])
        run_game.alive.assert_not_called()
        self.assert_clean()

    def test_intent_is_fsynced_before_launch_and_interruption_blocks_reuse(self):
        def launch(*args, **kwargs):
            record = json.loads((self.root / 'run-intent.json').read_text())
            self.assertEqual(record['state'], 'intent')
            self.assertIn('never infer success', record['recovery'])
            raise OSError('injected launch failure')
        self.launch.side_effect = launch
        with self.assertRaisesRegex(OSError, 'launch failure'):
            self.run_guard()
        self.launch.reset_mock()
        with self.assertRaisesRegex(SystemExit, 'interrupted run intent'):
            self.run_guard()
        self.launch.assert_not_called()

    def test_blocked_monitor_failure_stops_before_any_snapshot(self):
        self.child.returncode = None
        self.monitor.sample.side_effect = [{}, {}, {}, OSError('bounded telemetry request timed out')]
        with self.assertRaisesRegex(OSError, 'telemetry request timed out'):
            self.run_guard()
        self.assertTrue(self.child.killed)
        self.snapshot.assert_not_called()
        self.assert_clean()

    def test_exit71_persists_even_without_display_reset_and_prevents_handover(self):
        self.child.returncode = 71
        result = self.run_guard(NXRS_GPU_STOP_PATH=str(self.extra))
        self.assertIn('GPU DEVICE LOST', result['why'])
        self.assertEqual(self.faults()[0]['kind'], 'exit-71')
        self.assertEqual(self.faults()[0]['exit'], 71)
        self.assertEqual(self.faults()[0]['log'], str(self.log))
        self.assertEqual(self.faults(), self.faults(self.extra))
        run_game.successor.assert_not_called()
        self.assert_clean()

    def test_late_reset_is_persisted_before_claim_release(self):
        self.resets.side_effect = [[], [], ['13:57:10']]
        original = gpu_stop.record_fault
        def record(paths, event):
            self.assertTrue(self.claim.exists())
            return original(paths, event)
        with mock.patch.object(gpu_stop, 'record_fault', side_effect=record):
            result = self.run_guard()
        self.assertTrue(result['why'].startswith('GPU RESET'))
        self.assertEqual(self.faults()[0]['kind'], 'late-display-reset')
        self.assertEqual(self.faults()[0]['resets'], ['13:57:10'])
        self.assert_clean()

    def test_exit71_and_late_reset_retain_both_observations(self):
        self.child.returncode = 71
        self.resets.side_effect = [[], [], ['13:57:10']]
        self.run_guard()
        self.assertEqual([f['kind'] for f in self.faults()], ['exit-71', 'late-display-reset'])
        self.assert_clean()

    def test_in_run_reset_stops_child_even_if_snapshot_fails(self):
        self.child.returncode = None
        self.resets.side_effect = [[], [], ['13:57:10']]
        self.snapshot.side_effect = OSError('diagnostics disk full')
        self.run_guard()
        self.assertTrue(self.child.killed)
        self.snapshot.assert_not_called()  # no evidence before or after a safety stop
        self.assertEqual(self.faults()[0]['kind'], 'display-reset')
        self.assert_clean()

    def test_in_run_write_failure_reaps_child_and_reports_error(self):
        self.child.returncode = None
        self.resets.side_effect = [[], [], ['13:57:10']]
        with mock.patch.object(gpu_stop.os, 'replace', side_effect=OSError('disk full')):
            with self.assertRaisesRegex(gpu_stop.PersistenceError, 'disk full'):
                self.run_guard()
        self.assertTrue(self.child.killed)
        self.assertIn(2, self.child.waits)
        self.assertTrue(self.summary.call_args.args[1]['gpu_stop_errors'])
        self.assert_clean()
        self.launch.reset_mock()
        with self.assertRaisesRegex(SystemExit, 'GPU STOP'):
            self.run_guard()
        self.launch.assert_not_called()

    def test_exit71_and_late_write_failures_are_not_success(self):
        for exit_code, late in ((71, []), (0, ['13:57:10'])):
            with self.subTest(exit_code=exit_code):
                (self.root / 'run-intent.json').unlink(missing_ok=True)  # explicit fixture recovery
                self.child.returncode = exit_code
                self.resets.side_effect = [[], [], late]
                with mock.patch.object(gpu_stop, 'record_fault', side_effect=OSError('denied')):
                    with self.assertRaisesRegex(gpu_stop.PersistenceError, 'denied'):
                        self.run_guard()
                self.assert_clean()

    def test_fault_on_handover_stops_actual_successor(self):
        next_pid = 456
        live = {next_pid}
        run_game.successor.return_value = ('next.exe', next_pid)
        run_game.alive.side_effect = lambda pid: pid in live
        def poll():
            self.next_child.returncode = None if next_pid in live else -9
            return self.next_child.returncode
        self.next_child.poll = poll
        self.kill.side_effect = lambda: live.clear()
        self.resets.side_effect = [[], [], ['13:57:10']]
        self.run_guard()
        self.assertTrue(self.kill.called)
        self.assertEqual(self.faults()[0]['pid'], next_pid)
        self.assertIsNone(self.faults()[0]['exit_pid'])  # no successor exit observed at reset
        self.assertFalse(live)
        self.assert_clean()

    def test_latch_failure_on_handover_still_stops_actual_successor(self):
        next_pid = 456
        live = {next_pid}
        run_game.successor.return_value = ('next.exe', next_pid)
        run_game.alive.side_effect = lambda pid: pid in live
        def poll():
            self.next_child.returncode = None if next_pid in live else -9
            return self.next_child.returncode
        self.next_child.poll = poll
        self.kill.side_effect = lambda: live.clear()
        self.resets.side_effect = [[], [], ['13:57:10']]
        with mock.patch.object(gpu_stop.os, 'replace', side_effect=OSError('disk full')):
            with self.assertRaisesRegex(gpu_stop.PersistenceError, 'disk full'):
                self.run_guard()
        self.assertTrue(self.kill.called)
        self.assertFalse(live)
        self.assert_clean()

    def test_fault_kills_before_persistence(self):
        self.child.returncode = None
        self.resets.side_effect = [[], [], ['13:57:10']]
        original = gpu_stop.record_fault
        def persist(paths, event):
            self.assertTrue(self.child.killed)
            return original(paths, event)
        with mock.patch.object(gpu_stop, 'record_fault', side_effect=persist):
            self.run_guard()
        self.assertTrue(self.child.killed)
        self.assertEqual(self.faults()[0]['kind'], 'display-reset')
        self.assert_clean()

    def test_exception_after_launch_kills_original_child(self):
        self.child.returncode = None
        run_game.priv_mb.side_effect = OSError('memory evidence failed')
        with self.assertRaisesRegex(OSError, 'memory evidence failed'):
            self.run_guard()
        self.assertTrue(self.child.killed)
        self.assert_clean()

    def test_scenario_restores_save_after_latch_failure_and_child_stop(self):
        save = self.root / 'data/save'
        save.mkdir(parents=True)
        (save / 'slot').write_bytes(b'original')
        self.child.returncode = None
        self.resets.side_effect = [[], [], ['13:57:10']]
        def launch(*args, **kwargs):
            kwargs['before_resume']()
            kwargs['on_resume']()
            (save / 'slot').write_bytes(b'child write')
            return self.child
        self.launch.side_effect = launch
        options = SimpleNamespace(data=str(save.parent), env=['NXRS_AUTO_CAPTURE=0'], option=[],
                                  seconds=100, capture_interval=0, diagnostics='baseline')
        finish = play_scenario.SaveTransaction.finish
        def restore(tx):
            self.assertTrue(self.child.killed)
            self.assertIsNotNone(self.child.returncode)
            return finish(tx)
        with mock.patch.object(play_scenario, 'PM', []), \
                mock.patch.object(play_scenario.SaveTransaction, 'finish', restore), \
                mock.patch.object(gpu_stop.os, 'replace', side_effect=OSError('latch denied')):
            with self.assertRaisesRegex(gpu_stop.PersistenceError, 'latch denied'):
                play_scenario.scenario(options, dict(save=True, seconds=100, steps=[]),
                                       self.root, self.exe)
        self.assertEqual((save / 'slot').read_bytes(), b'original')
        self.assertEqual((self.root / 'save-after/slot').read_bytes(), b'child write')
        self.assertTrue(json.loads((self.root / 'save-restoration.json').read_text())['restored'])
        self.assert_clean()

    def test_scenario_refusal_preserves_original_absence_of_save(self):
        self.seed()
        data = self.root / 'data'
        options = SimpleNamespace(data=str(data), env=[], option=[], seconds=100,
                                  capture_interval=0, diagnostics='baseline')
        with mock.patch.object(play_scenario, 'PM', []):
            with self.assertRaisesRegex(SystemExit, 'GPU STOP'):
                play_scenario.scenario(options, dict(save=True, seconds=100, steps=[]),
                                       self.root, self.exe)
        self.launch.assert_not_called()
        self.assertFalse((data / 'save').exists())
        self.assertFalse((self.root / 'save-restoration.json').exists())  # no launch, no replacement
        self.assert_clean()


if __name__ == '__main__':
    unittest.main()
