"""Small fail-closed monitoring contract; sensors and monotonic clock are injectable.

Fresh query receipt is NOT proof of freshness of a cached CPU sensor value. ACPI
thermal zones are deliberately excluded. No utilization-only leak diagnosis.
"""
import json
import math
import os
from pathlib import Path
import re
import subprocess
import time

from harness_job import bounded_run
import hwinfo_csv


def reading(value=None, source='unavailable', at=None, status='unknown', detail=None):
    return dict(value=value, source=source, at=at, status=status, detail=detail)


def powershell(command):
    return bounded_run(['pwsh', '-NoProfile', '-NonInteractive', '-Command',
                        "$ErrorActionPreference='Stop'; " + command], timeout=3)


def sensors(clock=time.monotonic, cpu=None):
    """Existing providers only. WMI hardware-monitor caches lack acquisition timestamps."""
    result = {key: reading() for key in ('cpu_c', 'gpu_c', 'cpu_load', 'gpu_load', 'vram_mb')}
    errors = []
    if cpu is not None:
        result['cpu_c'] = cpu
    for namespace in (() if cpu is not None else
                      ('root/LibreHardwareMonitor', 'root/OpenHardwareMonitor')):
        try:
            rows = json.loads(powershell(
                "@(Get-CimInstance -Namespace '%s' -ClassName Sensor | "
                "Where-Object { $_.SensorType -eq 'Temperature' -and "
                "$_.Parent -match '^/(intelcpu|amdcpu)/' } | "
                "Select-Object Name,Parent,Value) | ConvertTo-Json -Compress" % namespace) or '[]')
            if isinstance(rows, dict):
                rows = [rows]
            values = [float(row['Value']) for row in rows if row.get('Value') is not None]
            if values:
                result['cpu_c'] = reading(max(values), namespace, detail=
                    'CPU-identified provider value; acquisition timestamp unavailable; freshness unverified')
                break
        except (OSError, ValueError, KeyError, subprocess.TimeoutExpired) as exc:
            errors.append('%s: %s' % (namespace, exc))
    if cpu is None and result['cpu_c']['value'] is None:
        result['cpu_c']['detail'] = '; '.join(errors) or 'No CPU temperature sensors'
    try:
        at = clock()
        load = float(powershell('(Get-CimInstance Win32_Processor | '
                               'Measure-Object LoadPercentage -Average).Average'))
        result['cpu_load'] = reading(load, 'Win32_Processor.LoadPercentage', at, 'ok')
    except (OSError, ValueError, subprocess.TimeoutExpired) as exc:
        result['cpu_load']['detail'] = str(exc)
    try:
        at = clock()
        out = bounded_run(['nvidia-smi', '--query-gpu=temperature.gpu,utilization.gpu,memory.used,memory.total',
                           '--format=csv,noheader,nounits'], timeout=2)
        rows = [[float(x.strip()) for x in line.split(',')] for line in out.splitlines()]
        if not rows or any(len(row) != 4 or any(not math.isfinite(v) or v < 0 for v in row)
                           for row in rows):
            raise ValueError('invalid NVIDIA telemetry')
        for key, column in (('gpu_c', 0), ('gpu_load', 1), ('vram_mb', 2)):
            result[key] = reading(max(row[column] for row in rows), 'nvidia-smi', at, 'ok',
                                  'device-wide maximum across %d NVIDIA device(s); '
                                  'capacity uses minimum; no per-process attribution' % len(rows))
        result['vram_total_mb'] = min(row[3] for row in rows)
    except (OSError, ValueError, subprocess.TimeoutExpired) as exc:
        for key in ('gpu_c', 'gpu_load', 'vram_mb'):
            result[key]['detail'] = str(exc)
    return result


class SensorProvider:
    """One progression history per run, shared by preflight/resume/runtime checks."""
    def __init__(self, env=None, clock=time.monotonic):
        self.clock, self.first, self.error, self.cpu = clock, True, None, None
        try:
            path = hwinfo_csv.configured_path(os.environ if env is None else env)
            if path is not None:
                self.cpu = hwinfo_csv.Source(path, clock=clock)
        except (OSError, ValueError) as exc:
            self.error = str(exc)

    def __call__(self):
        cpu = self.cpu.read() if self.cpu else None
        # Only initial preflight may observe startup progression. No waiting at resume
        # or during a run. Every disk read has its own independent one-second job.
        if self.first and cpu is not None:
            until = time.monotonic() + 3
            while (cpu['status'] == 'unknown' and cpu.get('sample_local')
                   and not cpu.get('alerts') and (cpu.get('value') or 0) < 85
                   and time.monotonic() < until):
                time.sleep(.2)
                cpu = self.cpu.read()
        self.first = False
        if self.error:
            cpu = reading(source='HWiNFO configuration', detail=self.error)
        return sensors(self.clock, cpu=cpu)


class Monitor:
    def __init__(self, provider=None, clock=time.monotonic, env=None):
        self.provider = provider if provider is not None else SensorProvider(env, clock)
        self.clock = clock
        self.history = []
        self.last_progress = clock()
        self.frames = None

    def sample(self):
        return self.provider()

    def check(self, sample, private_mb=None, frames=None):
        now = self.clock()
        for key in ('cpu_c', 'gpu_c', 'cpu_load', 'gpu_load', 'vram_mb'):
            item = sample.setdefault(key, reading())
            at = item.get('at')
            if item.get('status') == 'ok':
                if not isinstance(at, (int, float)) or not math.isfinite(at):
                    item.update(status='unknown', detail='acquisition time unavailable')
                elif not 0 <= now-at <= 12:
                    item.update(status='stale', detail='sample age %.3f s' % (now-at))
        for key, maximum in (('cpu_c', 85), ('gpu_c', 80), ('cpu_load', 100),
                             ('gpu_load', 100), ('vram_mb', 20000)):
            item = sample.get(key, reading())
            value, at = item.get('value'), item.get('at')
            if (item.get('status') != 'ok' or not isinstance(value, (int, float))
                    or not math.isfinite(value) or value < 0):
                status = item.get('status') if item.get('status') != 'ok' else 'unknown'
                item['status'] = status
                return '%s %s: %s' % (key, status, item.get('detail'))
            if not isinstance(at, (int, float)) or not math.isfinite(at) or not 0 <= now-at <= 12:
                return '%s stale/unknown acquisition time' % key
            if value >= maximum and key not in ('cpu_load', 'gpu_load'):
                return '%s cap: measured %s >= %s' % (key, value, maximum)
            if key in ('cpu_load', 'gpu_load') and value > 100:
                return '%s invalid percentage' % key
            if item.get('alerts'):
                return '%s protective flag: %s' % (key, item['alerts'])
        total = sample.get('vram_total_mb')
        if not isinstance(total, (int, float)) or not math.isfinite(total) or total <= 0:
            return 'VRAM capacity unknown'
        vram = sample['vram_mb']['value']
        if vram >= total * .9:
            return 'VRAM cap: measured %s MB / %s MB (90%%)' % (vram, total)
        if private_mb is None:
            return None  # preflight
        if private_mb >= 20000:
            return 'private memory cap: measured %s MB' % private_mb
        if frames is not None:
            if self.frames is not None and frames < self.frames:
                return 'frame counter regressed: %s < %s; progress unknown' % (frames, self.frames)
            if self.frames is None or frames > self.frames:
                self.frames, self.last_progress = frames, now
        stalled = now - self.last_progress >= 30
        self.history.append((now, private_mb, vram))
        while len(self.history) > 1 and self.history[1][0] <= now - 30:
            self.history.pop(0)
        first = self.history[0]
        if now - first[0] >= 30:
            for index, name in ((1, 'private memory'), (2, 'VRAM')):
                growth = self.history[-1][index] - first[index]
                if growth >= 1024 and (stalled or growth / (now-first[0]) >= 100):
                    return '%s sustained growth: %s MB in %.1f s; progress stalled=%s' % (
                        name, growth, now-first[0], stalled)
        if stalled:
            return ('progress absent/stalled for %.1f s; CPU load=%s%% GPU load=%s%%; '
                    'no leak inferred') % (now-self.last_progress, sample['cpu_load']['value'],
                                          sample['gpu_load']['value'])
        return None


class RunLog:
    """Incremental, bounded log reader. Falling behind is a monitor fault, not safe."""
    FAULT = re.compile(r'(?:VK_)?ERROR_DEVICE_LOST|GPU completion watchdog|GPU DEVICE LOST|device lost|GPU fault|'
                       r'out of device memory|VK_ERROR_OUT_OF_DEVICE_MEMORY|'
                       r'VK_ERROR_OUT_OF_HOST_MEMORY|fatal error|panicked at', re.I)

    def __init__(self, path):
        self.path, self.offset, self.tail = Path(path), 0, ''

    def fault(self):
        with self.path.open('rb') as stream:
            stream.seek(self.offset)
            chunk = stream.read(256 * 1024 + 1)
            self.offset = stream.tell()
        if len(chunk) > 256 * 1024:
            return 'log observation backlog exceeded 256 KiB'
        text = self.tail + chunk.decode('utf-8', 'replace')
        self.tail = text[-256:]
        match = self.FAULT.search(text)
        return 'direct log fault: ' + match[0] if match else None


def progress(folder):
    """Frame counter from bounded tails; duplicate/stale rows cannot renew progress."""
    counters = []
    for path in Path(folder).glob('metrics-*.csv'):
        with path.open('rb') as stream:
            stream.seek(0, 2)
            stream.seek(max(0, stream.tell() - 8192))
            lines = stream.read(8192).decode('ascii', 'replace').splitlines()
        for line in reversed(lines[:-1]):  # avoid accepting a partially written final row
            fields = line.split(',')
            try:
                float(fields[0])
                counters.append(int(fields[1]))
                break
            except (ValueError, IndexError):
                continue
    return sum(counters) if counters else None
