"""A play.py run folder -> report.md / report.json, plus contact sheets of its screenshots (sheet-N.png).

The report states facts from the run (outcome, frames, memory, the engine's own red lines, stalls and fault
lines), the anomaly timeline (play_events: what the engine flagged, when, near which screenshot), the route checks
(play_route: whether the declared shots look like a known-good run's, else OFF-ROUTE and the FPS is not valid) and
the fault captures (NXGFX_FAULT_SHOTS). The sheets put every screenshot, labelled, into one image to look at.
"""
import json
import re
import tomllib
from pathlib import Path

import play_events
import play_route
import play_sheet

EXITS = {0: 'normal exit', 70: 'guest fault (exit 70)', 71: 'GPU device lost (exit 71)',
         72: 'engine safety stop (exit 72)', 0xC0000005: 'access violation', 0xC00000FD: 'stack overflow',
         0xC0000409: 'stack buffer overrun / fast fail', 0x40010004: 'killed (debugger/taskkill)', 1: 'killed or error'}
PACING = re.compile(r'^\[\s*([\d.]+)\] \[vk\] pacing: ([\d.]+) FPS \| p95 ([\d.]+) p99 ([\d.]+) max ([\d.]+) ms')
STAMP = re.compile(r'^\[\s*(?:front\s+)?([\d.]+)\]\s*')
FAULT = re.compile(r'DEVICE_LOST|device lost|\bstall|\bexception\b|panicked|\bfault\b|\babort(ed|ing)?\b', re.I)
NOT_FAULT = ('if the GPU stops responding', 'init takeover:')
COUNTER = re.compile(r'skip|drop|lost|fail|unimpl|blacklist|stale|miss', re.I)
SHEET_COLUMNS, SHEET_ROWS, CELL_W = 4, 3, 480


def metrics(folder, window):
    """Per-second rows of every metrics-*.csv (successor processes write their own)."""
    rows, names = [], []
    for path in sorted(folder.glob('metrics-*.csv')):
        for line in path.read_text(encoding='utf-8', errors='replace').splitlines():
            if line.startswith('#'):
                names = line[1:].split(',')
                continue
            values = line.split(',')
            if names and len(values) == len(names):
                try:
                    rows.append(dict(zip(names, map(float, values))))
                except ValueError:
                    pass
    out = dict(seconds_recorded=len(rows))
    if not rows:
        return out
    lo, hi = window or (0, 1e9)
    play = [r for r in rows if lo <= r['seconds'] <= hi] or rows
    fps = sorted(r['fps'] for r in play)
    out.update(window=[lo, hi], fps_mean=round(sum(fps) / len(fps), 1), fps_min=fps[0],
               fps_p5=fps[len(fps) // 20], memory_mb_start=play[0]['private_mb'],
               memory_mb_end=play[-1]['private_mb'], memory_mb_peak=max(r['private_mb'] for r in rows),
               frames=int(max(r['frames'] for r in rows)))
    totals = {}
    for r in rows:
        for k, v in r.items():
            if k not in ('seconds', 'frames', 'fps', 'cpu_percent', 'private_mb', 'xmon') and v:
                totals[k] = totals.get(k, 0) + v
    out['counters_of_note'] = {k: int(v) for k, v in sorted(totals.items()) if COUNTER.search(k)}
    return out


def log_facts(folder, failed):
    path = folder / 'run.log'
    lines = path.read_text(encoding='utf-8', errors='replace').splitlines() if path.exists() else []
    red, faults, pacing = {}, [], []
    for line in lines:
        if '[RED]' in line:
            key = re.sub(r'0x[0-9a-fA-F]+|\b\d+\b', '#', STAMP.sub('', line))
            entry = red.setdefault(key, [0, line.strip()])
            entry[0] += 1
        elif FAULT.search(line) and not any(s in line for s in NOT_FAULT) and len(faults) < 40:
            faults.append(line.strip()[:300])
        m = PACING.match(line)
        if m:
            pacing.append([float(x) for x in m.groups()])
    out = dict(log_lines=len(lines), red_kinds=len(red), red_lines=sum(c for c, _ in red.values()),
               red=[dict(count=c, first=first[:300]) for c, first in sorted(red.values(), key=lambda e: -e[0])[:15]],
               fault_lines=faults)
    if pacing:
        worst = max(pacing, key=lambda p: p[4])
        out['pacing_worst'] = dict(at=worst[0], fps=worst[1], p99_ms=worst[3], max_ms=worst[4])
    if failed:
        out['log_tail'] = [line[:300] for line in lines[-40:]]
    return out


def screenshots(folder):
    shots = sorted(p for p in folder.glob('[0-9][0-9]-*.png'))
    times = {}
    steps = folder / 'steps.jsonl'
    if steps.exists():
        for line in steps.read_text(encoding='utf-8').splitlines():
            e = json.loads(line)
            m = re.search(r'(?:fullres|shot) (\S+\.png)', e.get('ctl', ''))
            if m:
                times[Path(m.group(1)).name] = e['t']
    return [(p, times.get(p.name)) for p in shots]


def sheets(folder, shots):
    """Contact sheets (play_sheet, pure Python: a Windows PowerShell stuck while starting held the old
    System.Drawing sheets, and with them every report, 2026-11-26)."""
    for old in [*folder.glob('sheet-*.png'), folder / 'sheet.ps1', folder / 'sheet.json']:
        old.unlink(missing_ok=True)
    per = SHEET_COLUMNS * SHEET_ROWS
    made = []
    for n in range(0, len(shots), per):
        out = folder / ('sheet-%d.png' % (n // per + 1))
        items = [(p, '%s  @%ss' % (p.stem, '?' if t is None else round(t))) for p, t in shots[n:n + per]]
        play_sheet.sheet(out, items, min(SHEET_COLUMNS, len(items)), CELL_W)
        made.append(out.name)
    return made


def scenario_checks(meta):
    """The scenario's declared route checks: scenarios.toml's current ones (they go with the current references in
    checkpoints.json, so a regenerated report of an old run uses today's check kinds), else run.json's copy."""
    path = Path(__file__).resolve().parent / 'scenarios.toml'
    try:
        with path.open('rb') as f:
            current = tomllib.load(f).get(meta['scenario'], {}).get('checks')
    except (OSError, tomllib.TOMLDecodeError):
        current = None
    return current if current else meta.get('checks') or {}


def anomalies(folder, window, shots):
    """The anomaly timeline: per-kind counts, the events near each screenshot, the fault captures."""
    events, engine_shots = play_events.from_log(folder / 'run.log')
    rates = play_events.steady_rates(events)
    near = []
    for p, t in shots:
        et, present = engine_shots.get(p.name, (t, None))
        if et is None:
            continue
        near.append(dict(shot=p.name, t=et, present=present,
                         events=[dict(kind=k, count=n, first=x) for k, n, x in play_events.near(events, et, rates)]))
    captures = sorted(x.relative_to(folder).as_posix() for x in (folder / 'faults').glob('*.png')) \
        if (folder / 'faults').is_dir() else []
    return dict(kinds=play_events.summary(events, window), near_shots=near, captures=captures,
                capture_lines=[e[4] for e in events if e[1] == 'capture'][:40])


def anomaly_md(an, window):
    md = ['', '## Anomalies (engine log; window %s s)' % (window,)]
    if not an['kinds']:
        md.append('- none')
    else:
        md += ['', '| kind | run | window | first s | last s | steady / peak per period | example (peak) |',
               '|---|---|---|---|---|---|---|']
        md += ['| %s | %d | %d | %.1f | %.1f | %s | `%s` |' % (
            k, v['total'], v['in_window'], v['first'], v['last'],
            '%d / %d at %.0f s' % (v['steady'], v['peak'], v['peak_at']) if 'steady' in v else '-',
            v['example'][:160].replace('|', '/')) for k, v in an['kinds'].items()]
    md += ['', '## Near each screenshot (anomalies in the %g s before it; periodic counts only above twice their '
           'rate over the 12 periods before)' % play_events.NEAR_BEFORE]
    quiet = []
    for s in an['near_shots']:
        if not s['events']:
            quiet.append('`%s`' % s['shot'])
            continue
        md.append('- `%s` @%.1f s%s: ' % (s['shot'], s['t'], ' (present %d)' % s['present'] if s['present'] else '')
                  + ', '.join('%s %d' % (e['kind'], e['count']) for e in s['events']))
        md += ['    - `%s`' % e['first'][:220] for e in s['events']]
    md.append('- nothing near: ' + (', '.join(quiet) if quiet else '-'))
    if an['captures'] or an['capture_lines']:
        md += ['', '## Fault captures (NXGFX_FAULT_SHOTS)'] + ['- `%s`' % c for c in an['capture_lines']] + \
              ['- files: ' + ', '.join('`%s`' % c for c in an['captures'])]
    return md


def route_md(route):
    if not route:
        return []
    md = ['', '## Route checks (against a known-good run: whole-frame colour-histogram distance, off route past %g; '
          'region checks: share of each region matching, off route below its min)' % play_route.OFF_ROUTE_HIST, '',
          '| shot | should show | distance | verdict | reference |', '|---|---|---|---|---|']
    for r in route:
        verdict = 'n/a (%s)' % r['why'] if r['off'] is None else '**OFF ROUTE**' if r['off'] else 'on route'
        md.append('| `%s` | %s | %s | %s | %s |' % (r['shot'], r['about'], play_route.describe(r), verdict,
                                                   r.get('ref', '-')))
    return md


def write(folder):
    folder = Path(folder)
    meta = json.loads((folder / 'run.json').read_text(encoding='utf-8'))
    result = json.loads((folder / 'result.json').read_text(encoding='utf-8')) \
        if (folder / 'result.json').exists() else dict(outcome='unknown (no result.json)')
    code = result.get('exit_code')
    failed = result['outcome'] != 'completed' or code not in (0, None)
    shots = screenshots(folder)
    route = play_route.check(folder, meta['scenario'], scenario_checks(meta), [p for p, _ in shots])
    off = [r['shot'] for r in route if r['off']]
    report = dict(scenario=meta['scenario'], exe=meta['exe'], **result,
                  exit_meaning=EXITS.get(code, 'code %#x' % code) if code is not None else None,
                  metrics=metrics(folder, meta.get('window')), log=log_facts(folder, failed),
                  route=route, off_route=off, anomalies=anomalies(folder, meta.get('window'), shots),
                  screenshots=[p.name for p, _ in shots], sheets=sheets(folder, shots))
    (folder / 'report.json').write_text(json.dumps(report, indent=1), encoding='utf-8')
    m, lg = report['metrics'], report['log']
    md = ['# %s: %s%s' % (meta['scenario'], report['outcome'], ' - OFF-ROUTE' if off else ''), '',
          '- exe: `%s`' % meta['exe'],
          '- ran %s s, steps %s/%s%s' % (result.get('seconds'), result.get('steps_done'), result.get('steps'),
                                         ', step error: %s' % result['step_error'] if result.get('step_error') else ''),
          '- exit: %s' % (report['exit_meaning'] or 'still running when stopped (killed)')]
    if off:
        md.append('- **OFF-ROUTE**: %s do not show what the scenario expects (see Route checks); the FPS below is '
                  'not the scenario\'s stretch and is not valid' % ', '.join('`%s`' % s for s in off))
    if m.get('fps_mean') is not None:
        md.append('- %sFPS in %s s: mean %s, 5%% low %s, min %s (target %s); %s frames' % (
            '**INVALID (off route)** ' if off else '', m['window'], m['fps_mean'], m['fps_p5'], m['fps_min'],
            meta.get('fps'), m['frames']))
        md.append('- memory over the window: %d -> %d MB (run peak %d)' % (m['memory_mb_start'], m['memory_mb_end'], m['memory_mb_peak']))
    else:
        md.append('- no metrics rows (the game never reached its first second of presenting?)')
    if lg.get('pacing_worst'):
        p = lg['pacing_worst']
        md.append('- worst pacing at %ss: %s FPS, p99 %s ms, max %s ms' % (p['at'], p['fps'], p['p99_ms'], p['max_ms']))
    if m.get('counters_of_note'):
        md.append('- counters: ' + ', '.join('%s %d' % kv for kv in m['counters_of_note'].items()))
    md += route_md(route)
    md += anomaly_md(report['anomalies'], meta.get('window'))
    md += ['', '## Red lines (%d lines, %d kinds)' % (lg['red_lines'], lg['red_kinds'])]
    md += ['- %dx `%s`' % (r['count'], r['first']) for r in lg['red']] or ['- none']
    if lg['fault_lines']:
        md += ['', '## Fault / stall lines'] + ['    ' + line for line in lg['fault_lines'][:20]]
    if lg.get('log_tail'):
        md += ['', '## Log tail'] + ['    ' + line for line in lg['log_tail']]
    md += ['', '## Screenshots'] + ['- sheet: `%s`' % s for s in report['sheets']] + \
          ['- `%s`' % s for s in report['screenshots']]
    (folder / 'report.md').write_text('\n'.join(md) + '\n', encoding='utf-8')
    return '\n'.join(md)
