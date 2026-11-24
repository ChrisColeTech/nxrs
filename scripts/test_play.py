"""Seconds-fast checks of play.py / play_report.py (no game, no GPU): py -m unittest scripts.test_play"""
import json
import struct
import sys
import tempfile
import unittest
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import play  # noqa: E402
import play_report  # noqa: E402


def png(path, w, h, rgb):
    raw = b''.join(b'\0' + bytes(rgb) * w for _ in range(h))
    chunk = lambda t, d: struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d))
    path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0))
                     + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b''))


class Steps(unittest.TestCase):
    def test_wait_and_repeat(self):
        self.assertEqual(play.expand(['press A ; 4 * 3', 'shot boot', 'wait 2']),
                         [('press A', 4.0)] * 3 + [('shot boot', 0.0), ('wait 2', 0.0)])
        with self.assertRaises(ValueError):
            play.expand(['  '])

    def test_every_scenario_parses_and_names_real_inputs(self):
        for name, sc in play.load_scenarios().items():
            with self.subTest(name):
                self.assertTrue(sc['title'] and sc['seconds'] > 0)
                for step, _ in play.expand(sc['steps']):
                    if step.startswith('replay '):
                        self.assertTrue((play.INPUTS / step[7:]).is_file(), step)
                lo, hi = sc['window']
                self.assertLess(lo, hi)
                self.assertLessEqual(hi, sc['seconds'])


class Report(unittest.TestCase):
    def setUp(self):
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        self.f = Path(d.name)
        (self.f / 'run.json').write_text(json.dumps(dict(scenario='t', exe='g.exe', window=[2, 4], fps=60)))

    def test_metrics_log_and_failure(self):
        (self.f / 'metrics-1.csv').write_text(
            '#seconds,frames,fps,cpu_percent,private_mb,xmon,draws_skipped,wait_sync\n'
            '1.0,10,10,100,500,0,0,3\n3.0,70,60,100,600,0,2,1\n4.0,100,30,100,700,0,1,0\n')
        (self.f / 'run.log').write_text(
            '[   1.00] [RED] lazy write-back: page 0x1000 dropped\n'
            '[   2.00] [RED] lazy write-back: page 0x2000 dropped\n'
            '[   3.00] [vk] pacing: 30.00 FPS | p95 40.00 p99 90.00 max 120.00 ms | >16.67 1\n'
            '[   4.00] [vk] VK_ERROR_DEVICE_LOST at submit\n')
        (self.f / 'result.json').write_text(json.dumps(dict(outcome='exited', exit_code=71, seconds=5,
                                                            steps_done=1, steps=3)))
        text = play_report.write(self.f)
        r = json.loads((self.f / 'report.json').read_text())
        self.assertEqual(r['exit_meaning'], 'GPU device lost (exit 71)')
        self.assertEqual(r['metrics']['fps_mean'], 45.0)
        self.assertEqual((r["metrics"]["memory_mb_start"], r["metrics"]["memory_mb_end"]), (600, 700))
        self.assertEqual(r['metrics']['counters_of_note'], {'draws_skipped': 3})
        self.assertEqual((r['log']['red_kinds'], r['log']['red_lines']), (1, 2))
        self.assertEqual(r['log']['pacing_worst']['max_ms'], 120.0)
        self.assertIn('DEVICE_LOST', ' '.join(r['log']['fault_lines']))
        self.assertIn('log_tail', r['log'])
        self.assertIn('# t: exited', text)

    def test_contact_sheet_holds_every_screenshot(self):
        for i, colour in enumerate([(255, 0, 0), (0, 0, 255)], 1):
            png(self.f / ('%02d-shot.png' % i), 64, 36, colour)
        (self.f / 'steps.jsonl').write_text(json.dumps(dict(t=12.0, ctl='fullres %s' % (self.f / '01-shot.png'))) + '\n')
        (self.f / 'result.json').write_text(json.dumps(dict(outcome='completed', exit_code=0)))
        play_report.write(self.f)
        r = json.loads((self.f / 'report.json').read_text())
        self.assertEqual(r['sheets'], ['sheet-1.png'])
        self.assertEqual(r['screenshots'], ['01-shot.png', '02-shot.png'])
        head = (self.f / 'sheet-1.png').read_bytes()[:24]
        self.assertEqual(struct.unpack('>II', head[16:24]), (2 * play_report.CELL_W, 270 + 26))


if __name__ == '__main__':
    unittest.main()
