<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/lockup-dark.svg">
    <img src="assets/lockup-light.svg" alt="Emularium" width="520">
  </picture>
</p>

Emularium runs whole boards, not just chips. You describe a board, either
as a KiCad schematic or as a small TOML file. Emularium wires up the parts,
loads the firmware into the microcontroller and runs it all in real time.
The board's displays appear in a VNC viewer, keys typed in the viewer go to
its keypads, and every net can be traced to a VCD file for GTKWave or
sigrok.

It simulates digital logic, not electronics. Each signal is 0, 1 or
undriven, and timing is modelled as delays between events.

## What it emulates

| Model | Part |
|---|---|
| `PIC16F876` | Microchip PIC16F876 at 4 MHz, running an Intel HEX image |
| `ST7920` | 128x64 graphic LCD on a parallel bus, with busy-flag readback |
| `74C922` | 16-key keypad encoder |
| `23LC1024` | 128 KB SPI SRAM |
| `25LC1024` | 128 KB SPI EEPROM, optionally preloaded from a file |

The PIC core has ports A-C and Timer 1. It doesn't emulate interrupts, TMR0,
TMR2, the watchdog, MSSP, the USART, the ADC or CCP.

Three boards come with it:

- `boards/tetris`: a PIC playing Tetris (`TETRIS.hex`) on an ST7920 LCD, with
  a 74C922 keypad.
- `boards/spiram`: a PIC bit-banging SPI to a 23LC1024 and checking what it
  reads back.
- `boards/tinystories`: the start of a PIC writing children's stories with
  a tiny language model, its weights in three 25LC1024 EEPROMs. For now
  its firmware only checks that it can read them; see
  `tinystories/ROADMAP.md`. It needs the weight image from
  `tinystories/export.py`.

## Building

You need Rust 1.85 or newer (edition 2024).

```sh
cargo build --release
```

The binary is `target/release/emularium`.

## Running

```sh
cargo run --release                                # the Tetris board
cargo run --release -- boards/spiram/spiram.toml   # another board file
cargo run --release -- boards/tetris/tetris.kicad_sch
```

Emularium prints a VNC address for each display:

```
VNC J1: localhost:5900 (256x512)
Running boards/tetris/tetris.toml; Ctrl+C stops.
```

Open it with any VNC viewer, for example `vncviewer localhost:5900`. In
Tetris, `a`/`d` or the left and right arrows move the piece, `s` or the down
arrow moves it down, and space or the up arrow rotates it. Press Ctrl+C in
the terminal to stop.

The VNC servers listen on 127.0.0.1 only, because they have no password.

### Checking a board

`--check` prints what was loaded and exits:

```sh
$ cargo run --release -- --check
Emulated: J1 (ST7920), J2 (74C922), U1 (PIC16F876)
Buses: KEY_D, LCD_DB
...
```

### Tracing signals

`--vcd FILE` records nets and buses to a VCD file. Name the signals you want,
or leave the names out to record all of them. The file is written when you
press Ctrl+C.

```sh
cargo run --release -- --vcd out.vcd LCD_E LCD_DB
gtkwave out.vcd
```

A Tetris run writes about 1.5 MB per simulated second with every signal
traced.

## Making a board

A board file names the parts and lists which pins each net connects:

```toml
name = "Tetris"

[parts]
U1 = { model = "PIC16F876", rom = "../../TETRIS.hex", fosc = "4MHz" }
J1 = { model = "ST7920", rotate = 270, scale = 4 }

[nets]
LCD_E = ["U1.RA2", "J1.E"]
"LCD_DB0..7" = ["U1.RC0..7", "J1.DB0..7"]
```

In a KiCad schematic, give each part to emulate an `Emu` field such as
`PIC16F876 rom=../../TETRIS.hex fosc=4MHz`. Everything else is treated as a
drawing only. Loading a `.kicad_sch` file needs `kicad-cli`; an exported
`.net` file doesn't.

[docs/BOARDS.md](docs/BOARDS.md) describes the whole format and every model's
parameters and pins.

### Firmware

Firmware is built with gputils. For example, the SPI RAM test:

```sh
cd boards/spiram
gpasm -p p16f876 -o spiram_test.hex spiram_test.asm
```

## Testing

```sh
cargo test
```

Some tests call external tools:

| Tool | Used for |
|---|---|
| `gpasm`, `gpsim` (gputils, gpsim) | running small test programs on the PIC core and in gpsim, then comparing the registers |
| `sigrok-cli` | decoding the SPI traffic on the SPI RAM board |
| `kicad-cli` (KiCad 9) | checking that the board files match their schematics |

## More

- [docs/BOARDS.md](docs/BOARDS.md): board files, models and VNC screens.
- [docs/SPI_RAM.md](docs/SPI_RAM.md): the 23LC1024, its wiring and its model.
- [docs/ROADMAP.md](docs/ROADMAP.md): where this is going, which is using
  Emularium as a testbed for computer architecture experiments, such as an
  external RAM with the PIC's own RAM as a cache.

## Licence

Copyright © 2026 Stjepan Poljak.

Emularium is free software: you can redistribute it and/or modify it under
the terms of the GNU Affero General Public License as published by the Free
Software Foundation, either version 3 of the License, or (at your option)
any later version. See [LICENSE](LICENSE).

If you run a modified Emularium as a network service, the AGPL requires you
to offer its source code to the people who use it.

Companies that want to use Emularium without the AGPL's terms need a
commercial licence, agreed with the author and every contributor whose
code it covers. See [CONTRIBUTING.md](CONTRIBUTING.md).
