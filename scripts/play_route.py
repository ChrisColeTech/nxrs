"""Route checkpoints: is a run where its scenario says it should be?

A scenario names the shots that show a known place (`checks = { walk1 = "Link in his house", ... }` in
scenarios.toml). `py scripts/play.py checkpoint <run>` stores the signatures of a known-good run's shots for those
names in scripts/checkpoints.json (read the run's sheets first); every later run's report compares its shots of the
same names with them. A shot far from its reference means the run went somewhere else (a menu, a title, another
room): the run is OFF-ROUTE and its FPS numbers measure the wrong thing.

A signature is cheap and tolerant of camera motion: a 4x4x4 colour histogram of a sparse sample of the picture and a
24x14 grey thumbnail. PNGs are decoded here (no PIL on this machine); the engine writes unfiltered rows, other
encoders' filters are undone row by row.
"""
import json
import struct
import zlib
from pathlib import Path

CHECKPOINTS = Path(__file__).resolve().parent / 'checkpoints.json'
THUMB_W, THUMB_H = 24, 14
HIST_BINS = 4  # per channel
SAMPLE = 96  # histogram sample grid: SAMPLE x SAMPLE*9/16 points
# a shot whose histogram distance (0 same .. 1 disjoint) to its reference passes this is off route; the thumbnail
# distance (mean grey difference, 0..255) is reported beside it. Calibrated on OoT (2026-11-26): the same in-house
# shots of six on-route runs 0.01-0.20; the options menu of the desynced 20261126-054320-v5-oot 0.42-0.70
OFF_ROUTE_HIST = 0.3


def decode(path):
    """(width, height, channels, pixel rows as bytes) of an 8-bit RGB/RGBA PNG; None when it is not one."""
    data = Path(path).read_bytes()
    if data[:8] != b'\x89PNG\r\n\x1a\n':
        return None
    pos, idat, w = 8, [], None
    while pos + 8 <= len(data):
        n, kind = struct.unpack('>I4s', data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + n]
        if kind == b'IHDR':
            w, h, depth, ctype, _, _, interlace = struct.unpack('>IIBBBBB', body)
            if depth != 8 or ctype not in (2, 6) or interlace:
                return None
            ch = 4 if ctype == 6 else 3
        elif kind == b'IDAT':
            idat.append(body)
        elif kind == b'IEND':
            break
        pos += 12 + n
    if w is None:
        return None
    raw = zlib.decompress(b''.join(idat))
    stride = w * ch
    rows, prev = [], bytes(stride)
    for y in range(h):
        f = raw[y * (stride + 1)]
        line = raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)]
        if f:
            line = unfilter(f, bytearray(line), prev, ch)
        rows.append(line)
        prev = line
    return w, h, ch, rows


def unfilter(f, cur, prev, bpp):
    for i in range(len(cur)):
        a = cur[i - bpp] if i >= bpp else 0
        b = prev[i]
        if f == 1:
            cur[i] = (cur[i] + a) & 255
        elif f == 2:
            cur[i] = (cur[i] + b) & 255
        elif f == 3:
            cur[i] = (cur[i] + ((a + b) >> 1)) & 255
        elif f == 4:
            c = prev[i - bpp] if i >= bpp else 0
            p = a + b - c
            pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
            cur[i] = (cur[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
    return bytes(cur)


def signature(path):
    """{'hist': [64 fractions], 'thumb': [24*14 greys]} of a screenshot; None when it cannot be read."""
    try:
        img = decode(path)
    except (OSError, zlib.error, struct.error, IndexError):
        return None
    if not img:
        return None
    w, h, ch, rows = img
    sh = max(1, SAMPLE * 9 // 16)
    hist = [0] * HIST_BINS ** 3
    q = 256 // HIST_BINS
    for j in range(sh):
        row = rows[(2 * j + 1) * h // (2 * sh)]
        for i in range(SAMPLE):
            o = ((2 * i + 1) * w // (2 * SAMPLE)) * ch
            hist[(row[o] // q * HIST_BINS + row[o + 1] // q) * HIST_BINS + row[o + 2] // q] += 1
    total = SAMPLE * sh
    thumb = []
    for j in range(THUMB_H):
        for i in range(THUMB_W):
            s = 0
            for dy in (1, 3):
                row = rows[(4 * j + dy) * h // (4 * THUMB_H)]
                for dx in (1, 3):
                    o = ((4 * i + dx) * w // (4 * THUMB_W)) * ch
                    s += row[o] * 3 + row[o + 1] * 6 + row[o + 2]
            thumb.append(round(s / 40))
    return dict(hist=[round(c / total, 4) for c in hist], thumb=thumb)


def distance(a, b):
    """(histogram distance 0..1, thumbnail mean grey difference 0..255)."""
    hd = sum(abs(x - y) for x, y in zip(a['hist'], b['hist'])) / 2
    td = sum(abs(x - y) for x, y in zip(a['thumb'], b['thumb'])) / len(a['thumb'])
    return round(hd, 3), round(td, 1)


def shot_name(png):
    """'15-h2.png' -> 'h2' (the scenario's shot name as play.py wrote it)."""
    return Path(png).stem.split('-', 1)[-1]


def load():
    if CHECKPOINTS.exists():
        return json.loads(CHECKPOINTS.read_text(encoding='utf-8'))
    return {}


def record(folder, scenario, checks):
    """Store `folder`'s shots named in `checks` as the scenario's references; returns the names stored."""
    folder = Path(folder)
    refs = load()
    mine = refs.setdefault(scenario, {})
    stored = []
    for png in sorted(folder.glob('[0-9][0-9]-*.png')):
        name = shot_name(png)
        if name in checks:
            sig = signature(png)
            if sig:
                mine[name] = dict(about=checks[name], run=folder.name, shot=png.name, **sig)
                stored.append(name)
    # one line per reference shot: readable diffs when a reference is replaced
    text = ',\n'.join('%s: {\n%s\n}' % (json.dumps(sc), ',\n'.join(
        ' %s: %s' % (json.dumps(n), json.dumps(v, separators=(',', ':'))) for n, v in sorted(shots.items())))
        for sc, shots in sorted(refs.items()))
    CHECKPOINTS.write_text('{\n' + text + '\n}\n', encoding='utf-8')
    return stored


def check(folder, scenario, checks, shots):
    """Each declared shot against its reference: [{shot, about, hist, thumb, off}], and whether the run is off route.

    `shots` are the run's screenshot paths. A declared shot with no reference yet is listed with off=None."""
    refs = load().get(scenario, {})
    out = []
    for png in shots:
        name = shot_name(png)
        if name not in checks:
            continue
        ref = refs.get(name)
        sig = signature(png) if ref else None
        if not ref or not sig:
            out.append(dict(shot=png.name, about=checks[name], off=None,
                            why='no reference yet' if not ref else 'unreadable'))
            continue
        hd, td = distance(sig, ref)
        out.append(dict(shot=png.name, about=checks[name], hist=hd, thumb=td, off=hd > OFF_ROUTE_HIST,
                        ref='%s/%s' % (ref['run'], ref['shot'])))
    return out
