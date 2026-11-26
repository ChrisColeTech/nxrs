"""The engine log's anomalies as a timeline: what went wrong, when (engine seconds, frame), and near which screenshot.

Every event is (t, kind, count, frame, text). Kinds:
    stand-in     a texture slot read as a zero stand-in (unbound, released or never made)
    no-target    a draw skipped for having no render target (the red line, and the engine's context lines)
    skip-*       draws skipped in a period (no shader / no pipeline / no target) and dropped work, from the
                 periodic replay totals (one event per period, its count the period's number)
    pipeline     a pipeline not compiled (crashed the compiler before) or failing
    gate         parallel translation: segments that gave up at the gate or were translated again, per period
    held-back    CPU writes kept out of GPU-written textures, or uploaded again over them
    held-back-frame  KB of CPU writes held back a frame (periodic pools line)
    query        GPU queries refused or exhausted
    fault        device loss, guest faults, stalls, panics
    capture      a fault-triggered frame capture (NXGFX_FAULT_SHOTS)
    red          any other red line
"""
import re
from pathlib import Path

STAMP = re.compile(r'^\[\s*(?:[a-z]+\s+)?([\d.]+)\]\s*')
FRAME = re.compile(r'\b(?:at |up to )?frame (\d+)')
PRESENT = re.compile(r'\b(?:at |after )present (\d+)')
SHOT = re.compile(r'\[vk\] screenshot: present (\d+) \(\d+x\d+\) written to (.+?\.png)')
FAULT = re.compile(r'DEVICE_LOST|device lost|\bstall|\bexception\b|panicked|\bfault\b|\babort(ed|ing)?\b', re.I)
NOT_FAULT = ('if the GPU stops responding', 'init takeover:', 'fault capture', 'NXGFX_FAULT')
REPLAY = re.compile(r'\[vk\] replay up to frame (\d+), .*?no shader (\d+), no pipeline (\d+), no target (\d+) in all\).*?'
                    r'dropped work (\d+)')
GATE = re.compile(r'\[vk\] translate \(parallel\).*?(\d+) segments translated again; the gate asked (\d+) times, '
                  r'(\d+) segments gave up')
HELD = re.compile(r'CPU writes held back a frame: ([\d.]+) MB under in-flight write-backs \(([\d.]+) syncs\), '
                  r'([\d.]+) MB past the upload budget')
TARGETS = {0: '1D', 1: '2D', 2: '3D', 3: '1D array', 4: '2D array', 5: '2D multisample', 6: '2D multisample array',
           7: 'rectangle', 8: 'cube', 9: 'cube array', 10: 'buffer'}
SKIPS = ('skip-no-shader', 'skip-no-pipeline', 'skip-no-target', 'dropped-work')
PERIODIC = set(SKIPS) | {'gate', 'held-back-frame'}
KIND_ORDER = ['fault', 'capture', 'stand-in', 'no-target', 'skip-no-target', 'skip-no-shader', 'skip-no-pipeline',
              'dropped-work', 'pipeline', 'gate', 'held-back', 'held-back-frame', 'query', 'red']
NEAR_BEFORE, NEAR_AFTER = 5.0, 0.5  # seconds around a screenshot that count as "near" it


def named_target(text):
    """'texture ID 7 (target 4) ...' -> '... (target 4 = 2D array) ...' (NVN texture targets)."""
    return re.sub(r'\(target (\d+)', lambda m: '(target %s = %s' % (m.group(1), TARGETS.get(int(m.group(1)), '?')),
                  text, count=1)


def classify(body):
    """The kind of one log line (without its stamp), or None for an ordinary line."""
    red = body.startswith('[RED] ')
    if re.match(r'\[vk\] fault capture \d', body):  # first: it quotes the anomaly it captured
        return 'capture'
    if body.startswith('[vk] fault captures to'):
        return None
    if 'zero stand-in is bound' in body or 'storage stand-in' in body:
        return 'stand-in'
    if 'no render target bound' in body or body.startswith('[vk] no render target:'):
        return 'no-target'
    if 'crashed the driver' in body or 'pipeline' in body and red:
        return 'pipeline'
    if 'CPU write kept out of GPU-written' in body or 'written by the CPU: uploaded again' in body:
        return 'held-back'
    if 'quer' in body and red:
        return 'query'
    if red:
        return 'red'
    if FAULT.search(body) and not any(s in body for s in NOT_FAULT):
        return 'fault'
    return None


def parse(lines):
    """(events, shots): events sorted by time; shots {png name: (t, present)} from the engine's screenshot lines."""
    events, shots = [], {}
    last_replay = None
    for line in lines:
        m = STAMP.match(line)
        if not m:
            continue
        t, body = float(m.group(1)), line[m.end():].rstrip()
        s = SHOT.search(body)
        if s:
            shots[Path(s.group(2)).name] = (t, int(s.group(1)))
            continue
        r = REPLAY.search(body)
        if r:
            now = [int(x) for x in r.groups()]
            if last_replay:
                # quiet periods are kept (count 0): they are part of a kind's steady rate
                for n, kind in zip([a - b for a, b in zip(now[1:], last_replay[1:])], SKIPS):
                    events.append((t, kind, max(n, 0), now[0], 'frames %d-%d: %d %s' % (
                        last_replay[0], now[0], n, kind.replace('skip-', 'draws skipped, ').replace('-', ' '))))
            last_replay = now
            continue
        gm = GATE.search(body)
        if gm:
            again, _, gave = map(int, gm.groups())
            events.append((t, 'gate', again + gave, None,
                           'last 600 frames: %d segments gave up, %d translated again' % (gave, again)))
            continue
        hm = HELD.search(body)
        if hm:
            under, _, past = map(float, hm.groups())
            events.append((t, 'held-back-frame', round((under + past) * 1024), None,
                           'CPU writes held back a frame (KB): %.2f MB under write-backs, %.2f MB past the '
                           'upload budget' % (under, past)))
            continue
        kind = classify(body)
        if kind:
            f = FRAME.search(body) or PRESENT.search(body)
            # a capture's follow-up (whether its flash was gone at the next present) is not another capture
            n = 0 if kind == 'capture' and 'at the next present' in body else 1
            events.append((t, kind, n, int(f.group(1)) if f else None, named_target(body)[:400]))
    events.sort(key=lambda e: e[0])
    return events, shots


def steady_rates(events, span=12):
    """{(t, periodic kind): the median count of that kind's previous `span` periods, quiet ones included}: a draw
    skipped every frame, or the writes a scene always holds back, is background, not news near a shot."""
    seen, out = {}, {}
    for t, kind, n, _, _ in events:
        if kind in PERIODIC:
            prior = sorted(seen.setdefault(kind, [])[-span:])
            out[(t, kind)] = prior[len(prior) // 2] if prior else 0
            seen[kind].append(n)
    return out


def is_spike(e, rates):
    """A periodic event above twice its local steady rate, or any non-periodic event; never a quiet period."""
    return e[2] > 0 and (e[1] not in PERIODIC or e[2] > 2 * rates.get((e[0], e[1]), 0) + 1)


def summary(events, window):
    """{kind: dict(total, in_window, first, last, example[, steady, peak, peak_at])} in KIND_ORDER; periodic kinds'
    `steady` is the run's median per period, quiet periods included."""
    lo, hi = window or (0, 1e9)
    per = {}
    for _, kind, n, _, _ in events:
        if kind in PERIODIC:
            per.setdefault(kind, []).append(n)
    rates = {k: sorted(v)[len(v) // 2] for k, v in per.items()}
    out = {}
    for t, kind, n, _, text in events:
        if n <= 0 and kind in PERIODIC:
            continue
        k = out.setdefault(kind, dict(total=0, in_window=0, first=t, last=t, example=text))
        k['total'] += n
        k['in_window'] += n if lo <= t <= hi else 0
        k['last'] = t
        if kind in PERIODIC and n > k.get('peak', -1):
            k.update(steady=rates[kind], peak=n, peak_at=t, example=text)
    return {k: out[k] for k in KIND_ORDER if k in out}


def near(events, t, rates=None):
    """The events in [t - NEAR_BEFORE, t + NEAR_AFTER] (periodic ones only above their local steady rate),
    grouped: [(kind, count, first text)]."""
    rates = steady_rates(events) if rates is None else rates
    groups = {}
    for e in events:
        et, kind, n, _, text = e
        if t - NEAR_BEFORE <= et <= t + NEAR_AFTER and is_spike(e, rates):
            g = groups.setdefault(kind, [0, '%.2f s: %s' % (et, text)])
            g[0] += n
    return [(k, groups[k][0], groups[k][1]) for k in KIND_ORDER if k in groups]


def from_log(path):
    p = Path(path)
    if not p.exists():
        return [], {}
    return parse(p.read_text(encoding='utf-8', errors='replace').splitlines())
