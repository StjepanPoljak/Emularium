# Board files and screens

A board is a TOML file loaded at run time, so a new setup needs no
recompiling:

```sh
cargo run --release -- boards/tetris/tetris.toml          # the default board
cargo run --release -- boards/spiram/spiram.toml --check  # print what was loaded
```

KiCad schematics (`.kicad_sch`, `.net`) still load the same way; a board
file is the quick option when there's no schematic yet.

## Format

```toml
name = "Tetris"                     # optional, shown in VNC viewers' title bars

[parts]
U1 = { model = "PIC16F876", rom = "../../TETRIS.hex", fosc = "4MHz" }
J1 = { model = "ST7920", rotate = 270, scale = 4 }
J2 = { model = "74C922", hold = "130ms", keys = { a = 3, space = 5, left = 3 } }

[nets]
LCD_E = ["U1.RA2", "J1.E"]
"LCD_DB0..7" = ["U1.RC0..7", "J1.DB0..7"]   # 8 nets, LCD_DB0 = U1.RC0 + J1.DB0, ...
"+5V" = ["U2.HOLD"]                          # power rail: driven high
```

- **Parts**: a name (used in pins, VCD traces and tests) and `model`; the
  other keys are the model's parameters. Paths are relative to the board file.
- **Nets**: a name and a list of `PART.PIN`, with the model's pin names.
  `A..B` (also counting down, `7..0`) makes one net per number; every pin
  entry of such a net needs a range of the same length.
- Nets called `GND*`, `VSS`, `0V` are driven low; `+*`, `VCC`, `VDD` high.
- Nets `NAME0..NAMEn` are also traced as bus `NAME` (`--vcd out.vcd NAME`).
- Mistakes are reported with the file and net: unknown parts or pins (with
  the pins the model has), a pin on two nets, mismatched ranges.

`boards/tetris/tetris.toml` and `boards/spiram/spiram.toml` wire exactly
what their schematics do; a test checks this.

## Models

| model | parameters | pins |
|---|---|---|
| `PIC16F876` | `rom` (Intel HEX), `fosc` (`4MHz`) | `RA0`-`RA5`, `RB0`-`RB7`, `RC0`-`RC7` |
| `ST7920` | `vnc` (port), `scale` (1-16, default 4), `rotate` (0/90/180/270 clockwise) | `RS RW E DB0`-`DB7` |
| `74C922` | `hold` (`130ms`), `keys` (`{ a = 3, space = 5 }`) | `D0`-`D3 DA` |
| `23LC1024` | | `CS SO SI SCK HOLD` |
| `25LC1024` | `image` (file to preload), `offset` (where this chip's 128K starts in it: `131072` or `128K`) | `CS SO SI SCK HOLD WP` |

Keys are one character, or `space`, `enter`, `left`, `right`, `up`,
`down`.

## Screens over VNC

Each display gets its own VNC server on 127.0.0.1 (no password, so not
reachable from other machines). Ports come from `vnc = PORT`, else the
first free one from 5900 up; the emulator prints them:

```
VNC J1: localhost:5900 (256x512)
```

Open it with any viewer, e.g. `vncviewer localhost:5900` (TigerVNC),
`remmina vnc://localhost:5900`, or `gvncviewer localhost:0`. Key presses
in any viewer go to the board's keypads; Ctrl+C in the terminal stops the
emulator (and writes out a VCD trace).
