# Builds the GitHub social preview (1280x640): the dark lockup on a
# background of faint circuit traces; with --tagline, a tagline under it.
# Run from assets/ after wordmark.py; needs fontTools and Comfortaa.
import re, sys
from fontTools.ttLib import TTFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen

W, H = 1280, 640
TAGLINE = "Run whole boards, not just chips."
FONT = "/usr/share/fonts/truetype/comfortaa/Comfortaa-Regular.ttf"

def text_path(text, size, font_file, track=0):
    font = TTFont(font_file)
    gs, cmap, hmtx = font.getGlyphSet(), font.getBestCmap(), font["hmtx"]
    scale = size / font["head"].unitsPerEm
    pen, x = SVGPathPen(gs), 0
    for ch in text:
        g = cmap[ord(ch)]
        gs[g].draw(TransformPen(pen, (scale, 0, 0, -scale, x * scale, 0)))
        x += hmtx[g][0] + track
    return pen.getCommands(), (x - track) * scale

lockup = open("lockup-dark.svg").read()
lw, lh = map(float, re.search(r'viewBox="0 0 ([\d.]+) ([\d.]+)"', lockup).groups())
inner = re.search(r"<svg[^>]*>(.*)</svg>", lockup, re.S).group(1)

tagline = "--tagline" in sys.argv
scale = (820 if tagline else 940) / lw  # lockup width on the card
lx = (W - lw * scale) / 2
ly = 170 if tagline else (H - lh * scale) / 2
tag, tw = text_path(TAGLINE, 40, FONT, track=10)
tx, ty = (W - tw) / 2, ly + lh * scale + 70

def trace(points, pad=True):
    d = "M" + " L".join(f"{x} {y}" for x, y in points)
    end = points[-1]
    ring = f'<circle cx="{end[0]}" cy="{end[1]}" r="7" fill="none" stroke-width="3"/>' if pad else ""
    return f'<path d="{d}" fill="none" stroke-width="3" stroke-linejoin="round" stroke-linecap="round"/>{ring}'

traces = "\n    ".join([
    trace([(0, 560), (180, 560), (220, 600), (420, 600)]),
    trace([(0, 590), (120, 590), (150, 620), (300, 620)]),
    trace([(1280, 540), (1100, 540), (1060, 580), (880, 580)]),
    trace([(1280, 600), (1180, 600), (1150, 630), (1000, 630)]),
    trace([(0, 70), (140, 70), (180, 30), (330, 30)]),
    trace([(1280, 90), (1150, 90), (1110, 50), (960, 50)]),
])

svg = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#0b1220"/><stop offset="1" stop-color="#0c2a2a"/>
    </linearGradient>
    <radialGradient id="sun" cx="0.5" cy="0.5" r="0.5">
      <stop offset="0" stop-color="#ffb020" stop-opacity="0.22"/>
      <stop offset="1" stop-color="#ffb020" stop-opacity="0"/>
    </radialGradient>
    <radialGradient id="leaf" cx="0.5" cy="0.5" r="0.5">
      <stop offset="0" stop-color="#22c55e" stop-opacity="0.12"/>
      <stop offset="1" stop-color="#22c55e" stop-opacity="0"/>
    </radialGradient>
  </defs>
  <rect width="{W}" height="{H}" fill="url(#bg)"/>
  <circle cx="{lx + 128 * scale:.0f}" cy="{ly + 90 * scale:.0f}" r="300" fill="url(#sun)"/>
  <ellipse cx="{W/2}" cy="{H}" rx="760" ry="220" fill="url(#leaf)"/>
  <g stroke="#2dd4bf" opacity="0.18">
    {traces}
  </g>
  <g transform="translate({lx:.1f} {ly:.1f}) scale({scale:.4f})">{inner}</g>
{f'  <path transform="translate({tx:.1f} {ty:.1f})" d="{tag}" fill="#cbd5e1"/>' if tagline else ""}
</svg>
'''
open("social-preview-tagline.svg" if tagline else "social-preview.svg", "w").write(svg)
