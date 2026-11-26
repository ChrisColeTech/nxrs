"""Contact sheets in pure Python: screenshots scaled into a labelled grid, written as PNG.

No PowerShell/System.Drawing (a Windows PowerShell that hangs while starting held every report, 2026-11-26) and no
PIL: the PNGs are decoded by play_route, scaled nearest-neighbour and labelled with a small built-in 5x7 font
(labels are drawn in capitals).
"""
import operator
import struct
import zlib

import play_route

BG, FG, BAD = (24, 24, 24), (255, 255, 255), (255, 80, 40)
BAR = 26
FONT_SCALE = 2
# 5x7 glyphs: seven rows of five bits, as hex
FONT = {
    'A': '0e11111f111111', 'B': '1e11111e11111e', 'C': '0e11101010110e', 'D': '1e11111111111e',
    'E': '1f10101e10101f', 'F': '1f10101e101010', 'G': '0e11101711110f', 'H': '1111111f111111',
    'I': '0e04040404040e', 'J': '0702020202120c', 'K': '11121418141211', 'L': '1010101010101f',
    'M': '111b1515111111', 'N': '11111915131111', 'O': '0e11111111110e', 'P': '1e11111e101010',
    'Q': '0e11111115120d', 'R': '1e11111e141211', 'S': '0f10100e01011e', 'T': '1f040404040404',
    'U': '1111111111110e', 'V': '11111111110a04', 'W': '1111111515150a', 'X': '11110a040a1111',
    'Y': '1111110a040404', 'Z': '1f01020408101f',
    '0': '0e11131519110e', '1': '040c040404040e', '2': '0e11010204081f', '3': '1f02040201110e',
    '4': '02060a121f0202', '5': '1f101e0101110e', '6': '0608101e11110e', '7': '1f010204080808',
    '8': '0e11110e11110e', '9': '0e11110f01020c',
    '-': '0000001f000000', '_': '0000000000001f', '.': '00000000000c0c', '@': '0e11010d15150e',
    '?': '0e110102040004', '(': '02040808080402', ')': '08040202020408', ':': '000c0c000c0c00',
    ' ': '00000000000000',
}


def glyph(ch):
    hexrows = FONT.get(ch.upper(), FONT['?'])
    return [int(hexrows[i:i + 2], 16) for i in range(0, 14, 2)]


def text(canvas, width, x0, y0, s, colour):
    """Draws `s` into the RGB bytearray `canvas` (`width` pixels a row) at (x0, y0)."""
    for k, ch in enumerate(s):
        for r, bits in enumerate(glyph(ch)):
            for c in range(5):
                if bits >> (4 - c) & 1:
                    for dy in range(FONT_SCALE):
                        y = y0 + r * FONT_SCALE + dy
                        x = x0 + (k * 6 + c) * FONT_SCALE
                        o = (y * width + x) * 3
                        canvas[o:o + 3 * FONT_SCALE] = bytes(colour) * FONT_SCALE


def png_rgb(path, rgb, w, h):
    row = w * 3
    raw = b''.join(b'\0' + bytes(rgb[y * row:(y + 1) * row]) for y in range(h))
    chunk = lambda t, d: struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d))
    with open(path, 'wb') as f:
        f.write(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0))
                + chunk(b'IDAT', zlib.compress(raw, 6)) + chunk(b'IEND', b''))


def sheet(out, items, columns, cell_w):
    """`items`: [(png path, label)] -> one PNG of `columns` cells `cell_w` wide (16:9 pictures under a label)."""
    cell_h = cell_w * 9 // 16
    rows = -(-len(items) // columns)
    W, H = columns * cell_w, rows * (cell_h + BAR)
    canvas = bytearray(bytes(BG) * (W * H))
    for i, (path, label) in enumerate(items):
        x0, y0 = (i % columns) * cell_w, (i // columns) * (cell_h + BAR)
        text(canvas, W, x0 + 4, y0 + 5, label[:cell_w // (6 * FONT_SCALE) - 1], FG)
        try:
            img = play_route.decode(path)
        except (OSError, zlib.error, struct.error, IndexError):
            img = None
        if not img:
            text(canvas, W, x0 + 4, y0 + BAR + 4, 'unreadable', BAD)
            continue
        w, h, ch, src = img
        scale = min(cell_w / w, cell_h / h)
        ow, oh = max(1, int(w * scale)), max(1, int(h * scale))
        ox, oy = x0 + (cell_w - ow) // 2, y0 + BAR + (cell_h - oh) // 2
        pick = operator.itemgetter(*[int(x / scale) * ch + c for x in range(ow) for c in range(3)])
        for y in range(oh):
            o = ((oy + y) * W + ox) * 3
            canvas[o:o + ow * 3] = bytes(pick(src[min(h - 1, int(y / scale))]))
    png_rgb(out, canvas, W, H)
