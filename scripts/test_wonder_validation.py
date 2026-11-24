"""CPU-only boundaries for the one-title adapter/resumption; temporary latches/saves only."""
from contextlib import ExitStack
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import gpu_stop as gs
import harness_job as hj
import wonder_resume as wr
import wonder_tools as wt
import wonder_validation as wv


class WonderTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.folder = self.root / 'run'
        self.folder.mkdir()
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.stack.enter_context(mock.patch.object(gs, 'SHARED_STOP', self.root / 'stop.json'))
        self.stack.enter_context(mock.patch.object(gs, 'RESUMPTION_INTENT', self.root / 'resume.json'))
        self.stack.enter_context(mock.patch.object(gs, '_FAILED_WRITES', {}))
        self.stack.enter_context(mock.patch.object(wr, '_active', None))
        self.old = json.dumps(dict(schema=gs.SCHEMA, stopped=True, faults=[dict(kind='old-review')])).encode()
        self.pending = b'{"kind":"old-pending-review"}'
        gs.SHARED_STOP.write_bytes(self.old)
        gs.pending_path(gs.SHARED_STOP).write_bytes(self.pending)
        self.stack.enter_context(mock.patch.object(wr, 'REVIEWED', (wr.sha(self.old), wr.sha(self.pending))))

    def test_exact_history_preserved_normal_gate_and_reblock(self):
        with hj.MachineLease() as lease:
            with wr.WonderResumption(lease, self.folder):
                self.assertEqual((self.folder / 'history/shared-before.json').read_bytes(), self.old)
                self.assertEqual((self.folder / 'history/pending-before.json').read_bytes(), self.pending)
                gs.require_clear((gs.SHARED_STOP,))
                refused = []
                def other_thread():
                    try:
                        gs.require_clear((gs.SHARED_STOP,))
                    except SystemExit:
                        refused.append(True)
                thread = threading.Thread(target=other_thread)
                thread.start()
                thread.join(timeout=1)
                self.assertEqual(refused, [True])
            with self.assertRaises(SystemExit):
                gs.require_clear((gs.SHARED_STOP,))
        self.assertFalse(gs.RESUMPTION_INTENT.exists())
        self.assertEqual(gs.pending_path(gs.SHARED_STOP).read_bytes(), self.pending)

    def test_new_fault_is_additive_not_overwritten(self):
        with hj.MachineLease() as lease:
            with wr.WonderResumption(lease, self.folder):
                gs.record_fault((gs.SHARED_STOP,), dict(kind='new-thermal-fault'))
                with self.assertRaises(SystemExit):
                    gs.require_clear((gs.SHARED_STOP,))
        kinds = [f['kind'] for f in gs._read(gs.SHARED_STOP)['faults']]
        self.assertEqual(kinds, ['old-review', 'new-thermal-fault', 'wonder-authorized-attempt-closed'])

    def test_new_partial_pending_is_preserved_exactly(self):
        new = b'{"kind":"new-fault", "partial":'
        with hj.MachineLease() as lease:
            with wr.WonderResumption(lease, self.folder):
                gs.pending_path(gs.SHARED_STOP).write_bytes(new)
        self.assertEqual(gs.pending_path(gs.SHARED_STOP).read_bytes(), new)
        self.assertEqual((self.folder / 'history/pending-after.json').read_bytes(), new)

    def test_unreviewed_history_refuses_without_clearing(self):
        gs.SHARED_STOP.write_bytes(self.old+b' ')
        with hj.MachineLease() as lease:
            with self.assertRaisesRegex(RuntimeError, 'changed since lead review'):
                with wr.WonderResumption(lease, self.folder):
                    self.fail('unexpected resumption')
        self.assertEqual(gs.pending_path(gs.SHARED_STOP).read_bytes(), self.pending)

    def test_failed_archive_never_exposes_clear_gate(self):
        with hj.MachineLease() as lease, mock.patch.object(wr, 'exclusive_bytes', side_effect=OSError('disk')):
            with self.assertRaises(OSError):
                with wr.WonderResumption(lease, self.folder):
                    self.fail('unexpected resumption')
        self.assertEqual(gs.SHARED_STOP.read_bytes(), self.old)
        self.assertEqual(gs.pending_path(gs.SHARED_STOP).read_bytes(), self.pending)

    def test_failed_restore_keeps_recovery_intent_blocking(self):
        with hj.MachineLease() as lease:
            scope = wr.WonderResumption(lease, self.folder)
            scope.__enter__()
            with mock.patch.object(wr, 'write_json', side_effect=OSError('disk')):
                with self.assertRaises(OSError):
                    scope.__exit__(None, None, None)
            with self.assertRaises(SystemExit):
                gs.require_clear((gs.SHARED_STOP,))
            self.assertTrue(gs.RESUMPTION_INTENT.exists())
            scope.restore()  # explicit test recovery after the injected storage failure

    def test_owner_process_death_leaves_durable_block_without_pid_authority(self):
        script = ('import sys,os; sys.path.insert(0,%r); import gpu_stop as g; import wonder_resume as w; '
                  'import harness_job as h; from pathlib import Path; '
                  'g.SHARED_STOP=Path(%r); g.RESUMPTION_INTENT=Path(%r); w.REVIEWED=%r; '
                  'lease=h.MachineLease(); lease.__enter__(); '
                  'w.WonderResumption(lease,Path(%r)).__enter__(); os._exit(0)') % (
                      str(Path(__file__).parent), str(gs.SHARED_STOP), str(gs.RESUMPTION_INTENT),
                      wr.REVIEWED, str(self.folder))
        hj.bounded_run([sys.executable, '-c', script], timeout=2)
        self.assertFalse(gs.SHARED_STOP.exists())
        with self.assertRaisesRegex(SystemExit, 'resumption/recovery'):
            gs.require_clear((gs.SHARED_STOP,))
        with hj.MachineLease() as lease:
            wr.WonderResumption(lease, self.folder).restore()
        self.assertTrue(gs.SHARED_STOP.exists())

    def test_intent_tampering_and_poisoned_stop_still_refuse(self):
        with hj.MachineLease() as lease:
            with wr.WonderResumption(lease, self.folder):
                gs._FAILED_WRITES[gs.SHARED_STOP] = 'injected persistence failure'
                with self.assertRaises(SystemExit):
                    gs.require_clear((gs.SHARED_STOP,))
                gs.RESUMPTION_INTENT.write_bytes(b'changed')
                with self.assertRaises(SystemExit):
                    gs.require_clear((gs.SHARED_STOP,))

    def test_engine_jobs_normalized_without_modifying_caller_or_other_title(self):
        game = dict(source='C:/my game/source.nup', buildDir='C:/my game/build', outputName='Wonder')
        args = ['build', game['source'], '--outdir', game['buildDir'], '-o', 'Wonder', '--jobs', '8', '--machine']
        result = wt.engine_args(args, game)
        self.assertEqual(result[result.index('--jobs')+1], '4')
        self.assertEqual(args[args.index('--jobs')+1], '8')
        with self.assertRaises(ValueError):
            wt.engine_args(['build', 'C:/other/source.nup', *args[2:]], game)
        with self.assertRaises(ValueError):
            wt.engine_args(['doctor'], game)

    def test_native_tool_slot_serializes_compiler_shader_and_validator(self):
        name = 'Local\\wonder-test-'+str(os.getpid())
        active, peaks, completed = [], [], []
        def use(kind):
            with wt.ToolSlot(name, time.monotonic()+2):
                active.append(kind)
                peaks.append(len(active))
                time.sleep(.03)
                active.remove(kind)
                completed.append(kind)
        threads = [threading.Thread(target=use, args=(kind,)) for kind in ('clang', 'xlate', 'spirv')]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(timeout=2)
            self.assertFalse(thread.is_alive())
        self.assertEqual(max(peaks), 1)
        self.assertEqual(len(completed), 3)

    def test_native_tool_slots_bound_concurrency_to_worker_count(self):
        name = 'Local\\wonder-slots-'+str(os.getpid())
        lock, active, peaks = threading.Lock(), [0], []
        def use():
            with wt.ToolSlot(name, time.monotonic()+5, 2):
                with lock:
                    active[0] += 1
                    peaks.append(active[0])
                time.sleep(.05)
                with lock:
                    active[0] -= 1
        threads = [threading.Thread(target=use) for _ in range(6)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(timeout=5)
            self.assertFalse(thread.is_alive())
        self.assertEqual(len(peaks), 6)
        self.assertLessEqual(max(peaks), 2)

    def test_shadow_route_never_constructs_production_save_transaction(self):
        game = dict(buildDir=str(self.root / 'game'), outputName='Wonder', id=wt.TITLE)
        exe, data = wv.locations(game)
        data.mkdir(parents=True)
        seed = self.root / 'seed'
        seed.mkdir()
        (seed / 'slot').write_bytes(b'original')
        build = self.root / 'build'
        build.mkdir()
        (build / 'after.json').write_text(json.dumps(dict(artifacts={})))
        def scenario(options, sc, folder, executable, lease):
            self.assertFalse(sc['save'])
            self.assertEqual(options.seconds, 330)
            self.assertTrue(lease.held)
            gs.require_clear((gs.SHARED_STOP,))
            save = Path(next(v.split('=', 1)[1] for v in options.env if v.startswith('NXOS_SAVE=')))
            self.assertEqual(save, folder / 'shadow-save')
            (save / 'slot').write_bytes(b'child changed shadow')
            (folder / 'result.json').write_text('{"why":"exited","exit":0}')
        with mock.patch.object(wv, 'EVIDENCE', self.root), \
                mock.patch.object(wv, 'HISTORICAL_SAVE', seed), \
                mock.patch.object(wt, 'entry', return_value=game), \
                mock.patch.object(wv, 'artifact_hashes', return_value={}), \
                mock.patch.object(wv, 'verified_release'), \
                mock.patch.object(wv.ps, '_scenario', side_effect=scenario), hj.MachineLease() as lease:
            wv.play(self.folder, lease)
        self.assertFalse((data / 'save').exists())
        self.assertEqual((seed / 'slot').read_bytes(), b'original')
        self.assertTrue(json.loads((self.folder / 'save-after.json').read_text())['unchanged'])


if __name__ == '__main__':
    unittest.main()
