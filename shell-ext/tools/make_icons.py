"""Generate the context-menu icons for the SpaceRecycle shell extension.

Icons are shipped as standalone .ico files rather than embedded resources with
fixed indices: a system icon index such as shell32.dll,44 is not stable across
Windows builds, and a menu that shows the wrong glyph is worse than one that
shows a plain dot.

Each level gets a distinct hue and shape so the four are distinguishable at
16x16, which is the size Explorer actually draws in a context menu.

Run:  python shell-ext/tools/make_icons.py
"""

from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent.parent / "icons"
SIZES = [16, 20, 24, 32, 48]

# (file stem, fill, ring, glyph)
# Two levels only, so each gets a hue that is unambiguous on its own, plus a
# glyph that still carries the meaning for colour-blind users at 16x16.
LEVELS = [
    ("important", (222, 145, 20), (160, 96, 8), "ring"),
    ("disposable", (46, 160, 67), (26, 110, 44), "check"),
    ("clear", (110, 118, 129), (72, 78, 86), "cross"),
]


def draw_icon(size: int, fill, ring, glyph: str) -> Image.Image:
    # Supersample, then downscale: Pillow's own antialiasing on a 16px circle
    # is not good enough for a badge this small.
    scale = 8
    s = size * scale
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    pad = int(s * 0.06)
    box = (pad, pad, s - pad, s - pad)
    d.ellipse(box, fill=fill, outline=ring, width=max(1, int(s * 0.045)))

    # Inner glyph, kept inside 60% of the badge so it never touches the ring.
    cx = cy = s / 2
    r = s * 0.30
    lw = max(1, int(s * 0.055))
    white = (255, 255, 255, 235)

    if glyph == "stop":
        d.line([(cx - r * 0.55, cy), (cx + r * 0.55, cy)], fill=white, width=lw)
    elif glyph == "ring":
        d.ellipse([cx - r * 0.5, cy - r * 0.5, cx + r * 0.5, cy + r * 0.5],
                  outline=white, width=lw)
    elif glyph == "dot":
        d.ellipse([cx - r * 0.3, cy - r * 0.3, cx + r * 0.3, cy + r * 0.3], fill=white)
    elif glyph == "check":
        d.line([(cx - r * 0.55, cy + r * 0.05), (cx - r * 0.12, cy + r * 0.5)],
               fill=white, width=lw)
        d.line([(cx - r * 0.12, cy + r * 0.5), (cx + r * 0.58, cy - r * 0.45)],
               fill=white, width=lw)
    elif glyph == "cross":
        d.line([(cx - r * 0.45, cy - r * 0.45), (cx + r * 0.45, cy + r * 0.45)],
               fill=white, width=lw)
        d.line([(cx + r * 0.45, cy - r * 0.45), (cx - r * 0.45, cy + r * 0.45)],
               fill=white, width=lw)

    return img.resize((size, size), Image.LANCZOS)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    sheets = []
    for stem, fill, ring, glyph in LEVELS:
        frames = [draw_icon(s, fill, ring, glyph) for s in SIZES]
        # The largest frame drives the .ico; Pillow writes every size into the
        # file so Explorer picks the right one per DPI.
        path = OUT / f"{stem}.ico"
        frames[-1].save(path, format="ICO", sizes=[(s, s) for s in SIZES])
        print(f"wrote {path}")
        sheets.append(frames[-1])

    # A contact sheet, so the four badges can be judged side by side at the
    # size Explorer actually draws them. Regenerated rather than hand-made so
    # it cannot drift from the icons it is meant to depict.
    cell = 48
    sheet = Image.new("RGBA", (cell * len(sheets), cell), (30, 34, 40, 255))
    for i, im in enumerate(sheets):
        small = im.resize((cell, cell), Image.LANCZOS)
        sheet.paste(small, (i * cell, 0), small)
    preview = OUT / "_preview.png"
    sheet.save(preview)
    print(f"wrote {preview}")


if __name__ == "__main__":
    main()