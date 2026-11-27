"""Draws the README banner and GitHub social preview: the launcher's app icon and the name NXRS in
the title bar's wordmark style (bold mono, letter-spaced, the text colour) on the launcher's dark
canvas colour. The colours, the word and its spacing are read from the launcher's constants, so the
images follow the app when those change.

    python scripts/readme/banner.py      # needs Pillow; writes docs/images/readme/*.png
"""

import re
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[2]
LAUNCHER = ROOT / "src" / "launcher"
OUT = ROOT / "docs" / "images" / "readme"
FONTS = [
    Path("C:/Windows/Fonts/consolab.ttf"),  # the launcher's wordmark face on Windows
    Path("/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf"),
]


def constant(file: Path, name: str) -> str:
    """The right-hand side of `pub const NAME: ... = <value>;` in a launcher constants file."""
    m = re.search(rf"pub const {name}:[^=]*=\s*(.+?);", file.read_text(encoding="utf-8"))
    if not m:
        sys.exit(f"{name} not found in {file}")
    return m.group(1)


def dark(name: str) -> str:
    """The dark-mode hex of a `(light, dark)` theme token."""
    return re.findall(r'"(#[0-9a-fA-F]{6})"', constant(LAUNCHER / "src/constants/theme.rs", name))[1]


def draw(size: tuple[int, int], icon_h: int, text_h: int, path: Path) -> None:
    layout = LAUNCHER / "src/constants/layout.rs"
    word = constant(layout, "WORDMARK").strip('"')
    # the title bar draws 12 px with 1.2 px extra spacing: keep that ratio at banner scale
    spacing = float(constant(layout, "WORDMARK_SPACING")) / float(constant(layout, "WORDMARK_SIZE"))
    canvas, text = dark("CANVAS"), dark("TEXT")
    font_file = next((f for f in FONTS if f.exists()), None)
    if font_file is None:
        sys.exit("no bold mono font found (Consolas Bold or DejaVu Sans Mono Bold)")

    img = Image.new("RGB", size, canvas)
    icon = Image.open(LAUNCHER / "assets" / "app.png").convert("RGBA")
    icon = icon.resize((icon_h, icon_h), Image.LANCZOS)

    # a font size whose capitals' ink is text_h tall
    font = ImageFont.truetype(str(font_file), 100)
    cap = font.getbbox("N")
    font = ImageFont.truetype(str(font_file), round(100 * text_h / (cap[3] - cap[1])))
    gap = spacing * font.size
    glyphs = [(ch, font.getbbox(ch)) for ch in word]
    advances = [font.getlength(ch) for ch in word]
    # ink width: the advances between letters plus their spacing, then the last letter's ink
    ink_w = sum(advances[:-1]) + gap * (len(word) - 1) + glyphs[-1][1][2] - glyphs[0][1][0]
    top, bottom = glyphs[0][1][1], glyphs[0][1][3]

    between = round(icon_h * 0.30)
    total = icon_h + between + ink_w
    x = (size[0] - total) / 2
    img.paste(icon, (round(x), (size[1] - icon_h) // 2), icon)

    d = ImageDraw.Draw(img)
    tx = x + icon_h + between - glyphs[0][1][0]
    ty = (size[1] - (bottom - top)) / 2 - top
    for (ch, _), adv in zip(glyphs, advances):
        d.text((tx, ty), ch, font=font, fill=text)
        tx += adv + gap
    OUT.mkdir(parents=True, exist_ok=True)
    img.save(path, optimize=True)
    print(f"{path.relative_to(ROOT)}: {size[0]}x{size[1]}, {path.stat().st_size // 1024} KB")


if __name__ == "__main__":
    draw((1280, 400), 200, 92, OUT / "banner.png")
    draw((1280, 640), 256, 118, OUT / "social-preview.png")
