#!/usr/bin/env python3
"""Turns the JSON dumps written by the TUI's screenshot gallery into PNG images.

    NVOKE_SHOTS=/tmp/shots cargo test --manifest-path crates/tui/Cargo.toml -- --ignored gallery
    python3 crates/tui/tools/render_screens.py /tmp/shots

Needs Pillow. Uses a monospace font with box-drawing glyphs if one is installed (a Nerd Font or DejaVu
Sans Mono look closest to a real terminal); box corners may look squarer than in your terminal.
"""
import glob, json, os, sys
from PIL import Image, ImageDraw, ImageFont

CANDIDATES = [
    ("/usr/share/fonts/TTF/MesloLGMNerdFontMono-Regular.ttf", "/usr/share/fonts/TTF/MesloLGMNerdFontMono-Bold.ttf"),
    ("/usr/share/fonts/TTF/DejaVuSansMono.ttf", "/usr/share/fonts/TTF/DejaVuSansMono-Bold.ttf"),
    ("/usr/share/fonts/dejavu/DejaVuSansMono.ttf", "/usr/share/fonts/dejavu/DejaVuSansMono-Bold.ttf"),
    ("/usr/share/fonts/liberation/LiberationMono-Regular.ttf", "/usr/share/fonts/liberation/LiberationMono-Bold.ttf"),
]
regular = bold = None
for r, b in CANDIDATES:
    if os.path.exists(r):
        regular, bold = r, b
        break
if regular is None:
    sys.exit("no monospace font found")

SIZE = int(os.environ.get("NVOKE_FONT", "16"))
font, font_bold = ImageFont.truetype(regular, SIZE), ImageFont.truetype(bold, SIZE)
cw, ch = int(font.getlength("M")), int(SIZE * 1.25)

def color(text, default):
    return tuple(map(int, text.split(","))) if text != "none" else default

for path in sorted(glob.glob(os.path.join(sys.argv[1], "*.json"))):
    data = json.load(open(path))
    w, h = data["w"], data["h"]
    image = Image.new("RGB", (w * cw, h * ch), (0, 0, 0))
    draw = ImageDraw.Draw(image)
    for i, (symbol, fg, bg, is_bold) in enumerate(data["cells"]):
        x, y = i % w, i // w
        draw.rectangle([x * cw, y * ch, (x + 1) * cw, (y + 1) * ch], fill=color(bg, (0, 0, 0)))
        if symbol.strip():
            draw.text((x * cw, y * ch), symbol, font=font_bold if is_bold else font, fill=color(fg, (230, 230, 230)))
    out = path[:-5] + ".png"
    image.save(out)
    print(out, image.size)
