#!/usr/bin/env python3
"""Draws a board file (boards/*/NAME.toml) as a block diagram, with Graphviz.

Parts are boxes (name, model, parameters). Connections are drawn per group
of parts, not per pin:
- the nets between the same two parts become one line, labelled with each
  net and the pins it joins, e.g. "LCD_DB0..7: RB0..7 - DB0..7";
- nets that join three or more parts meet at a small hub; nets with the
  same set of parts share a hub (an SPI bus is one hub), and each line from
  the hub lists that part's pins;
- power rails (GND*, VSS, 0V, +*, VCC, VDD) are drawn as rail hubs.

Usage: tools/schematic.py BOARD.toml [-o OUT.svg|.png|.pdf|.dot]
       (default: BOARD.svg next to the board file; needs Graphviz's `dot`
       for anything but .dot)
"""

import argparse
import html
import os
import re
import shutil
import subprocess
import sys
import tomllib
from collections import defaultdict

RAIL = re.compile(r"^(GND.*|VSS|0V|\+.*|VCC|VDD)$")
PALETTE = {"mcu": "#dbeafe", "display": "#dcfce7", "memory": "#fef3c7", "other": "#f3f4f6"}


def kind(model):
    m = model.upper()
    if m.startswith("PIC"):
        return "mcu"
    if m.startswith("ST7920"):
        return "display"
    if re.match(r"^2[35]", m):
        return "memory"
    return "other"


def split_pin(entry):
    part, _, pin = entry.partition(".")
    return part, pin


def load(path):
    with open(path, "rb") as f:
        board = tomllib.load(f)
    parts = board.get("parts", {})
    nets = []                                   # (name, {part: [pins]})
    for name, pins in board.get("nets", {}).items():
        by_part = defaultdict(list)
        for entry in pins:
            part, pin = split_pin(entry)
            by_part[part].append(pin)
        nets.append((name, dict(by_part)))
    return board.get("name", os.path.basename(path)), parts, nets


def part_label(name, spec):
    model = spec.get("model", "?")
    rows = [f'<tr><td><b>{html.escape(name)}</b></td></tr>',
            f'<tr><td>{html.escape(model)}</td></tr>']
    params = []
    for k, v in spec.items():
        if k == "model":
            continue
        if isinstance(v, str) and "/" in v:
            v = os.path.basename(v)                 # a file: its name is enough
        if isinstance(v, dict):
            v = " ".join(f"{kk}:{vv}" for kk, vv in v.items())
        params.append(f"{k} = {v}")
    for p in params:
        if len(p) > 44:
            p = p[:41] + "..."
        rows.append(f'<tr><td><font point-size="9">{html.escape(str(p))}</font></td></tr>')
    color = PALETTE[kind(model)]
    return (f'<<table border="1" cellborder="0" cellspacing="1" cellpadding="3" '
            f'bgcolor="{color}">{"".join(rows)}</table>>')


def order(parts, names):
    """MCUs first, then the rest by name: lines are drawn from the MCU out."""
    return sorted(names, key=lambda n: (kind(parts.get(n, {}).get("model", "")) != "mcu", n))


def dot(title, parts, nets):
    out = ["graph board {",
           f'  label=<<b>{html.escape(title)}</b>>; labelloc=t; fontname="Helvetica"; fontsize=18;',
           '  graph [rankdir=LR, splines=true, overlap=false, nodesep=0.6, ranksep=1.4, pad=0.3];',
           '  node [shape=plaintext, fontname="Helvetica", fontsize=11];',
           '  edge [fontname="Helvetica", fontsize=9, color="#475569"];']
    for name, spec in parts.items():
        out.append(f'  "{name}" [label={part_label(name, spec)}];')

    pairs = defaultdict(list)                   # (a, b) -> label lines
    hubs = defaultdict(list)                    # (rail?, parts) -> nets
    for name, by_part in nets:
        rail = bool(RAIL.match(name))
        if len(by_part) == 2 and not rail:
            a, b = order(parts, by_part)
            pa, pb = by_part[a], by_part[b]
            pairs[(a, b)].append(f"{name}: {', '.join(pa)} - {', '.join(pb)}")
        else:
            hubs[(rail, tuple(sorted(by_part)))].append((name, by_part))

    for (a, b), lines in pairs.items():
        label = "<br/>".join(html.escape(line) for line in lines)
        out.append(f'  "{a}" -- "{b}" [label=<{label}>, penwidth={1 + min(len(lines), 4) * 0.5}];')

    for i, ((rail, members), group) in enumerate(hubs.items()):
        hub = f"hub{i}"
        names = ", ".join(n for n, _ in group)
        if rail:
            out.append(f'  {hub} [shape=box, style="rounded,filled", fillcolor="#fee2e2", '
                       f'fontsize=10, label="{html.escape(names)}"];')
        else:
            out.append(f'  {hub} [shape=ellipse, style=filled, fillcolor="#e0e7ff", '
                       f'fontsize=10, label="{html.escape(names)}"];')
        for part in order(parts, members):
            pins = ", ".join(", ".join(by_part[part]) for _, by_part in group)
            style = ', style=dashed, color="#b91c1c"' if rail else ""
            ends = (f'"{part}" -- {hub}' if kind(parts.get(part, {}).get("model", "")) == "mcu"
                    else f'{hub} -- "{part}"')
            out.append(f'  {ends} [label="{html.escape(pins)}"{style}];')
    out.append("}")
    return "\n".join(out) + "\n"


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("board")
    ap.add_argument("-o", "--output")
    a = ap.parse_args()
    title, parts, nets = load(a.board)
    src = dot(title, parts, nets)
    out = a.output or os.path.splitext(a.board)[0] + ".svg"
    fmt = os.path.splitext(out)[1].lstrip(".") or "svg"
    if fmt == "dot":
        open(out, "w").write(src)
    else:
        if not shutil.which("dot"):
            sys.exit("Graphviz's dot isn't installed; use -o FILE.dot")
        subprocess.run(["dot", f"-T{fmt}", "-o", out], input=src.encode(), check=True)
    print(out)


if __name__ == "__main__":
    main()
