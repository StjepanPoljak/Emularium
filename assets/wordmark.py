# Run from assets/ (needs fontTools and Comfortaa Bold). Builds the Emularium wordmark (and logo + wordmark lockup) as SVG with the
# letters converted to outlines, in a light-background and a dark-background
# palette.
import re, sys
from fontTools.ttLib import TTFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.boundsPen import BoundsPen

FONT = "/usr/share/fonts/truetype/comfortaa/Comfortaa-Bold.ttf"
TEXT = "Emularıum"          # dotless i: the dot is drawn as an LED
TRACK = 40                 # extra letter spacing, font units
font = TTFont(FONT)
gs, cmap, hmtx = font.getGlyphSet(), font.getBestCmap(), font["hmtx"]
upm = font["head"].unitsPerEm
asc = font["OS/2"].sCapHeight or 0.7 * upm

PALETTES = {
    "light": dict(a="#15803d", b="#0f766e", trace="#14b8a6", led="#ffb020"),
    "dark":  dict(a="#4ade80", b="#2dd4bf", trace="#2dd4bf", led="#ffb020"),
}

def outline(scale, x0, y0):
    """Text as one SVG path, baseline at y0; returns (d, width, i_stem_x, x_height)."""
    pen = SVGPathPen(gs)
    x, i_x = 0, None
    for ch in TEXT:
        g = cmap[ord(ch)]
        if ch == "ı":
            bp = BoundsPen(gs); gs[g].draw(bp)
            i_x = x + (bp.bounds[0] + bp.bounds[2]) / 2
        gs[g].draw(TransformPen(pen, (scale, 0, 0, -scale, x0 + x * scale, y0)))
        x += hmtx[g][0] + TRACK
    x -= TRACK
    bp = BoundsPen(gs); gs[cmap[ord("x")]].draw(bp)
    return pen.getCommands(), x * scale, x0 + i_x * scale, bp.bounds[3] * scale

def wordmark(p, x0, y0, size):
    scale = size / upm
    d, w, ix, xh = outline(scale, x0, y0)
    led_y = y0 - xh - size * 0.17
    r = size * 0.075
    ty = y0 + size * 0.2            # trace under the baseline
    sw = size * 0.035
    jog = size * 0.12
    return w, f'''
  <path d="{d}" fill="url(#g)"/>
  <circle cx="{ix:.1f}" cy="{led_y:.1f}" r="{r*2.6:.1f}" fill="url(#glow)"/>
  <circle cx="{ix:.1f}" cy="{led_y:.1f}" r="{r:.1f}" fill="{p['led']}"/>
  <path d="M{x0:.1f} {ty:.1f} H{x0 + w*0.62:.1f} L{x0 + w*0.62 + jog:.1f} {ty + jog:.1f} H{x0 + w - r*1.2:.1f}"
        fill="none" stroke="{p['trace']}" stroke-width="{sw:.1f}" stroke-linecap="round" stroke-linejoin="round" opacity="0.8"/>
  <circle cx="{x0 + w:.1f}" cy="{ty + jog:.1f}" r="{r*1.2:.1f}" fill="none" stroke="{p['trace']}" stroke-width="{sw:.1f}" opacity="0.8"/>''', ty + jog + r*1.2 + sw

def defs(p, x0, w):
    return f'''<defs>
    <linearGradient id="g" gradientUnits="userSpaceOnUse" x1="{x0}" y1="0" x2="{x0 + w}" y2="0">
      <stop offset="0" stop-color="{p['a']}"/><stop offset="1" stop-color="{p['b']}"/>
    </linearGradient>
    <radialGradient id="glow"><stop offset="0" stop-color="#ffc24a" stop-opacity="0.8"/><stop offset="1" stop-color="#ffc24a" stop-opacity="0"/></radialGradient>
  </defs>'''

logo = open("logo.svg").read()
logo_inner = re.search(r"<svg[^>]*>(.*)</svg>", logo, re.S).group(1).replace('id="glow"', 'id="lglow"').replace("url(#glow)", "url(#lglow)")

for name, p in PALETTES.items():
    size, pad = 160, 24
    w, body, bottom = wordmark(p, pad, pad + size * 0.95, size)
    W, H = w + 2 * pad, bottom + pad
    open(f"wordmark-{name}.svg", "w").write(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W:.0f} {H:.0f}">\n  {defs(p, pad, w)}{body}\n</svg>\n')
    # Lockup: logo mark left, wordmark right, centred on the mark.
    mark = 256
    x0 = mark + 8
    w, body, bottom = wordmark(p, x0, 150, 120)
    W = x0 + w + 32
    open(f"lockup-{name}.svg", "w").write(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W:.0f} 256">\n  {defs(p, x0, w)}\n  <g>{logo_inner}</g>{body}\n</svg>\n')
