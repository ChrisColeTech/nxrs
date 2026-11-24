"""Synthetic CSV-only tests; no owner log, telemetry, games or production saves."""
import base64
import csv
import datetime as dt
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import hwinfo_csv as hw
import hwinfo_snapshot as snap
import harness_monitor as hm

HEAD = ['Date', 'Time', 'CPU Package [°C]', 'Core Max [°C]', 'CPU Package [°C]',
        'P-core 0 [°C]', 'E-core 1 [°C]', 'Core Thermal Throttling [Yes/No]',
        'Core Critical Temperature [Yes/No]', 'Core Power Limit Exceeded [Yes/No]',
        'GPU Temperature [°C]', 'GPU Temperature [°C]', 'Other, quoted sensor']


def csv_line(fields, encoding='cp1252'):
    output = io.StringIO(newline='')
    csv.writer(output).writerow(fields)
    return output.getvalue().encode(encoding)


class Feed:
    def __init__(self, path, wall=None):
        self.path = Path(path)
        self.wall = wall if wall is not None else dt.datetime(2026, 11, 23, 9).timestamp()
        self.now = 100.
        self.head = list(HEAD)
        self.path.write_bytes(csv_line(self.head))

    def row(self, stamp=None, **values):
        date = dt.datetime.fromtimestamp(self.wall if stamp is None else stamp)
        fields = [date.strftime('%d.%m.%Y'), date.strftime('%H:%M:%S.%f'),
                  '45', '46', '47', '44', '43', 'No', 'No', 'No', '55', '65', 'text']
        for index, value in values.items():
            fields[int(index[1:])] = str(value)
        return csv_line(fields)

    def append(self, seconds=0, **values):
        self.now += seconds
        self.wall += seconds
        with self.path.open('ab') as stream:
            stream.write(self.row(**values))

    def source(self, **kwargs):
        return hw.Source(self.path, clock=lambda: self.now, wall=lambda: self.wall,
                         reader=kwargs.get('reader', snap.snapshot))

    def proven(self):
        self.append()
        source = self.source()
        assert source.read()['status'] == 'unknown'
        self.append(2)
        assert source.read()['status'] == 'ok'
        return source


class CsvTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.feed = Feed(Path(directory.name) / 'synthetic.csv')

    def test_requires_two_observations_not_two_historical_rows(self):
        f = self.feed
        f.append()
        f.append(2)
        source = f.source()
        self.assertEqual(source.read()['status'], 'unknown')
        self.assertEqual(source.read()['status'], 'unknown')
        f.append(2)
        self.assertEqual(source.read()['status'], 'ok')

    def test_duplicate_package_and_hybrid_cores_are_not_overwritten(self):
        f = self.feed
        source = f.proven()
        f.append(2, c2=87, c4=41, c5=88, c6=89)
        value = source.read()
        self.assertEqual(value['value'], 89)
        self.assertEqual(len(value['sensors']), 5)
        self.assertEqual([s['column'] for s in value['sensors'] if s['name'] == HEAD[2]], [2, 4])
        self.assertNotIn('GPU', str(value['sensors']))

    def test_frozen_and_mtime_only_updates_do_not_refresh_acquisition(self):
        f = self.feed
        source = f.proven()
        original = source.read()['at']
        f.now += 11
        f.wall += 11
        os.utime(f.path, None)
        self.assertEqual(source.read()['at'], original)
        f.now += 2
        f.wall += 2
        value = source.read()
        self.assertEqual(value['status'], 'stale')
        self.assertEqual(value['at'], original)

    def test_old_copy_with_new_mtime_is_stale(self):
        f = self.feed
        f.append()
        f.now += 30
        f.wall += 30
        os.utime(f.path, None)
        self.assertIn('old/future', f.source().read()['detail'])

    def test_partial_final_write_uses_last_complete_without_renewal(self):
        f = self.feed
        source = f.proven()
        at = source.read()['at']
        f.now += 2
        f.wall += 2
        row = f.row(c2=90)
        with f.path.open('ab') as stream:
            stream.write(row[:24])
        value = source.read()
        self.assertEqual(value['status'], 'ok')
        self.assertEqual(value['partial_bytes'], 24)
        self.assertEqual(value['at'], at)
        with f.path.open('ab') as stream:
            stream.write(row[24:])
        self.assertEqual(source.read()['value'], 90)

    def test_partial_first_record_is_not_a_sample(self):
        with self.feed.path.open('ab') as stream:
            stream.write(self.feed.row()[:-2])
        self.assertIn('no complete sample', self.feed.source().read()['detail'])

    def test_complete_malformed_row_cannot_fall_back_to_cool_row(self):
        source = self.feed.proven()
        with self.feed.path.open('ab') as stream:
            stream.write(b'23.11.2026,09:00:04.000,45\r\n')
        self.assertIn('malformed', source.read()['detail'])

    def test_bounded_numeric_fields_after_empty_header_marker_are_ignored(self):
        f = self.feed
        f.head.append('')
        f.path.write_bytes(csv_line(f.head))
        base = next(csv.reader([f.row().decode('cp1252')]))
        with f.path.open('ab') as stream:
            stream.write(csv_line([*base, '1.25', '']))
        source = f.source()
        self.assertEqual(source.read()['status'], 'unknown')
        f.now += 2
        f.wall += 2
        base = next(csv.reader([f.row().decode('cp1252')]))
        with f.path.open('ab') as stream:
            stream.write(csv_line([*base, '2.5', '3.5', '']))
        self.assertEqual(source.read()['status'], 'ok')

        with f.path.open('ab') as stream:
            stream.write(csv_line([*base, 'not-a-number', '']))
        self.assertEqual(source.read()['status'], 'unknown')

    def test_required_bad_nonfinite_missing_and_boolean_values_refuse(self):
        for column, value in ((2, 'NaN'), (3, 'inf'), (4, ''), (5, -1), (6, 'oops'),
                              (7, 'maybe'), (8, ''), (9, '1')):
            with self.subTest(column=column, value=value):
                f = self.feed
                f.path.write_bytes(csv_line(HEAD))
                f.append(**{'c%d' % column: value})
                self.assertEqual(f.source().read()['status'], 'unknown')

    def test_each_protective_flag_vetoes_monitor(self):
        for column in (7, 8, 9):
            with self.subTest(column=column):
                f = self.feed
                f.path.write_bytes(csv_line(HEAD))
                source = f.proven()
                f.append(2, **{'c%d' % column: 'Yes'})
                cpu = source.read()
                sample = {k: hm.reading(v, 'synthetic', f.now, 'ok') for k, v in
                          dict(gpu_c=40, cpu_load=1, gpu_load=1, vram_mb=100).items()}
                sample.update(cpu_c=cpu, vram_total_mb=8000)
                self.assertIn('protective flag', hm.Monitor(clock=lambda: f.now).check(sample))

    def test_hot_or_flagged_intermediate_sample_is_not_hidden_by_latest_cool(self):
        f = self.feed
        source = f.proven()
        f.append(2, c2=90, c7='Yes')
        f.append(2)
        value = source.read()
        self.assertEqual(value['value'], 90)
        self.assertTrue(value['alerts'])

    def test_utf8_bom_ansi_quoted_and_reordered_headers(self):
        f = self.feed
        for encoding in ('cp1252', 'utf-8-sig'):
            order = [0, 1, 4, 6, 5, 7, 8, 9, 3, 2, 10, 11, 12]
            header = [HEAD[i] for i in order]
            fields = next(csv.reader([f.row(c2=88).decode('cp1252')]))
            f.path.write_bytes(csv_line(header, encoding) +
                               csv_line([fields[i] for i in order], encoding.replace('-sig', '')))
            value = f.source().read()
            self.assertEqual(value['value'], 88)
            self.assertEqual(value['encoding'], encoding)

    def test_missing_file_and_bad_schema_fail_closed(self):
        self.feed.path.unlink()
        self.assertEqual(self.feed.source().read()['status'], 'unknown')
        for head in (['Date', 'Time'], ['Time', 'Date', *HEAD[2:]], HEAD[:8]):
            self.feed.path.write_bytes(csv_line(head))
            value = self.feed.source().read()
            self.assertEqual(value['status'], 'unknown')
            self.assertRegex(value['detail'], 'schema|columns missing')

    def test_truncation_requires_new_baseline_then_progression(self):
        f = self.feed
        source = f.proven()
        f.path.write_bytes(csv_line(HEAD) + f.row())
        self.assertIn('reset/rotation', source.read()['detail'])
        self.assertEqual(source.read()['status'], 'unknown')
        f.append(2)
        self.assertEqual(source.read()['status'], 'ok')

    def test_same_size_rewrite_and_rotation_invalidate_previous_trust(self):
        for rotate in (False, True):
            with self.subTest(rotate=rotate):
                f = self.feed
                f.path.write_bytes(csv_line(HEAD))
                source = f.proven()
                raw = f.path.read_bytes().replace(b',47,', b',48,')
                if rotate:
                    replacement = f.path.with_suffix('.replacement')
                    replacement.write_bytes(raw)
                    os.replace(replacement, f.path)
                else:
                    f.path.write_bytes(raw)
                self.assertIn('reset/rotation', source.read()['detail'])

    def test_repeated_header_reestablishes_progression_footer_refuses(self):
        f = self.feed
        source = f.proven()
        with f.path.open('ab') as stream:
            stream.write(csv_line(HEAD))
        self.assertEqual(source.read()['status'], 'unknown')
        f.append(2)
        self.assertEqual(source.read()['status'], 'unknown')
        f.append(2)
        self.assertEqual(source.read()['status'], 'ok')
        with f.path.open('ab') as stream:
            stream.write(b'Logging stopped\r\n')
        self.assertEqual(source.read()['status'], 'unknown')

    def test_midnight_month_and_year_rollover_are_valid(self):
        f = self.feed
        f.wall = dt.datetime(2026, 12, 31, 23, 59, 59).timestamp()
        source = f.proven()
        value = source.read()
        self.assertEqual(value['status'], 'ok')
        self.assertTrue(value['sample_local'].startswith('2027-01-01'))

    def test_backward_duplicate_and_future_sample_movement_refuses(self):
        for shift in (-2, 0, 10):
            f = self.feed
            f.path.write_bytes(csv_line(HEAD))
            source = f.proven()
            with f.path.open('ab') as stream:
                stream.write(f.row(stamp=f.wall+shift))
            self.assertNotEqual(source.read()['status'], 'ok')

    def test_wall_clock_jump_cannot_rejuvenate_frozen_sample(self):
        f = self.feed
        source = f.proven()
        f.wall -= 5
        self.assertIn('wall clock movement', source.read()['detail'])

    def test_long_observation_gap_requires_progression_again(self):
        f = self.feed
        source = f.proven()
        f.append(13)
        self.assertEqual(source.read()['status'], 'unknown')
        f.append(2)
        self.assertEqual(source.read()['status'], 'ok')

    def test_header_and_incremental_backlog_bounds(self):
        f = self.feed
        source = f.proven()
        with f.path.open('ab') as stream:
            stream.write(b'x' * (snap.TAIL_BYTES+1))
        self.assertIn('backlog', source.read()['detail'])
        f.path.write_bytes(b'x' * (snap.HEADER_BYTES+1))
        self.assertIn('header', source.read()['detail'])

    def test_initial_snapshot_only_reads_bounded_tail(self):
        f = self.feed
        with f.path.open('ab') as stream:
            stream.write(f.row() * 5000)
        value = snap.snapshot(f.path)
        self.assertGreater(value['start'], len(csv_line(HEAD)))
        self.assertLessEqual(len(base64.b64decode(value['data'])), snap.TAIL_BYTES)

    def test_invalid_config_never_falls_back_to_wmi(self):
        provider = hm.SensorProvider({'NXRS_HWINFO_CSV': ''})
        with mock.patch.object(hm, 'sensors', side_effect=lambda clock, cpu: cpu):
            self.assertIn('absolute path', provider()['detail'])

    def test_repository_config_and_per_run_override_are_resolved(self):
        config = self.feed.path.with_suffix('.json')
        config.write_text('{"schema":"nxrs.harness-monitor.v1","hwinfo_csv":' +
                          json.dumps(str(self.feed.path)) + '}')
        with mock.patch.object(hw, 'CONFIG', config):
            self.assertEqual(hw.configured_path({}), self.feed.path)
            other = self.feed.path.with_suffix('.other')
            self.assertEqual(hw.configured_path({'NXRS_HWINFO_CSV': str(other)}), other)


if __name__ == '__main__':
    unittest.main()
