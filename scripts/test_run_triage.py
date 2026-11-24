import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import gamectl
import run_game
import run_triage


class TriageTests(unittest.TestCase):
    def test_interleaved_pipeline_calls_and_incomplete_log(self):
        text = '\n'.join([
            '[ 1.000] [vk] pipeline compute compiling on replay: 0123456789abcdef',
            '[ 2.000] [vk] pipeline graphics compiling on worker 0: fedcba9876543210',
            '[ 3.000] [vk] pipeline graphics made on worker 0: fedcba9876543210',
            '[ 10.000] [vk] wait-diag lane=1 phase=dispatch: compute pipeline',
        ])
        report = run_triage.pipeline_events(text)
        self.assertEqual(report['completed'], 1)
        self.assertEqual(report['slowest'][0]['elapsed_ms'], 1000)
        self.assertEqual(report['unfinished'][0]['shader_keys'], ['0123456789abcdef'])
        self.assertEqual(report['unfinished'][0]['observed_pending_ms'], 9000)

    def test_frame_costs_are_windowed_and_missing_stays_unknown(self):
        with tempfile.TemporaryDirectory() as tmp:
            run = Path(tmp)
            (run / 'frames-1.csv').write_text(
                '#present,guest_s,frame_ms,draws,draws_skipped,replay_ms,gpu_ms\n'
                '1,1,1000,50,40,999,500\n2,10,100,50,2,80,5\n3,11,100,50,3,90,7\n4,12,')
            report = run_triage.generate(run, '', (10, 11))
            self.assertEqual(report['draws_skipped'], 5)
            self.assertEqual(report['frame_costs']['replay_ms']['mean'], 85)
            self.assertIsNone(report['frame_costs']['tex_upload_ms']['mean'])
            self.assertEqual(len(report['findings']), 2)

    def test_binary_fingerprints_and_environment_filter(self):
        with tempfile.TemporaryDirectory() as tmp:
            run = Path(tmp)
            exe = run / 'game.exe'
            exe.write_bytes(b'executable')
            result = run_triage.provenance(run, exe, [], {'NXGFX_WAIT_DIAG': '1',
                'NXRS_SECRET_TOKEN': 'private', 'API_PASSWORD': 'private'})
            self.assertEqual(result['settings'], {'NXGFX_WAIT_DIAG': '1'})
            self.assertEqual(len(result['files'][0]['sha256']), 64)
            self.assertEqual(result['files'][1]['status'], 'missing')

    def test_snapshot_records_timeouts_and_keeps_collecting(self):
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(gamectl, 'pipe', side_effect=[TimeoutError('blocked'), 'threads', 'gaps', 'counters']):
                events = run_game.diagnostic_snapshot(123, tmp, 'test', capture=False)
            self.assertEqual([e['status'] for e in events], ['timeout', 'ok', 'ok', 'ok'])
            saved = json.loads((Path(tmp) / 'diagnostics-test-requests.json').read_text())
            self.assertEqual(len(saved), 4)

    def test_fault_report_keeps_allocation_history_and_shader_identity(self):
        with tempfile.TemporaryDirectory() as tmp:
            run = Path(tmp)
            shader = run / 'game_data/shaders/spv/6098504f5d12ea03.spv'
            shader.parent.mkdir(parents=True)
            shader.write_bytes(b'fixture shader')
            shader.with_suffix('.json').write_text(json.dumps({'stage': 'ts', 'bindings': []}))
            (run / 'provenance.json').write_text(json.dumps({'data': str(run / 'game_data')}))
            report = run_triage.generate(run, '\n'.join([
                '[ 23.47] [RED] the GPU started #94396 a mesh draw with shaders [3:6098504f5d12ea03]',
                '[ 23.47] [RED] fault address: invalid read at 0xd434b0000',
                '[ 23.47] [RED] inside BUFFER "staging", UNBOUND at 21.13 s',
                'earlier: BUFFER "pool", UNBOUND at 15.64 s']))
            self.assertEqual(len(report['gpu_fault_context']), 4)
            self.assertEqual(report['shader_evidence'][0]['reflection']['stage'], 'ts')
            self.assertEqual(len(report['shader_evidence'][0]['spirv']['sha256']), 64)
            self.assertFalse(report['pipelines']['logging_observed'])


@unittest.skipUnless(sys.platform == 'win32', 'Windows named pipe test')
class ControlDeadlineTests(unittest.TestCase):
    def test_connected_server_never_replying_is_bounded_and_helper_reaped(self):
        self.exchange(None)

    def test_normal_reply_and_server_disconnect_preserve_reply(self):
        self.exchange(b'frame 42\nseconds 1.0\n')

    def exchange(self, response):
        # Real pipe exchange and job membership. A separate idle server is itself
        # bounded, so failed assertions cannot hang in synchronous CloseHandle.
        import harness_job
        fixture = Path(__file__).with_name('harness_idle_fixture.py')
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'pipe.log'
            job = harness_job.Job(3)
            child = None
            start = time.monotonic()
            try:
                with path.open('w') as output:
                    child = job.launch([sys.executable, str(fixture),
                                        'pipe' if response is None else 'pipe-reply'], output)
                    until = time.monotonic() + 1
                    while 'ready' not in path.read_text() and time.monotonic() < until:
                        time.sleep(.01)
                    self.assertIn('ready', path.read_text())
                    with mock.patch.object(gamectl, 'check_owned', return_value=job.name):
                        if response is None:
                            with self.assertRaisesRegex(TimeoutError, 'delivery unknown'):
                                gamectl.pipe(child.pid, 'status', timeout=.75)
                        else:
                            self.assertEqual(gamectl.pipe(child.pid, 'status', timeout=.75), response.decode())
                    self.assertLess(time.monotonic() - start, 3)
            finally:
                job.close()
                if child:
                    self.assertIsNotNone(child.poll())
                    child.close()


if __name__ == '__main__':
    unittest.main()
