#!/usr/bin/env python3
"""Draws a board file (boards/*/NAME.toml) as a schematic you can edit.

Parts are drawn with their package's real pins and numbers
(tools/schematic/symbols.json); wires are routed on a grid like PCB tracks,
horizontal and vertical only, never two nets on one track, with a dot where
a net branches. Power nets (GND*, VSS, 0V, +*, VCC, VDD) get power symbols
instead of wires.

The editor runs in the browser: drag parts and the wires follow, R rotates,
M mirrors, the wheel zooms, dragging the background pans. Save writes the
parts' places to NAME.layout.toml and the drawing to NAME.svg, both next to
the board file.

Usage: tools/schematic.py BOARD.toml            open the editor
       tools/schematic.py BOARD.toml --svg      just write NAME.svg (needs
                                                Firefox or Chromium, headless)
       options: --port N (default: a free one), --no-browser,
                -o OUT.svg|.pdf|.png (.pdf and .png need rsvg-convert)
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import tomllib
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

HERE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "schematic")
GND = re.compile(r"^(GND.*|VSS|0V)$")
VCC = re.compile(r"^(\+.*|VCC|VDD)$")
RANGE = re.compile(r"^(.*?)(\d+)\.\.(\d+)$")
STATIC = {"/": ("editor.html", "text/html"), "/editor.js": ("editor.js", "text/javascript"),
          "/router.js": ("router.js", "text/javascript"), "/editor.css": ("editor.css", "text/css")}


def expand(text):
    """'RC0..7' -> ['RC0', ..., 'RC7']; '7..0' counts down; else [text]."""
    m = RANGE.match(text)
    if not m:
        return [text]
    a, b = int(m[2]), int(m[3])
    step = 1 if b >= a else -1
    return [f"{m[1]}{i}" for i in range(a, b + step, step)]


def load_board(path):
    with open(path, "rb") as f:
        board = tomllib.load(f)
    parts = {}
    for ref, spec in board.get("parts", {}).items():
        params = {k: v for k, v in spec.items() if k != "model"}
        parts[ref] = {"model": spec.get("model", "?"),
                      "params": {k: v if isinstance(v, str) else json.dumps(v) for k, v in params.items()}}
    nets = []
    for name, entries in board.get("nets", {}).items():
        names = expand(name)
        columns = []
        for entry in entries:
            ref, _, pin = entry.partition(".")
            pins = expand(pin)
            if len(names) > 1 and len(pins) == 1:
                pins = pins * len(names)    # one pin on every net of the range
            if len(pins) != len(names):
                sys.exit(f"{path}: net {name}: {entry} doesn't match the range")
            columns.append([(ref, p) for p in pins])
        for i, n in enumerate(names):
            power = "gnd" if GND.match(n) else "vcc" if VCC.match(n) else None
            nets.append({"name": n, "bus": name if len(names) > 1 else None, "power": power,
                         "pins": [list(col[i]) for col in columns]})
    return {"name": board.get("name", os.path.basename(path)), "parts": parts, "nets": nets}


def layout_path(board_path):
    return os.path.splitext(board_path)[0] + ".layout.toml"


def load_layout(board_path):
    path = layout_path(board_path)
    if not os.path.exists(path):
        return {}
    with open(path, "rb") as f:
        return tomllib.load(f)


def write_layout(board_path, layout):
    lines = ["# Where tools/schematic.py draws each part (grid cells of 10 px);",
             "# drag the parts in its editor and Save, or edit by hand.", ""]
    for ref, p in layout.items():
        if not re.match(r"^[A-Za-z0-9_]+$", ref):
            continue
        lines.append(f"{ref} = {{ x = {int(p['x'])}, y = {int(p['y'])}, rotate = {int(p.get('rotate', 0)) % 360}, "
                     f"mirror = {'true' if p.get('mirror') else 'false'} }}")
    with open(layout_path(board_path), "w") as f:
        f.write("\n".join(lines) + "\n")


def page_data(board_path):
    with open(os.path.join(HERE, "symbols.json")) as f:
        symbols = json.load(f)
    symbols.pop("_doc", None)
    data = load_board(board_path)
    data["symbols"] = symbols
    data["layout"] = load_layout(board_path)
    data["file"] = os.path.basename(board_path)
    return data


def write_drawing(path, svg):
    """Writes the SVG, or converts it for .pdf and .png (with rsvg-convert)."""
    fmt = os.path.splitext(path)[1].lower().lstrip(".")
    if fmt in ("pdf", "png"):
        if not shutil.which("rsvg-convert"):
            raise OSError(f"writing .{fmt} needs rsvg-convert (librsvg); use .svg")
        subprocess.run(["rsvg-convert", "-f", fmt, "-z", "2" if fmt == "png" else "1", "-o", path],
                       input=svg.encode(), check=True)
    else:
        with open(path, "w") as f:
            f.write(svg)


def make_handler(board_path, svg_out, on_save):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def send(self, code, body, ctype):
            body = body.encode() if isinstance(body, str) else body
            self.send_response(code)
            self.send_header("Content-Type", ctype + "; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            path = urlparse(self.path).path
            if path in STATIC:
                name, ctype = STATIC[path]
                with open(os.path.join(HERE, name), "rb") as f:
                    body = f.read()
                if name == "editor.html":
                    try:
                        data = json.dumps(page_data(board_path)).replace("</", "<\\/")
                    except SystemExit as e:
                        return self.send(500, str(e), "text/plain")
                    body = body.replace(b"/*BOARD*/null", data.encode())
                self.send(200, body, ctype)
            else:
                self.send(404, "not found", "text/plain")

        def do_POST(self):
            if urlparse(self.path).path != "/api/save":
                return self.send(404, "not found", "text/plain")
            req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            saved = []
            if req.get("layout"):
                write_layout(board_path, req["layout"])
                saved.append(layout_path(board_path))
            if req.get("svg"):
                try:
                    write_drawing(svg_out, req["svg"])
                    saved.append(svg_out)
                except (OSError, subprocess.CalledProcessError) as e:
                    print("couldn't write", svg_out + ":", e, file=sys.stderr)
            for p in saved:
                print("wrote", os.path.relpath(p))
            self.send(200, json.dumps({"saved": [os.path.basename(p) for p in saved]}), "application/json")
            on_save(req)
    return Handler


def headless_browser():
    for name in ("firefox", "chromium", "chromium-browser", "google-chrome"):
        if shutil.which(name):
            return name
    return None


def export_svg(server, url, browser):
    """Loads the page in a headless browser; the page POSTs its SVG and the
    server stops."""
    # A snap browser can't see /tmp or hidden directories; ~/snap/NAME/common it can.
    snap_dir = os.path.expanduser(f"~/snap/{browser}/common")
    tmp = tempfile.mkdtemp(prefix="schematic-", dir=snap_dir if os.path.isdir(snap_dir) else None)
    try:
        if browser == "firefox":
            cmd = [browser, "--headless", "--no-remote", "--profile", tmp, "--window-size", "1200,900",
                   "--screenshot", os.path.join(tmp, "shot.png"), url]
        else:
            cmd = [browser, "--headless", "--disable-gpu", f"--user-data-dir={tmp}", "--dump-dom", url]
        subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=120)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
        server.shutdown()


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("board")
    ap.add_argument("-o", "--output", help="drawing to write: .svg, .pdf or .png (default: NAME.svg next to the board)")
    ap.add_argument("--svg", action="store_true", help="write the SVG without opening the editor")
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--no-browser", action="store_true")
    a = ap.parse_args()
    board = os.path.abspath(a.board)
    load_board(board)                                   # report mistakes now
    svg_out = a.output or os.path.splitext(board)[0] + ".svg"
    exported = threading.Event()

    def on_save(req):
        if req.get("export"):
            exported.set()

    server = ThreadingHTTPServer(("127.0.0.1", a.port), make_handler(board, svg_out, on_save))
    url = f"http://127.0.0.1:{server.server_address[1]}/"
    if a.svg:
        browser = headless_browser()
        if not browser:
            sys.exit("--svg needs Firefox or Chromium; open the editor and press Save instead")
        threading.Thread(target=export_svg, args=(server, url + "?export=1", browser), daemon=True).start()
        server.serve_forever()
        sys.exit(0 if exported.is_set() else "the browser didn't send the drawing")
    print(f"Schematic editor for {os.path.relpath(board)}: {url}  (Ctrl+C to stop)")
    if not a.no_browser:
        webbrowser.open(url)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
