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
import play_events  # noqa: E402
import play_report  # noqa: E402
import play_route  # noqa: E402


def png(path, w, h, rgb, sub_filter=False):
    """An RGB PNG of one colour (`sub_filter`: rows written with PNG filter 1, as other encoders do)."""
    if sub_filter:
        row = bytes(rgb) + bytes(3 * (w - 1))  # Sub: every texel after the first is a zero difference
        raw = b''.join(b'\1' + row for _ in range(h))
    else:
        raw = b''.join(b'\0' + bytes(rgb) * w for _ in range(h))
    chunk = lambda t, d: struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d))
    path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0))
                     + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b''))


def png_box(path, w, h, bg, box=None, fg=None):
    """An RGB PNG of colour bg with the pixel box (x0, y0, x1, y1) in colour fg (a HUD element over a scene)."""
    rows = []
    for y in range(h):
        row = bytearray(bytes(bg) * w)
        if box and box[1] <= y < box[3]:
            row[3 * box[0]:3 * box[2]] = bytes(fg) * (box[2] - box[0])
        rows.append(b'\0' + bytes(row))
    chunk = lambda t, d: struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d))
    path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0))
                     + chunk(b'IDAT', zlib.compress(b''.join(rows))) + chunk(b'IEND', b''))


LOG = '''\
[   1.00] [vk] replay up to frame 60, per frame: 1.0 draws (0.0 skipped: no shader 0, no pipeline 0, no target 0 in all), x; dropped work 0
[   2.00] [vk] replay up to frame 120, per frame: 1.0 draws (0.0 skipped: no shader 0, no pipeline 10, no target 5 in all), x; dropped work 0
[   3.00] [vk] replay up to frame 180, per frame: 1.0 draws (0.0 skipped: no shader 0, no pipeline 20, no target 10 in all), x; dropped work 0
[   4.00] [vk] replay up to frame 240, per frame: 1.0 draws (0.0 skipped: no shader 0, no pipeline 30, no target 60 in all), x; dropped work 0
[   4.10] [vk] texture ID 7 (target 4) at frame 230: nothing is registered there: a zero stand-in is bound
[   4.20] [vk] texture pool: 1 slot(s) of an image that goes now name the zero stand-in (NXGFX_SCRUB_SLOTS=0 off)
[   4.30] [RED] a draw with no render target bound (1 colour slots, 1 with a texture, depth set): skipped
[   4.40] [vk] translate (parallel), over the last 600 frames: 3 streams; 2 segments translated again; the gate asked 9 times, 1 segments gave up; asked at
[   4.50] [vk] pools: x; CPU writes held back a frame: 0.50 MB under in-flight write-backs (1.0 syncs), 0.00 MB past the upload budget; y
[   4.60] [vk] fault capture 1: stand-in: texture ID 7 at present 241: f.png (8x8)
[   4.70] [vk] screenshot: present 241 (8x8) written to C:\\run\\01-race.png (1.0 ms)
[  20.00] [RED] lazy write-back: page 0x1000 dropped
'''


class Events(unittest.TestCase):
    def test_kinds_times_and_screenshot_present(self):
        ev, shots = play_events.parse(LOG.splitlines())
        ev = [e for e in ev if e[2]]  # quiet periods (count 0) are kept for the steady rates
        kinds = [e[1] for e in ev]
        self.assertEqual(kinds, ['skip-no-pipeline', 'skip-no-target', 'skip-no-pipeline', 'skip-no-target',
                                 'skip-no-pipeline', 'skip-no-target', 'stand-in', 'no-target', 'gate',
                                 'held-back-frame', 'capture', 'red'])
        stand_in = ev[6]
        self.assertEqual((stand_in[0], stand_in[3]), (4.1, 230))
        self.assertIn('(target 4 = 2D array)', stand_in[4])
        self.assertEqual(ev[8][2], 3)  # gave up + translated again
        self.assertEqual(ev[9][2], 512)  # KB held back
        self.assertEqual(shots, {'01-race.png': (4.7, 241)})

    def test_steady_skips_are_background_and_spikes_are_near(self):
        ev, _ = play_events.parse(LOG.splitlines())
        s = play_events.summary(ev, (3.5, 10))
        self.assertEqual((s['skip-no-target']['total'], s['skip-no-target']['in_window']), (60, 50))
        self.assertEqual((s['skip-no-target']['steady'], s['skip-no-target']['peak']), (5, 50))
        saved = play_events.NEAR_BEFORE
        play_events.NEAR_BEFORE = 1.0  # the shot's second only: the log's first periods have no baseline yet
        self.addCleanup(setattr, play_events, 'NEAR_BEFORE', saved)
        near = dict((k, n) for k, n, _ in play_events.near(ev, 4.7))
        # the steady 10 pipelines a period are background; the 50-draw no-target spike is news
        self.assertNotIn('skip-no-pipeline', near)
        self.assertEqual((near['skip-no-target'], near['stand-in'], near['no-target']), (50, 1, 1))
        self.assertNotIn('red', near)


class AB(unittest.TestCase):
    def test_serial_and_lanes_side_by_side(self):
        with tempfile.TemporaryDirectory() as d:
            runs = [Path(d) / 's', Path(d) / 'l']
            for f, n in zip(runs, (0, 7)):
                f.mkdir()
                kinds = {'gate': dict(total=n, in_window=n)} if n else {}
                (f / 'report.json').write_text(json.dumps(dict(
                    outcome='completed', off_route=[], metrics=dict(fps_mean=30.0),
                    anomalies=dict(kinds=kinds), route=[dict(shot='01-a.png', hist=0.05 * (n > 0))])))
            text = play.ab_report('t', runs)
            self.assertIn('| gate (run / window) | - | 7 / 7 |', text)
            self.assertIn('| route `01-a.png` | 0.0 | 0.05 |', text)
            self.assertTrue((runs[1] / 'ab.md').exists())


class Route(unittest.TestCase):
    def setUp(self):
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        self.f = Path(d.name)
        self.saved = play_route.CHECKPOINTS
        play_route.CHECKPOINTS = self.f / 'checkpoints.json'
        self.addCleanup(setattr, play_route, 'CHECKPOINTS', self.saved)

    def test_filtered_and_plain_rows_decode_the_same(self):
        png(self.f / 'a.png', 40, 4, (200, 30, 90))
        png(self.f / 'b.png', 40, 4, (200, 30, 90), sub_filter=True)
        self.assertEqual(play_route.decode(self.f / 'a.png')[3], play_route.decode(self.f / 'b.png')[3])
        self.assertEqual(play_route.distance(play_route.signature(self.f / 'a.png'),
                                             play_route.signature(self.f / 'b.png')), (0.0, 0.0))

    def test_reference_then_off_route_report(self):
        good, bad = self.f / 'good', self.f / 'bad'
        for run, colour in ((good, (40, 160, 40)), (bad, (90, 60, 30))):
            run.mkdir()
            png(run / '01-house.png', 64, 36, colour)
            png(run / '02-other.png', 64, 36, colour)
        self.assertEqual(play_route.record(good, 't', {'house': 'in the house'}), ['house'])
        self.assertFalse(play_route.check(good, 't', {'house': 'x'}, [good / '01-house.png'])[0]['off'])
        (bad / 'run.json').write_text(json.dumps(dict(scenario='t', exe='g.exe', window=[1, 3], fps=60,
                                                      checks={'house': 'in the house'})))
        (bad / 'metrics-1.csv').write_text('#seconds,frames,fps,cpu_percent,private_mb,xmon\n2.0,60,60,1,5,0\n')
        (bad / 'result.json').write_text(json.dumps(dict(outcome='completed', exit_code=0)))
        saved = play_report.sheets
        play_report.sheets = lambda folder, shots: []  # no PowerShell: the sheet has its own test
        self.addCleanup(setattr, play_report, 'sheets', saved)
        text = play_report.write(bad)
        self.assertIn('# t: completed - OFF-ROUTE', text)
        self.assertIn('**INVALID (off route)** FPS', text)
        self.assertEqual(json.loads((bad / 'report.json').read_text())['off_route'], ['01-house.png'])

    def test_region_check_follows_the_hud_not_the_scene(self):
        # a 100x50 frame; the HUD region is the box [0.1, 0.1, 0.3, 0.5] = pixels (10, 5)-(30, 25)
        check = {'race': {'about': 'in the race', 'regions': {'hud': [0.1, 0.1, 0.3, 0.5, 0.6]}}}
        hud, white = (10, 5, 30, 25), (250, 250, 250)
        runs = {n: self.f / n for n in ('ref', 'elsewhere', 'bleed', 'menu')}
        for r in runs.values():
            r.mkdir()
        png_box(runs['ref'] / '05-race.png', 100, 50, (40, 160, 40), hud, white)  # green track
        png_box(runs['elsewhere'] / '05-race.png', 100, 50, (200, 120, 60), hud, white)  # sand: another spot
        png_box(runs['bleed'] / '05-race.png', 100, 50, (200, 120, 60), (10, 5, 30, 20), white)  # 3/4 of the HUD
        png_box(runs['menu'] / '05-race.png', 100, 50, (200, 120, 60))  # no HUD at all
        self.assertEqual(play_route.record(runs['ref'], 't', check), ['race'])
        got = {n: play_route.check(r, 't', check, [r / '05-race.png'])[0] for n, r in runs.items()}
        self.assertEqual(got['elsewhere']['regions'], {'hud': (1.0, 0.6)})
        self.assertFalse(got['elsewhere']['off'])
        # the whole-frame histogram would call the other spot off route
        sig = play_route.signature(runs['elsewhere'] / '05-race.png')
        self.assertGreater(play_route.distance(sig, play_route.load()['t']['race'])[0], play_route.OFF_ROUTE_HIST)
        self.assertFalse(got['bleed']['off'])
        self.assertEqual(got['bleed']['regions']['hud'][0], 0.67)
        self.assertTrue(got['menu']['off'])
        self.assertEqual(play_route.describe(got['menu']), 'hud 0.0 (min 0.6)')

    def test_region_check_needs_a_reference_with_regions(self):
        png_box(self.f / '05-race.png', 100, 50, (40, 160, 40))
        play_route.record(self.f, 't', {'race': 'whole frame'})
        got = play_route.check(self.f, 't', {'race': {'about': 'x', 'regions': 'kart-hud'}}, [self.f / '05-race.png'])
        self.assertIsNone(got[0]['off'])
        self.assertIn('record it again', got[0]['why'])

    def test_bad_region_specs_are_refused(self):
        with self.assertRaises(ValueError):
            play_route.spec({'regions': 'no-such-set'})
        with self.assertRaises(ValueError):
            play_route.spec({'regions': {'hud': [0.5, 0.1, 0.3, 0.5, 0.6]}})  # x1 before x0


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
                # route checks name shots the steps take (as play.py names their files)
                shots = {play.shot_file(step[5:]) for step, _ in play.expand(sc['steps']) if step.startswith('shot ')}
                self.assertLessEqual(set(sc.get('checks', {})), shots)
                for value in sc.get('checks', {}).values():
                    play_route.spec(value)  # every check is a valid whole-frame or region check


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
