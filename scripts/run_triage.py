"""Bounded evidence index for stalled/incorrect frames; no inferred gameplay verdict.

py scripts/run_triage.py RUN [--window START,END]
Reprocesses retained evidence without launching a game or touching its data.
"""
import argparse
import collections
import hashlib
import json
import math
import re
from pathlib import Path


def fingerprint(path):
    path = Path(path).resolve()
    if not path.is_file():
        return {'path': str(path), 'status': 'missing'}
    h = hashlib.sha256()
    with path.open('rb') as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b''):
            h.update(chunk)
    return {'path': str(path), 'bytes': path.stat().st_size, 'sha256': h.hexdigest()}


def provenance(folder, exe, args, env):
    """Only diagnostic settings, never a dump of the process environment."""
    exe = Path(exe).resolve()
    data = Path(args[0]).resolve() if args and not str(args[0]).startswith('--') else exe.with_name(exe.stem + '_data')
    paths = [exe, exe.with_name(exe.stem + '_code.dll'), exe.with_suffix('.ini')]
    paths += sorted(data.glob('pipeline*'))
    settings = {k: v for k, v in env.items() if k.startswith(('NXGFX_', 'NXRS_', 'NXOS_'))
                and not re.search('TOKEN|SECRET|PASSWORD|CREDENTIAL|API_KEY', k, re.I)}
    report = {'schema': 'nxrs.run-provenance.v1', 'command': [str(exe), *map(str, args)],
              'data': str(data), 'settings': settings, 'files': [fingerprint(p) for p in paths]}
    (Path(folder) / 'provenance.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    return report


def pipeline_events(text):
    active = collections.defaultdict(collections.deque)
    complete, errors = [], []
    last_time = 0.0
    pattern = re.compile(r'pipeline (compute|graphics) (compiling|made|failed) on (.+?): ([0-9a-f ]+)$')
    for line_number, line in enumerate(text.splitlines(), 1):
        stamp = re.match(r'^\[\s*([\d.]+)\]', line)
        if stamp:
            last_time = max(last_time, float(stamp[1]))
        match = pattern.search(line)
        if not match or not stamp:
            continue
        kind, phase, thread, keys = match.groups()
        ident = (kind, thread, keys)
        at = float(stamp[1])
        if phase == 'compiling':
            active[ident].append({'kind': kind, 'thread': thread, 'shader_keys': keys.split(),
                                  'start_seconds': at, 'start_line': line_number})
        elif active[ident]:
            event = active[ident].popleft()
            event.update(end_seconds=at, elapsed_ms=round((at-event['start_seconds'])*1000, 3),
                         end_line=line_number, outcome=phase)
            (complete if phase == 'made' else errors).append(event)
    unfinished = [dict(event, observed_pending_ms=round((last_time-event['start_seconds'])*1000, 3))
                  for events in active.values() for event in events]
    return {'logging_observed': bool(complete or errors or unfinished),
            'completed': len(complete), 'failed': errors[:32], 'unfinished': unfinished[:32],
            'slowest': sorted(complete, key=lambda e: e['elapsed_ms'], reverse=True)[:20],
            'scope': 'Logged whole-pipeline driver calls; unfinished is not proof of a hang. '
                     'Graphics-library linking and unlogged calls are not covered.'}


def shader_evidence(run, keys):
    try:
        data = Path(json.loads((run / 'provenance.json').read_text())['data'])
    except (OSError, ValueError, KeyError):
        return []
    evidence = []
    for key in list(dict.fromkeys(keys))[:16]:
        if not re.fullmatch('[0-9a-f]{16}', key):
            continue
        spv = data / 'shaders/spv' / (key + '.spv')
        if not spv.exists():
            spv = data / 'shaders' / (key + '.spv')
        item = dict(key=key, spirv=fingerprint(spv))
        try:
            reflection = json.loads(spv.with_suffix('.json').read_text())
            item['reflection'] = {k: reflection[k] for k in ('stage', 'execution_model', 'capabilities', 'bindings', 'mesh_stage')
                                  if k in reflection}
        except (OSError, ValueError):
            item['reflection_status'] = 'missing/unreadable'
        evidence.append(item)
    return evidence


def generate(run, text, window=None):
    import observability
    run = Path(run)
    fields = ('frame_ms', 'gpu_ms', 'replay_ms', 'acquire_wait_ms', 'tex_upload_ms', 'pipelines_made',
              'prep_ms', 'pool_sync_ms', 'descriptor_ms', 'pipeline_ms')
    values = {key: [] for key in fields}
    skipped = draws = frames = 0
    for path in sorted(run.glob('frames-*.csv')):
        for row in observability._csv_rows(path):
            t = observability._float(row, 'guest_s', 'seconds')
            if not observability._in_window(t, window):
                continue
            frames += 1
            skipped += observability._int(row, 'draws_skipped')
            draws += observability._int(row, 'draws')
            for key in fields:
                value = observability._float(row, key)
                if value is not None and math.isfinite(value):
                    values[key].append(value)
    costs = {key: {'samples': len(v), 'mean': round(sum(v)/len(v), 3) if v else None,
                   'p95': observability._percentile(v, .95), 'max': max(v) if v else None}
             for key, v in values.items()}
    waits = [{'line': n, 'text': line} for n, line in enumerate(text.splitlines(), 1) if 'wait-diag' in line]
    pipelines = pipeline_events(text)
    recovered = {'counter_observed': False, 'lifetime_pool_events': 0, 'window_pool_events': 0}
    for path in run.glob('metrics-*.csv'):
        for row in observability._csv_rows(path):
            recovered['counter_observed'] |= 'pool_cold_writes_recovered' in row
            count = observability._int(row, 'pool_cold_writes_recovered')
            recovered['lifetime_pool_events'] += count
            if observability._in_window(observability._float(row, 'seconds'), window):
                recovered['window_pool_events'] += count
    fault_lines = [{'line': n, 'text': line} for n, line in enumerate(text.splitlines(), 1)
                   if re.search(r'fault address:|the GPU started|inside BUFFER|earlier: BUFFER|recorded device range|nearest bound range', line)]
    fault_keys = [key for event in fault_lines for key in re.findall(r'\b[0-5]:([0-9a-f]{16})\b', event['text'])]
    pipeline_keys = [key for event in pipelines['unfinished'] for key in event['shader_keys']]
    findings = []
    if pipelines['unfinished']:
        findings.append('Driver pipeline calls have no logged return; inspect shader keys and wait ledger before blaming fences/codecs.')
    if skipped:
        findings.append('Skipped draws limit rendering/stability coverage. Retain blacklists and inspect their causes.')
    replay, gpu = costs['replay_ms']['mean'], costs['gpu_ms']['mean']
    if replay is not None and gpu is not None and replay > max(5, gpu * 2):
        findings.append('CPU replay time exceeds GPU time; inspect pool sync, uploads and pipeline creation first.')
    report = {'schema': 'nxrs.triage.v1', 'window_seconds': list(window) if window else None,
              'frame_costs': costs, 'frames': frames, 'draws': draws, 'draws_skipped': skipped,
              'pool_write_recovery': recovered,
              'timing_note': 'Counters can overlap or cover asynchronous work; do not sum them into a frame budget.',
              'pipelines': pipelines, 'wait_ledger': waits[-48:], 'findings': findings,
              'gpu_fault_context': fault_lines[-64:],
              'shader_evidence': shader_evidence(run, fault_keys + pipeline_keys),
              'diagnostic_requests': [], 'screenshots': sorted(p.name for p in run.glob('*.png'))}
    for path in sorted(run.glob('diagnostics-*-requests.json')):
        try:
            report['diagnostic_requests'].append({'path': path.name, 'requests': json.loads(path.read_text())})
        except (OSError, ValueError):
            report['diagnostic_requests'].append({'path': path.name, 'error': 'unreadable/incomplete'})
    (run / 'triage.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    return report


if __name__ == '__main__':
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('run', type=Path)
    ap.add_argument('--window')
    a = ap.parse_args()
    window = tuple(map(float, a.window.split(','))) if a.window else None
    report = generate(a.run, (a.run / 'run.log').read_text(errors='replace'), window)
    print(json.dumps(report, indent=2))
