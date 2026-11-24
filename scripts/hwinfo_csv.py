"""HWiNFO CSV CPU source: sample time AND observed append progression, never mtime.

Column positions identify occurrences within a hashed schema, not hardware IDs.
All recognized CPU package/core temperatures and protective flags are conservative
aggregates. No CSV GPU identity is inferred. Unrecognized schemas fail closed.
"""
import base64
import csv
import datetime as dt
import hashlib
import json
import math
from pathlib import Path
import re
import subprocess
import sys
import time

from harness_job import bounded_run

MAX_AGE = 12
WORKER = str(Path(__file__).with_name('hwinfo_snapshot.py'))
CONFIG = Path(__file__).resolve().parents[1] / 'build' / 'harness-monitor.json'


def configured_path(env):
    """Per-run override, otherwise ignored repository-local configuration. No global edits."""
    value = env.get('NXRS_HWINFO_CSV')
    if value is None:
        try:
            with CONFIG.open('rb') as stream:
                raw = stream.read(4097)
        except FileNotFoundError:
            return None
        if len(raw) > 4096:
            raise ValueError('oversized harness monitor configuration')
        config = json.loads(raw)
        if (not isinstance(config, dict) or set(config) != {'schema', 'hwinfo_csv'}
                or config['schema'] != 'nxrs.harness-monitor.v1'):
            raise ValueError('invalid harness monitor configuration')
        value = config['hwinfo_csv']
    if not isinstance(value, str) or not value or not Path(value).is_absolute():
        raise ValueError('HWiNFO CSV path must be a nonempty absolute path')
    return Path(value)


def acquire(path, offset):
    return json.loads(bounded_run([sys.executable, WORKER, str(path), str(offset)], timeout=1))


def decode(raw):
    try:
        return raw.decode('utf-8-sig'), 'utf-8-sig'
    except UnicodeDecodeError:
        return raw.decode('cp1252'), 'cp1252'


class Schema:
    def __init__(self, raw):
        text, self.encoding = decode(raw)
        self.names = next(csv.reader([text], strict=True))
        if ('\0' in text or not 4 <= len(self.names) <= 4096 or self.names[:2] != ['Date', 'Time']
                or self.names.count('Date') != 1 or self.names.count('Time') != 1):
            raise ValueError('invalid HWiNFO Date/Time schema')
        self.digest = hashlib.sha256(raw).hexdigest()
        self.temps, self.flags = [], []
        for index, name in enumerate(self.names):
            label = name.rsplit(' [', 1)[0]
            if name.endswith(' [°C]') and (
                    label == 'CPU' or label.startswith('CPU ')
                    or re.match(r'^(?:[PE]-)?Core(?:s|\b)', label, re.I)):
                if 'Distance' not in label and 'TjMAX' not in label:
                    self.temps.append(index)
            if (name.endswith(' [Yes/No]') and
                    re.search(r'Thermal|Critical Temperature|Power Limit|Temperature Limit', name, re.I)):
                self.flags.append(index)
        if (not any(self.names[i] == 'CPU Package [°C]' for i in self.temps)
                or not any(re.match(r'^(?:[PE]-)?Core', self.names[i], re.I) for i in self.temps)
                or not any('Thermal Throttling' in self.names[i] for i in self.flags)
                or not any('Critical Temperature' in self.names[i] for i in self.flags)):
            raise ValueError('required CPU package/core/thermal/critical columns missing')

    def parse(self, raw):
        text = raw.decode(self.encoding)
        if '\0' in text:
            raise ValueError('NUL in CSV records')
        records, repeated = [], False
        for line in text.splitlines():
            row = next(csv.reader([line], strict=True))
            if row == self.names:
                records, repeated = [], True  # logging session boundary, not a sample
                continue
            declared = len(self.names) - 1 if self.names[-1] == '' else len(self.names)
            trailer = row[declared:]
            dynamic_trailer = (self.names[-1] == '' and 0 < len(trailer) <= 64
                               and trailer[-1] == '')
            if dynamic_trailer:
                for value in trailer[:-1]:
                    number = float(value)
                    if not math.isfinite(number):
                        raise ValueError('invalid dynamic CSV trailer')
                row = row[:declared]
            if len(row) != declared:
                raise ValueError('incomplete/malformed complete CSV record')
            stamp = dt.datetime.strptime(row[0] + ' ' + row[1], '%d.%m.%Y %H:%M:%S.%f')
            epoch = stamp.timestamp()  # HWiNFO local date/time; DST rollback fails progression
            temperatures = []
            for index in self.temps:
                value = float(row[index])
                if not math.isfinite(value) or value < 0:
                    raise ValueError('invalid required temperature at column %d' % index)
                temperatures.append(dict(column=index, name=self.names[index], value=value))
            flags = []
            for index in self.flags:
                value = row[index].strip()
                if value not in ('Yes', 'No'):
                    raise ValueError('invalid protective boolean at column %d' % index)
                if value == 'Yes':
                    flags.append(dict(column=index, name=self.names[index]))
            if records and epoch <= records[-1]['epoch']:
                raise ValueError('nonincreasing CSV sample time')
            records.append(dict(epoch=epoch, sample_local=stamp.isoformat(),
                                temperatures=temperatures, flags=flags))
        return records, repeated


class Source:
    def __init__(self, path, clock=time.monotonic, wall=time.time, reader=acquire):
        self.path, self.clock, self.wall, self.reader = Path(path), clock, wall, reader
        self.reset()

    def reset(self):
        self.snapshot = self.schema = self.last = self.at = self.progress_at = None
        self.offset_clock = None
        self.proven = False

    def read(self):
        result = dict(value=None, source='HWiNFO CSV: ' + str(self.path), at=None,
                      status='unknown', detail=None)
        try:
            snap = self.reader(self.path, self.snapshot['end'] if self.snapshot else 0)
            now, wall = self.clock(), self.wall()
            raw_header = base64.b64decode(snap['header'], validate=True)
            schema = Schema(raw_header)
            changed = self.snapshot and (
                snap['identity'] != self.snapshot['identity'] or snap['reset']
                or snap['size'] < self.snapshot['size'] or snap['header'] != self.snapshot['header']
                or snap['previous'] != self.snapshot['anchor'])
            if changed:
                raise ValueError('CSV reset/rotation/rewrite; reacquire and observe new progression')
            if self.offset_clock is not None and abs((wall-now)-self.offset_clock) > 1:
                raise ValueError('wall clock movement; acquisition time untrusted')
            self.offset_clock = wall-now
            records, repeated = schema.parse(base64.b64decode(snap['data'], validate=True))
            if repeated:
                self.last, self.at, self.progress_at, self.proven = None, None, None, False
            previous = self.last
            if records:
                latest = records[-1]
                result.update(sample_local=latest['sample_local'], sample_unix=latest['epoch'])
                age = wall - latest['epoch']
                if not 0 <= age <= MAX_AGE:
                    result['status'] = 'stale'
                    raise ValueError('sample time old/future: age %.3f s' % age)
                if previous:
                    delta = latest['epoch'] - previous['epoch']
                    if records[0]['epoch'] <= previous['epoch'] or delta <= 0:
                        raise ValueError('CSV time regressed/repeated on append')
                    if delta > now-self.progress_at + MAX_AGE:
                        raise ValueError('unexpected forward sample time movement')
                    self.proven = now-self.progress_at <= MAX_AGE
                self.last = latest
                # Never re-stamp an unchanged record. Bound both clocks on advancement.
                self.at = min(now-age, self.at + delta) if previous else now-age
                self.progress_at = now
            self.snapshot, self.schema = snap, schema
            if not self.last:
                raise ValueError('no complete sample; logging may be starting/stopped')
            result.update(sample_local=self.last['sample_local'], sample_unix=self.last['epoch'],
                          schema_sha256=schema.digest, file_identity=snap['identity'],
                          sample_end=snap['end'], partial_bytes=snap['partial'],
                          observed_progression=self.proven, at=self.at,
                          observed_at=self.progress_at, encoding=schema.encoding,
                          sample_time_basis='HWiNFO calendar in host local timezone',
                          aggregation='maximum of every CPU package/core temperature occurrence; '
                                      'any CSV thermal/critical/power-limit flag vetoes; no device IDs',
                          sensors=self.last['temperatures'], flag_columns=len(schema.flags))
            # Preserve transient hot/flagged readings between polls, not only the newest row.
            considered = records if previous and records else [self.last]
            result['value'] = max(s['value'] for r in considered for s in r['temperatures'])
            result['aggregation_from_local'] = considered[0]['sample_local']
            result['aggregation_samples'] = len(considered)
            result['peak_sensors'] = [dict(s, sample_local=r['sample_local'])
                                      for r in considered for s in r['temperatures']
                                      if s['value'] == result['value']]
            result['alerts'] = [dict(f, sample_local=r['sample_local'])
                                for r in considered for f in r['flags']]
            if not self.proven:
                result['detail'] = 'awaiting observed sample progression (two separate reads required)'
            elif not 0 <= now-self.at <= MAX_AGE or now-self.progress_at > MAX_AGE:
                result.update(status='stale', detail='logging stopped/frozen or sample expired')
            else:
                result.update(status='ok', detail='sample timestamp and observed append progression verified')
        except (OSError, ValueError, KeyError, csv.Error, OverflowError,
                subprocess.TimeoutExpired) as exc:
            self.reset()
            result['detail'] = str(exc)
        return result
