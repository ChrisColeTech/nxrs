"""Composes the README hero GIF (docs/images/readme/hero.gif) from the raw captures of the README-GIF
wave (docs/waves/README-GIF.md): the launcher (wincap FFV1, 1920x1080), the native game window and the
web build's Chrome screencast, all real. Every click is emphasised: a 1.8x magnifier grows smoothly
out of the control (ease in/out) and shrinks back afterwards, a lime ring marks the control's
rectangle inside it, and a pointer glides in and pulses at the exact point. Builds are sped up.

    python scripts/readme/hero_gif.py <capture dir> [--out GIF] [--fps 12] [--sheet PNG]

<capture dir> is build/README-GIF: raw/launcher.mkv(.times), raw/lf (the launcher at 960x540, every
frame), hf (the launcher at 1920x1080, the frame ranges the magnified beats use; extracted on demand
from raw/launcher.mkv), raw/gf + raw/game.mkv.times (the game), raw/web/frames + frames.times
(Chrome). Needs Pillow and ffmpeg (FFMPEG, C:\\ffmpeg\\bin\\ffmpeg.exe or on PATH).

Why a magnifier and not a full-frame camera zoom: a GIF frame in which every pixel moves costs about
220 KB here (LZW on the card art), so the 13 camera moves the clicks need came to 27 MB. A magnifier
changes only its own rectangle, which keeps the whole clip under 10 MB at 12 fps.
"""

import argparse
import bisect
import math
import os
import shutil
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

FF = os.environ.get("FFMPEG") or (
    r"C:\ffmpeg\bin\ffmpeg.exe" if Path(r"C:\ffmpeg\bin\ffmpeg.exe").exists() else "ffmpeg")
W, H = 960, 540            # output
SW, SH = 1920, 1080        # the space every coordinate below is in (the launcher capture)
LIME = (0xB5, 0xF3, 0x6B)  # the launcher's accent
B = 1795809000.0           # the captures' unix-time base (2026-11-27)
Z = 1.8                    # magnification
LW, LH = 480, 270          # the magnifier's full size in the output
M = 0.45                   # the magnifier's grow / shrink time

# Controls (1920x1080 launcher capture). The dialogs' buttons are where inp.py clicked them.
CARD = (1204, 186, 1467, 416)        # Mario Kart 8 Deluxe's card
CARD_PLAY = (1204, 430, 1412, 474)   # its Play button
GAME = (124, 8, 204, 52)             # menu bar: Game
BUILD = (128, 162, 524, 206)         # Game menu: Build
B_NATIVE = (1056, 697, 1176, 744)    # Build dialog: Native (clicked at 1113,696)
B_WEB = (1189, 697, 1309, 744)       # Build dialog: Web (1245,696)
P_NATIVE = (1032, 686, 1150, 730)    # Play dialog: Native (1090,708)
P_WEB = (1162, 686, 1282, 730)       # Play dialog: Web (1210,708)
MENU = (124, 8, 524, 206)            # Game and its Build item: one magnifier for both clicks


def mid(r):
    return ((r[0] + r[2]) / 2, (r[1] + r[3]) / 2)


# Beats: src, len (s), map [(beat s, source unix s - B)] (a hold where two keys share a source time),
# lens [(open, close, rect it centres on)], rings [(on, off, rect)], clicks [(s, rect)],
# q (source time stepped to q frames a second: cheap holds of moving video).
BEATS = [
    # the library; MK8D's card is magnified and selected
    dict(src="H", len=2.8, map=[(0, 974.6), (2.8, 974.7)],
         lens=[(0.8, 2.8, CARD)], rings=[(1.2, 2.4, CARD)], clicks=[(1.8, CARD)]),
    # Game -> Build (the menu opens inside the magnifier)
    dict(src="H", len=3.3, map=[(0, 974.8), (1.0, 974.9), (1.5, 975.6), (2.0, 976.2), (3.3, 976.25)],
         lens=[(0.0, 3.3, MENU)], rings=[(0.45, 1.1, GAME), (1.5, 3.0, BUILD)],
         clicks=[(0.9, GAME), (2.65, BUILD)]),
    # the Build dialog -> Native; the build starts
    dict(src="H", len=3.8, map=[(0, 976.35), (0.2, 977.6), (2.75, 977.8), (2.85, 978.4), (3.8, 979.3)],
         lens=[(1.2, 3.4, B_NATIVE)], rings=[(1.6, 3.0, B_NATIVE)], clicks=[(2.7, B_NATIVE)]),
    # the native build, sped up (~40x), to "Build complete"
    dict(src="L", len=2.8, map=[(0, 979.5), (2.2, 1074.4), (2.8, 1076.0)], q=6),
    # Game -> Build again
    dict(src="H", len=3.3, map=[(0, 1081.0), (1.05, 1081.1), (1.4, 1081.7), (2.0, 1082.5), (3.3, 1082.6)],
         lens=[(0.1, 3.3, MENU)], rings=[(0.55, 1.15, GAME), (1.5, 3.0, BUILD)],
         clicks=[(1.0, GAME), (2.65, BUILD)]),
    # the Build dialog (Native: up to date) -> Web; the web build starts
    dict(src="H", len=3.8,
         map=[(0, 1082.9), (0.2, 1083.9), (2.75, 1084.6), (2.85, 1085.0), (3.8, 1085.8)],
         lens=[(1.2, 3.4, B_WEB)], rings=[(1.6, 3.0, B_WEB)], clicks=[(2.7, B_WEB)]),
    # the web build, sped up (~110x), to "Web build complete"
    dict(src="L", len=2.8, map=[(0, 1085.8), (2.2, 1355.0), (2.8, 1357.0)], q=6),
    # the card's Play -> the Play dialog
    dict(src="H", len=2.7, map=[(0, 1363.2), (2.4, 1363.3), (2.4001, 1387.6), (2.7, 1387.6)],
         lens=[(0.2, 2.4, CARD_PLAY)], rings=[(0.65, 1.95, CARD_PLAY)], clicks=[(1.6, CARD_PLAY)]),
    # the Play dialog -> Native
    dict(src="H", len=3.4, map=[(0, 1387.6), (2.4, 1388.9), (2.5, 1389.2), (3.4, 1390.4)],
         lens=[(1.0, 3.2, P_NATIVE)], rings=[(1.45, 2.8, P_NATIVE)], clicks=[(2.35, P_NATIVE)]),
    # the game window: the intro swoosh to the title screen ("Press L+R to start")
    dict(src="G", len=3.2, map=[(0, 1394.0), (0.5, 1394.9), (1.0, 1397.0), (3.2, 1403.0)], q=2),
    # the card's Play again -> the Play dialog
    dict(src="H", len=2.7, map=[(0, 1425.6), (2.4, 1426.0), (2.4001, 1428.9), (2.7, 1429.0)],
         lens=[(0.2, 2.4, CARD_PLAY)], rings=[(0.65, 1.95, CARD_PLAY)], clicks=[(1.6, CARD_PLAY)]),
    # the Play dialog -> Web
    dict(src="H", len=3.3, map=[(0, 1429.0), (2.3, 1430.0), (2.4, 1430.4), (3.3, 1431.4)],
         lens=[(0.9, 3.1, P_WEB)], rings=[(1.35, 2.7, P_WEB)], clicks=[(2.25, P_WEB)]),
    # Chrome: the page's loading list, the logo, the title screen
    dict(src="C", len=4.5, map=[(0, 1431.3), (1.3, 1441.8), (2.0, 1446.0), (2.5, 1452.0), (4.5, 1458.0)],
         q=2),
]


def ease(x):  # cubic in-out
    x = min(1.0, max(0.0, x))
    return 4 * x ** 3 if x < 0.5 else 1 - (-2 * x + 2) ** 3 / 2


def lerp_keys(keys, t):
    if t <= keys[0][0]:
        return keys[0][1]
    for (t0, a), (t1, b) in zip(keys, keys[1:]):
        if t <= t1:
            return a + (b - a) * ((t - t0) / (t1 - t0) if t1 > t0 else 1.0)
    return keys[-1][1]


class Source:
    """Frames with times; file names are frame indices (wincap) or listed in the times file."""

    def __init__(self, frames, times, only_existing=False):
        pairs = []
        for line in Path(times).read_text().splitlines():
            if not line.strip():
                continue
            name, t = line.split("\t")
            f = Path(frames) / (f"{int(name):05d}.png" if name.isdigit() else name)
            if not only_existing or f.exists():
                pairs.append((float(t), f))
        pairs.sort()
        self.ts = [p[0] for p in pairs]
        self.files = [p[1] for p in pairs]
        self.cache = {}

    def at(self, t):
        f = self.files[max(0, bisect.bisect_right(self.ts, t) - 1)]
        if f not in self.cache:
            if len(self.cache) > 8:
                self.cache.clear()
            self.cache[f] = Image.open(f).convert("RGB")
        return self.cache[f]


def lens_geometry(rect, g):
    """Source region (1920 space) and its output rectangle at grow factor g (0 = in place, 1 = full)."""
    cx, cy = mid(rect)
    rw, rh = LW * 2 / Z, LH * 2 / Z
    x0 = min(max(cx - rw / 2, 0), SW - rw)
    y0 = min(max(cy - rh / 2, 0), SH - rh)
    nat = (x0 / 2, y0 / 2, rw / 2, rh / 2)
    m = 10
    fx = min(max((x0 + rw / 2) / 2 - LW / 2, m), W - m - LW)
    fy = min(max((y0 + rh / 2) / 2 - LH / 2, m), H - m - LH)
    full = (fx, fy, LW, LH)
    out = tuple(a + (b - a) * g for a, b in zip(nat, full))
    return (x0, y0, rw, rh), out


def cursor_poly(x, y, s):
    pts = [(0, 0), (0, 17), (4.5, 13), (7.5, 20), (10.5, 18.8), (7.6, 12.2), (13, 12.2)]
    return [(x + px * s, y + py * s) for px, py in pts]


def render(t, beat, im):
    """One output frame: the source fitted to 960x540, the magnifier, ring, pointer and pulse."""
    fr = im if im.size == (W, H) else im.resize((W, H), Image.LANCZOS)
    tf = lambda p: (p[0] / 2, p[1] / 2)  # source -> output, outside a magnifier
    S = 2  # overlays are drawn at 2x and scaled down (smooth edges)
    ov = Image.new("RGBA", (W * S, H * S), (0, 0, 0, 0))
    drew = False
    for on, off, rect in beat.get("lens", []):
        if not on <= t <= off:
            continue
        g = ease(min((t - on) / M, (off - t) / M))
        if g <= 0:
            continue
        drew = True
        (sx, sy, sw, sh), (ox, oy, ow, oh) = lens_geometry(rect, g)
        k = im.size[0] / SW
        mag = im.resize((round(ow), round(oh)), Image.LANCZOS,
                        box=(sx * k, sy * k, (sx + sw) * k, (sy + sh) * k))
        shadow = Image.new("RGBA", ov.size, (0, 0, 0, 0))
        ImageDraw.Draw(shadow).rounded_rectangle(
            ((ox - 4) * S, (oy + 2) * S, (ox + ow + 4) * S, (oy + oh + 8) * S), radius=16,
            fill=(0, 0, 0, int(150 * g)))
        fr = Image.alpha_composite(
            fr.convert("RGBA"), shadow.filter(ImageFilter.GaussianBlur(12)).resize((W, H), Image.LANCZOS))
        fr = fr.convert("RGB")
        fr.paste(mag, (round(ox), round(oy)))
        ImageDraw.Draw(ov).rounded_rectangle(
            (ox * S, oy * S, (ox + ow) * S, (oy + oh) * S), radius=8,
            outline=LIME + (int(230 * g),), width=4)
        tf = lambda p, sx=sx, sy=sy, sw=sw, ox=ox, oy=oy, ow=ow: (
            ox + (p[0] - sx) * ow / sw, oy + (p[1] - sy) * ow / sw)
    tr = lambda p: tuple(v * S for v in tf(p))
    for on, off, r in beat.get("rings", []):
        a = min(ease((t - on) / 0.25), ease((off - t) / 0.25))
        if a <= 0:
            continue
        drew = True
        pad = 6
        (ax, ay), (bx, by) = tr((r[0] - pad, r[1] - pad)), tr((r[2] + pad, r[3] + pad))
        glow = Image.new("RGBA", ov.size, (0, 0, 0, 0))
        ImageDraw.Draw(glow).rounded_rectangle((ax, ay, bx, by), radius=14,
                                               outline=LIME + (int(160 * a),), width=12)
        ov = Image.alpha_composite(ov, glow.filter(ImageFilter.GaussianBlur(7)))
        ImageDraw.Draw(ov).rounded_rectangle((ax, ay, bx, by), radius=14,
                                             outline=LIME + (int(255 * a),), width=6)
    for tc, r in beat.get("clicks", []):
        if not tc - 0.75 <= t <= tc + 0.55:
            continue
        drew = True
        px, py = tr(mid(r))
        d = ImageDraw.Draw(ov)
        if t >= tc:  # the pulse: a flash and two expanding rings at the point
            for lag in (0.0, 0.15):
                u = (t - tc - lag) / 0.45
                if 0 <= u <= 1:
                    rad = 10 + 64 * ease(u)
                    d.ellipse((px - rad, py - rad, px + rad, py + rad),
                              outline=LIME + (int(255 * (1 - u)),), width=6)
            u = (t - tc) / 0.25
            if u <= 1:
                rad = 20 - 10 * u
                d.ellipse((px - rad, py - rad, px + rad, py + rad), fill=LIME + (int(210 * (1 - u)),))
        u = ease((t - (tc - 0.75)) / 0.55)
        fade = max(0.0, min(1.0, (t - (tc - 0.75)) / 0.2, (tc + 0.55 - t) / 0.2))
        cx, cy = px + 110 * (1 - u), py + 80 * (1 - u)
        s = 2.4 * (0.82 if tc <= t < tc + 0.15 else 1.0)
        al = int(255 * fade)
        sh = Image.new("RGBA", ov.size, (0, 0, 0, 0))
        ImageDraw.Draw(sh).polygon(cursor_poly(cx + 3, cy + 4, s), fill=(0, 0, 0, int(al * 0.5)))
        ov = Image.alpha_composite(ov, sh.filter(ImageFilter.GaussianBlur(3)))
        ImageDraw.Draw(ov).polygon(cursor_poly(cx, cy, s), fill=(255, 255, 255, al),
                                   outline=(10, 10, 10, al), width=3)
    if drew:
        fr = Image.alpha_composite(fr.convert("RGBA"), ov.resize((W, H), Image.LANCZOS)).convert("RGB")
    return fr


def need_hf(cap):
    """The 1920 launcher frames the magnified beats use, extracted once from launcher.mkv."""
    raw = cap / "raw"
    ts = [float(l.split("\t")[1]) for l in (raw / "launcher.mkv.times").read_text().splitlines()
          if l.strip()]
    hf = cap / "hf"
    hf.mkdir(exist_ok=True)
    want = set()
    for b in BEATS:
        if b["src"] == "H":
            lo, hi = min(s for _, s in b["map"]) + B, max(s for _, s in b["map"]) + B
            want.update(range(max(0, bisect.bisect_right(ts, lo) - 2),
                              min(bisect.bisect_right(ts, hi) + 1, len(ts))))
    runs = []
    for i in sorted(i for i in want if not (hf / f"{i:05d}.png").exists()):
        if runs and i == runs[-1][1] + 1:
            runs[-1][1] = i
        else:
            runs.append([i, i])
    for a, b in runs:
        print("extracting launcher frames", a, b, flush=True)
        subprocess.run([FF, "-loglevel", "error", "-y", "-i", str(raw / "launcher.mkv"), "-vf",
                        f"select='between(n,{a},{b})'", "-vsync", "0", "-start_number", str(a),
                        str(hf / "%05d.png")], check=True)


def contact_sheet(frames, fps, out, step):
    picks = list(range(0, len(frames), step))
    cols, tw, th = 6, 320, 180
    sheet = Image.new("RGB", (cols * tw, math.ceil(len(picks) / cols) * th))
    d = ImageDraw.Draw(sheet)
    for k, i in enumerate(picks):
        x, y = (k % cols) * tw, (k // cols) * th
        sheet.paste(Image.open(frames[i]).resize((tw, th), Image.LANCZOS), (x, y))
        d.rectangle((x, y, x + 40, y + 12), fill=(0, 0, 0))
        d.text((x + 3, y + 1), f"{i / fps:.1f}s", fill=(255, 255, 0))
    sheet.save(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cap", type=Path)
    ap.add_argument("--out", type=Path)
    ap.add_argument("--fps", type=float, default=12)
    ap.add_argument("--sheet", type=Path, help="also write a contact sheet of every half second")
    ap.add_argument("--dither", default="bayer:bayer_scale=4")
    a = ap.parse_args()
    cap, fps, raw = a.cap, a.fps, a.cap / "raw"
    need_hf(cap)
    src = {
        "H": Source(cap / "hf", raw / "launcher.mkv.times", only_existing=True),
        "L": Source(raw / "lf", raw / "launcher.mkv.times"),
        "G": Source(raw / "gf", raw / "game.mkv.times"),
        "C": Source(raw / "web" / "frames", raw / "web" / "frames.times"),
    }
    work = cap / "compose2"
    shutil.rmtree(work, ignore_errors=True)
    (work / "f").mkdir(parents=True)
    frames = []
    for bi, b in enumerate(BEATS):
        for j in range(round(b["len"] * fps)):
            t = j / fps
            tq = math.floor(t * b["q"]) / b["q"] if "q" in b else t
            fr = render(t, b, src[b["src"]].at(lerp_keys(b["map"], tq) + B))
            frames.append(work / "f" / f"{len(frames):05d}.png")
            fr.save(frames[-1])
        print(f"beat {bi} {b['src']} {b['len']}s -> {len(frames)} frames", flush=True)
    out = a.out or cap / "hero.gif"
    pal, seq = work / "palette.png", str(work / "f" / "%05d.png")
    subprocess.run([FF, "-loglevel", "error", "-y", "-framerate", str(fps), "-i", seq, "-vf",
                    "palettegen=max_colors=256:stats_mode=full", str(pal)], check=True)
    subprocess.run([FF, "-loglevel", "error", "-y", "-framerate", str(fps), "-i", seq, "-i", str(pal),
                    "-lavfi", f"paletteuse=dither={a.dither}:diff_mode=rectangle", "-loop", "0",
                    str(out)], check=True)
    print(f"gif {out}: {len(frames)} frames, {len(frames) / fps:.1f} s, {os.path.getsize(out)} bytes")
    if a.sheet:
        contact_sheet(frames, fps, a.sheet, int(fps / 2))


if __name__ == "__main__":
    sys.exit(main())
