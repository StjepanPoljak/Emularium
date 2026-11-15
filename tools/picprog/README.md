# picprog: a Raspberry Pi PIC programmer

`picprog.py` programs a PIC16F873/874/876/877 or PIC16F873A/874A/876A/877A
from the GPIO header of a Raspberry Pi 3 or 4, with about €5 of parts. It
needs only Python 3.9 or newer, and no packages. The Pi 5 is not supported:
its GPIO is a different chip.

```
python3 picprog.py id                    # which chip, its revision and config word
python3 picprog.py write firmware.hex    # erase, program, verify
python3 picprog.py verify firmware.hex
python3 picprog.py read dump.hex
python3 picprog.py erase                 # also clears code protection
python3 picprog.py vpp 30                # MCLR at VPP for 30 s, to measure it
```

`id` is the first thing to run on an old chip. If it prints a name and a
config word, the chip is alive. If it prints "no PIC answers", check the
wiring and VPP.

Writing the TinyStories firmware, which is 7,340 words, takes about 17 s on
a PIC16F876A, which writes eight words at a time. On a PIC16F876 it takes
about 40 s, because that chip writes one word at a time. Verifying adds a
few seconds.

## Circuit

A 4-channel BSS138 level shifter handles the three logic lines. The PIC's
PGC and PGD inputs need 4 V at a 5 V supply, and the Pi's GPIO gives only
3.3 V. A 2N3904 and a 2N3906 switch MCLR between 0 V and VPP. VPP comes from
a 5 V to 12-13 V boost module. The PIC runs from the Pi's 5 V pin and needs
no crystal while it is being programmed.

```
 Raspberry Pi            level shifter            PIC16F876(A)
                         LV          HV
 pin 1   3.3 V --------- LV          HV --------- pin 20 VDD ---+--- pin 2 (5 V) of the Pi
 pin 6   GND ----------- GND        GND --------- pins 8, 19 VSS|
 pin 11  GPIO17 (PGC) -- LV1        HV1 --------- pin 27 RB6    |   100 nF from pin 20
 pin 13  GPIO27 (PGD) -- LV2        HV2 --------- pin 28 RB7    |   to pin 19
 pin 15  GPIO22 (PGM) -- LV3        HV3 --------- pin 24 RB3    |
                                                                |
 pin 2   5 V ---------------------------------------------------+--- boost IN+
 pin 6   GND ------------------------------------------------------- boost IN-

                       VPP: boost OUT+, set to 12.5-13 V
                        |   (or 5 V for --lvp, see below)
                        +---------+
                        |         |
                      10 k        | E
                        |      2N3906
                        +------B  Q2
                        |         | C
                      4.7 k       +------------------ pin 1 MCLR/VPP
                        |         |
                        | C      10 k
 pin 16  GPIO23 --4.7 k--B  Q1    |
                        | E      GND
                       GND   2N3904
```

Both transistors are shown with the flat side facing you: the 2N3904 legs
are E B C from left to right, and the 2N3906 legs are also E B C. The
boost's OUT- goes to GND. When GPIO23 is high, Q1 pulls Q2's base down, so
Q2 connects VPP to MCLR. Otherwise the 10 k pull-down holds MCLR at 0 V,
which keeps the PIC in reset with all its pins as inputs. GPIO23 has a
pull-down at power-up, so VPP stays off while the Pi boots. MCLR has to
reach VPP within 1 µs, which Q2 does easily.

## Building and bring-up

1. Wire everything except the PIC and the boost's OUT+.
2. Adjust the boost. Power the boost, measure OUT+ and turn its trimmer until
   it reads 12.5-13 V. The specs allow VDD + 3.5 V (8.5 V) to 13.5 V.
3. Check VPP. Connect OUT+ and run `python3 picprog.py vpp 30`. MCLR (the
   empty pin 1 position) should read 12-13 V for 30 s and 0 V afterwards.
4. Seat the PIC with the Pi off. Pin 1 is next to the notch. Then power on
   and run `python3 picprog.py id`.

VPP must only ever reach pin 1. Check the wiring before step 3, because
12 V on any other pin can destroy the PIC.

On Raspberry Pi OS the default user is already in the `gpio` group, which
`/dev/gpiomem` needs. Other GPIO pins can be used with
`--pins PGC,PGD,PGM,VPP` (BCM numbers).

## Low-voltage programming (no 12 V)

The 876 and 876A can also be programmed with VDD alone. This works only
while the config word's LVP bit is on. New chips come with LVP on, and the
TinyStories firmware keeps it on (`_LVP_ON`, as do the serial loader and
the EEPROM test), so a new chip never needs 12 V for that board. A chip
whose LVP bit was turned off, as old chips may have, needs VPP once.

To use it, connect Q2's emitter to 5 V instead of the boost and pass
`--lvp`. picprog then raises RB3 (PGM) before MCLR.

picprog refuses to write a config word with LVP off in this mode, because
the chip would then be unreachable without VPP. `--keep-lvp` keeps LVP on
instead.

With LVP on, RB3 is the PGM pin rather than an I/O pin. Give it a 10 k
pull-down on the target board: if it floats high, the chip can drop into
programming mode. The TinyStories board leaves RB3 alone for this; its LCD
runs on a 4-bit bus on RB4..RB7.

## Programming on the TinyStories board

Program the chip in the breadboard and then move it to the board. On the
board, RB6 and RB7 (PGC and PGD) are LCD data lines, and the LCD drives
them during a read, which RW and E can trigger while they float with the
PIC in reset.

## Tests

```
python3 -m pytest
```

The tests run picprog against `sim.py`, a pin-level model of a PIC16F876
and a PIC16F876A in programming mode. They write the TinyStories firmware
and the serial loader over random old contents with code protection on.
They also cover the data EEPROM and ID locations, reading back, verify
errors, and low-voltage mode with the LVP lock-out. The model raises an
error on any command sent before a write is over, on contention on PGD, and
on a bad entry sequence.

The model follows the programming specifications as picprog reads them:
Microchip DS39025F for the PIC16F87X and DS39589C for the PIC16F87XA. A
misreading of a specification would be in both, so only real hardware can
confirm the timing and the erase sequences.
