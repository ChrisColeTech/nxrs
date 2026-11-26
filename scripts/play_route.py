"""Route checkpoints: is a run where its scenario says it should be?

A scenario names the shots that show a known place (`checks = { walk1 = "Link in his house", ... }` in
scenarios.toml). `py scripts/play.py checkpoint <run>` stores the signatures of a known-good run's shots for those
names in scripts/checkpoints.json (read the run's sheets first); every later run's report compares its shots of the
same names with them. A shot far from its reference means the run went somewhere else (a menu, a title, another
room): the run is OFF-ROUTE and its FPS numbers measure the wrong thing.

A signature is cheap and tolerant of camera motion: a 4x4x4 colour histogram of a sparse sample of the picture and a
24x14 grey thumbnail. PNGs are decoded here (no PIL on this machine); the engine writes unfiltered rows, other
encoders' filters are undone row by row.

Whole-frame similarity suits a place the camera holds still (a house, a menu, a start grid). A race never lands at
the same spot of the track twice (game time drifts with FPS), so a check can instead name screen regions that must
match: `race = { about = "racing", regions = "kart-hud" }` (a named set in REGION_SETS) or inline
`regions = { coin = [x0, y0, x1, y1, min] }` (fractions of the frame; min = the share of the region's 12x6 sample
points that must match the reference, 0..1). A point matches when its mean RGB difference is at most CELL_MATCH, so
the background showing around or through a HUD element spoils only the points it covers. The shot is off route when
any region's share falls below its min.
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
REGION_W, REGION_H = 12, 6  # a region's sample points
CELL_MATCH = 40  # mean RGB difference (0..255) at which a region's sample point still matches
# Named region sets: name -> {region: (x0, y0, x1, y1, min share matching)}. Calibrated 2026-11-26 (DIAGFIX) on the
# MKW runs in build/play since 11-25 (race and t37-t46 shots of ~45 runs, the lead's included, against
# 20261126-055832-v5-mkw/05-race.png) and every boot / title shot and every OoT run as the negatives: the coin icon
# of the coin counter matches 0.90-1.00 in a race, at most 0.28 elsewhere; the place badge ("24th", the box kept
# inside the numerals' outline) 0.88-1.00 in a race, at most 0.32 elsewhere. One race shot taken during the start
# countdown (20261125-000839-mkw, no place badge yet: the run was behind) matches 0.06, off route, rightly.
# Both are up from the start grid on, so the grid's own check stays whole-frame.
REGION_SETS = {
    'kart-hud': {
        'coin': (0.034, 0.911, 0.056, 0.950, 0.6),
        'place': (0.845, 0.845, 0.935, 0.96, 0.6),
    },
}


def spec(value):
    """A declared check as {'about', 'regions'}: regions None for a whole-frame check, else {name: (box..., max)}."""
    if isinstance(value, str):
        return dict(about=value, regions=None)
    regions = value.get('regions')
    if isinstance(regions, str):
        if regions not in REGION_SETS:
            raise ValueError('unknown region set %r (play_route.REGION_SETS)' % regions)
        regions = REGION_SETS[regions]
    elif regions is not None:
        regions = {k: tuple(v) for k, v in regions.items()}
        for k, v in regions.items():
            if len(v) != 5 or not (0 <= v[0] < v[2] <= 1 and 0 <= v[1] < v[3] <= 1 and 0 < v[4] <= 1):
                raise ValueError('region %s must be [x0, y0, x1, y1, min share] with fractions' % k)
    return dict(about=value.get('about', ''), regions=regions)


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


def region_thumb(img, box):
    """REGION_W x REGION_H RGB points (flat r, g, b, ...) sampled inside box (x0, y0, x1, y1 as fractions)."""
    w, h, ch, rows = img
    x0, y0, x1, y1 = int(box[0] * w), int(box[1] * h), int(box[2] * w), int(box[3] * h)
    out = []
    for j in range(REGION_H):
        row = rows[min(h - 1, y0 + (2 * j + 1) * (y1 - y0) // (2 * REGION_H))]
        for i in range(REGION_W):
            o = min(w - 1, x0 + (2 * i + 1) * (x1 - x0) // (2 * REGION_W)) * ch
            out += row[o:o + 3]
    return out


def region_match(a, b):
    """The share (0..1) of two regions' sample points whose mean RGB difference is at most CELL_MATCH."""
    n = len(a) // 3
    ok = sum(1 for i in range(0, 3 * n, 3)
             if abs(a[i] - b[i]) + abs(a[i + 1] - b[i + 1]) + abs(a[i + 2] - b[i + 2]) <= 3 * CELL_MATCH)
    return round(ok / n, 2) if n else 0.0


def signature(path, regions=None):
    """{'hist': [64 fractions], 'thumb': [24*14 greys]} of a screenshot, plus 'regions': {name: greys} for the
    given regions; None when it cannot be read."""
    try:
        img = decode(path)
    except (OSError, zlib.error, struct.error, IndexError):
        return None
    if not img:
        return None
    sig = whole(img)
    if regions:
        sig['regions'] = {k: region_thumb(img, v[:4]) for k, v in regions.items()}
    return sig


def whole(img):
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
            s = spec(checks[name])
            sig = signature(png, s['regions'])
            if sig:
                mine[name] = dict(about=s['about'], run=folder.name, shot=png.name, **sig)
                stored.append(name)
    # one line per reference shot: readable diffs when a reference is replaced
    text = ',\n'.join('%s: {\n%s\n}' % (json.dumps(sc), ',\n'.join(
        ' %s: %s' % (json.dumps(n), json.dumps(v, separators=(',', ':'))) for n, v in sorted(shots.items())))
        for sc, shots in sorted(refs.items()))
    CHECKPOINTS.write_text('{\n' + text + '\n}\n', encoding='utf-8')
    return stored


def check(folder, scenario, checks, shots):
    """Each declared shot against its reference: [{shot, about, hist, thumb, off}], and whether the run is off route.

    `shots` are the run's screenshot paths. A declared shot with no reference yet is listed with off=None. A region
    check gives 'regions': {name: (share matching, min)} and no 'hist'."""
    refs = load().get(scenario, {})
    out = []
    for png in shots:
        name = shot_name(png)
        if name not in checks:
            continue
        s = spec(checks[name])
        ref = refs.get(name)
        regions = s['regions']
        if ref and regions and set(regions) - set(ref.get('regions', {})):
            out.append(dict(shot=png.name, about=s['about'], off=None,
                            why='reference lacks regions %s (record it again)' % ', '.join(
                                sorted(set(regions) - set(ref.get('regions', {}))))))
            continue
        sig = signature(png, regions) if ref else None
        if not ref or not sig:
            out.append(dict(shot=png.name, about=s['about'], off=None,
                            why='no reference yet' if not ref else 'unreadable'))
            continue
        r = dict(shot=png.name, about=s['about'], ref='%s/%s' % (ref['run'], ref['shot']))
        if regions:
            r['regions'] = {k: (region_match(sig['regions'][k], ref['regions'][k]), v[4])
                            for k, v in regions.items()}
            r['off'] = any(s < m for s, m in r['regions'].values())
        else:
            r['hist'], r['thumb'] = distance(sig, ref)
            r['off'] = r['hist'] > OFF_ROUTE_HIST
        out.append(r)
    return out


def describe(r):
    """The distance column of a route check row."""
    if 'regions' in r:
        return ', '.join('%s %s (min %s)' % (k, s, m) for k, (s, m) in r['regions'].items())
    return str(r.get('hist', '-'))
