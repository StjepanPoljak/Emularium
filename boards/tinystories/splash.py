# Draws the start-up screen, splash.png (128x64, 1 bit): "Made with" and
# the Emularium logo and wordmark (assets/), shown while the first token is
# computed. gen_data.py turns it into the SPLASH table.
# Needs Comfortaa Bold and the X11 6x10 font; run from boards/tinystories.
import gzip
from PIL import Image, ImageDraw, ImageFont, PcfFontFile

S = 8                                       # supersampling
W, H = 128, 64
COMFORTAA = "/usr/share/fonts/truetype/comfortaa/Comfortaa-Bold.ttf"
FONT6X10 = "/usr/share/fonts/X11/misc/6x10-ISO8859-1.pcf.gz"   # the firmware's 4x6 draws a like o


def small_text(img, text, cx, y):
    """Text in the X11 6x10 font, centred on cx."""
    pcf = PcfFontFile.PcfFontFile(gzip.open(FONT6X10))
    x = cx - 6 * len(text) // 2
    for ch in text:
        glyph = pcf.glyph[ord(ch)]
        if glyph:                           # every glyph is a whole 6x10 cell
            img.paste(1, (x, y), glyph[3].convert("1"))
        x += 6


def logo(d, ox, oy, scale):
    """The bell jar of assets/logo.svg (viewBox 256), in one colour."""
    def p(x, y):
        return ((ox + x * scale) * S, (oy + y * scale) * S)

    def width(w):
        return max(1, round(w * scale * S))

    def circle(x, y, r, fill=True, w=0):
        (cx, cy), rr = p(x, y), r * scale * S
        d.ellipse((cx - rr, cy - rr, cx + rr, cy + rr), fill=255 if fill else None,
                  outline=None if fill else 255, width=width(w) if w else 0)

    def line(points, w):
        pts = [p(x, y) for x, y in points]
        d.line(pts, fill=255, width=width(w), joint="curve")
        for pt in (pts[0], pts[-1]):
            r = width(w) / 2
            d.ellipse((pt[0] - r, pt[1] - r, pt[0] + r, pt[1] + r), fill=255)

    # Dome: two sides and the arc (centre 128,116, radius 68).
    (ax0, ay0), (ax1, ay1) = p(60, 48), p(196, 184)
    d.arc((ax0, ay0, ax1, ay1), 180, 360, fill=255, width=width(11))
    line([(60, 116), (60, 204)], 11)
    line([(196, 116), (196, 204)], 11)
    circle(128, 38, 10)                     # the knob
    # The plant: stem, two branches with hollow nodes, the LED on top.
    line([(128, 203), (128, 100)], 11)
    line([(128, 172), (98, 142), (98, 126)], 11)
    line([(128, 148), (158, 118), (158, 108)], 11)
    circle(98, 120, 13)
    circle(158, 102, 13)
    circle(128, 86, 16)
    # Base.
    (bx0, by0), (bx1, by1) = p(36, 202), p(220, 222)
    d.rounded_rectangle((bx0, by0, bx1, by1), 6 * scale * S, fill=255)


def wordmark(d, x, baseline, size):
    font = ImageFont.truetype(COMFORTAA, size * S)
    text = "Emularıum"                      # dotless i: the dot is an LED
    track = size * S * 0.04
    cx = x * S
    for ch in text:
        d.text((cx, baseline * S), ch, font=font, fill=255, anchor="ls")
        if ch == "ı":
            l, t, r, b = d.textbbox((cx, baseline * S), ch, font=font, anchor="ls")
            led = (l + r) / 2
        cx += font.getlength(ch) + track
    end = cx - track
    # The LED above the i, and the trace under the word, as in the wordmark.
    xh = font.getbbox("x", anchor="ls")[1]
    ly, rr = baseline * S + xh - size * S * 0.22, size * S * 0.11
    d.ellipse((led - rr, ly - rr, led + rr, ly + rr), fill=255)
    ty = (baseline + 4) * S
    jog = 2 * S
    xs = x * S
    knee = xs + (end - xs) * 0.62
    d.line([(xs, ty), (knee, ty), (knee + jog, ty + jog), (end - 2 * S, ty + jog)], fill=255, width=S, joint="curve")
    d.ellipse((end - 3.5 * S, ty + jog - 1.5 * S, end - 0.5 * S, ty + jog + 1.5 * S), outline=255, width=S)
    return end / S


def main():
    big = Image.new("L", (W * S, H * S), 0)
    d = ImageDraw.Draw(big)
    logo(d, 1, 11, 0.19)
    # The largest wordmark that fits right of the logo.
    for size in range(20, 8, -1):
        if wordmark(ImageDraw.Draw(Image.new("L", big.size)), 0, 40, size) <= W - 44 - 2:
            break
    wordmark(d, 44, 43, size)
    print("wordmark size", size)
    img = big.resize((W, H), Image.BOX).point(lambda v: 255 if v >= 110 else 0).convert("1")
    small_text(img, "Made with", W // 2, 0)
    img.save("splash.png")


if __name__ == "__main__":
    main()
