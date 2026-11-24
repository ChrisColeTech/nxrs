"""A play.py run folder -> report.md / report.json, plus contact sheets of its screenshots (sheet-N.png).

The report states facts from the run (outcome, frames, memory, the engine's own red lines, stalls and fault
lines); it does not judge pictures. The sheets put every screenshot, labelled, into one image to look at.
"""
import json
import re
import subprocess
from pathlib import Path

EXITS = {0: 'normal exit', 70: 'guest fault (exit 70)', 71: 'GPU device lost (exit 71)',
         72: 'engine safety stop (exit 72)', 0xC0000005: 'access violation', 0xC00000FD: 'stack overflow',
         0xC0000409: 'stack buffer overrun / fast fail', 0x40010004: 'killed (debugger/taskkill)', 1: 'killed or error'}
PACING = re.compile(r'^\[\s*([\d.]+)\] \[vk\] pacing: ([\d.]+) FPS \| p95 ([\d.]+) p99 ([\d.]+) max ([\d.]+) ms')
STAMP = re.compile(r'^\[\s*(?:front\s+)?([\d.]+)\]\s*')
FAULT = re.compile(r'DEVICE_LOST|device lost|stall|exception|panicked|fault|abort', re.I)
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
               fps_p5=fps[len(fps) // 20], memory_mb_start=rows[0]['private_mb'],
               memory_mb_end=rows[-1]['private_mb'], memory_mb_peak=max(r['private_mb'] for r in rows),
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
        elif FAULT.search(line) and len(faults) < 40:
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


SHEET_PS = r'''
param([string]$Spec)
Add-Type -AssemblyName System.Drawing
$s = Get-Content -LiteralPath $Spec -Raw | ConvertFrom-Json
$cellH = [int]($s.cell_w * 9 / 16); $bar = 26
$rows = [math]::Ceiling($s.items.Count / $s.columns)
$bmp = New-Object System.Drawing.Bitmap ($s.columns * $s.cell_w), ($rows * ($cellH + $bar))
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.InterpolationMode = 'HighQualityBicubic'
$g.Clear([System.Drawing.Color]::FromArgb(24, 24, 24))
$font = New-Object System.Drawing.Font 'Segoe UI', 11
$i = 0
foreach ($it in $s.items) {
  $x = ($i % $s.columns) * $s.cell_w; $y = [math]::Floor($i / $s.columns) * ($cellH + $bar)
  $g.DrawString($it.label, $font, [System.Drawing.Brushes]::White, $x + 4, $y + 3)
  try {
    $img = [System.Drawing.Image]::FromFile($it.path)
    $scale = [math]::Min($s.cell_w / $img.Width, $cellH / $img.Height)
    $w = [int]($img.Width * $scale); $h = [int]($img.Height * $scale)
    $g.DrawImage($img, $x + [int](($s.cell_w - $w) / 2), $y + $bar + [int](($cellH - $h) / 2), $w, $h)
    $img.Dispose()
  } catch { $g.DrawString('unreadable', $font, [System.Drawing.Brushes]::OrangeRed, $x + 4, $y + $bar + 4) }
  $i++
}
$bmp.Save($s.out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
'''


def sheets(folder, shots):
    for old in folder.glob('sheet-*.png'):
        old.unlink()
    if not shots:
        return []
    script = folder / 'sheet.ps1'
    script.write_text(SHEET_PS, encoding='utf-8')
    per = SHEET_COLUMNS * SHEET_ROWS
    made = []
    for n in range(0, len(shots), per):
        out = folder / ('sheet-%d.png' % (n // per + 1))
        items = [dict(path=str(p), label='%s  @%ss' % (p.stem, '?' if t is None else round(t)))
                 for p, t in shots[n:n + per]]
        spec = folder / 'sheet.json'
        spec.write_text(json.dumps(dict(out=str(out), columns=SHEET_COLUMNS, cell_w=CELL_W, items=items)),
                        encoding='utf-8')
        subprocess.run(['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(script),
                        str(spec)], capture_output=True, timeout=120)
        if out.exists():
            made.append(out.name)
    for tmp in (script, folder / 'sheet.json'):
        tmp.unlink(missing_ok=True)
    return made


def write(folder):
    folder = Path(folder)
    meta = json.loads((folder / 'run.json').read_text(encoding='utf-8'))
    result = json.loads((folder / 'result.json').read_text(encoding='utf-8')) \
        if (folder / 'result.json').exists() else dict(outcome='unknown (no result.json)')
    code = result.get('exit_code')
    failed = result['outcome'] != 'completed' or code not in (0, None)
    shots = screenshots(folder)
    report = dict(scenario=meta['scenario'], exe=meta['exe'], **result,
                  exit_meaning=EXITS.get(code, 'code %#x' % code) if code is not None else None,
                  metrics=metrics(folder, meta.get('window')), log=log_facts(folder, failed),
                  screenshots=[p.name for p, _ in shots], sheets=sheets(folder, shots))
    (folder / 'report.json').write_text(json.dumps(report, indent=1), encoding='utf-8')
    m, lg = report['metrics'], report['log']
    md = ['# %s: %s' % (meta['scenario'], report['outcome']), '',
          '- exe: `%s`' % meta['exe'],
          '- ran %s s, steps %s/%s%s' % (result.get('seconds'), result.get('steps_done'), result.get('steps'),
                                         ', step error: %s' % result['step_error'] if result.get('step_error') else ''),
          '- exit: %s' % (report['exit_meaning'] or 'still running when stopped (killed)')]
    if m.get('fps_mean') is not None:
        md.append('- FPS in %s s: mean %s, 5%% low %s, min %s (target %s); %s frames' % (
            m['window'], m['fps_mean'], m['fps_p5'], m['fps_min'], meta.get('fps'), m['frames']))
        md.append('- memory: %d -> %d MB (peak %d)' % (m['memory_mb_start'], m['memory_mb_end'], m['memory_mb_peak']))
    else:
        md.append('- no metrics rows (the game never reached its first second of presenting?)')
    if lg.get('pacing_worst'):
        p = lg['pacing_worst']
        md.append('- worst pacing at %ss: %s FPS, p99 %s ms, max %s ms' % (p['at'], p['fps'], p['p99_ms'], p['max_ms']))
    if m.get('counters_of_note'):
        md.append('- counters: ' + ', '.join('%s %d' % kv for kv in m['counters_of_note'].items()))
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
