#!/usr/bin/env python3
"""A PIC16F87X / PIC16F87XA programmer for a Raspberry Pi 3 or 4.

The Pi drives the PIC's ICSP pins from its GPIO (see README.md for the
circuit): PGC (RB6), PGD (RB7) and PGM (RB3) through a 3.3 V <-> 5 V level
shifter, and VPP_EN through two transistors that switch MCLR between 0 V and
VPP (12-13 V, or 5 V for low-voltage programming). The programming
algorithms follow Microchip's DS39025F (PIC16F87X) and DS39589C
(PIC16F87XA) programming specifications.

    picprog.py id                      which chip is there, and its config word
    picprog.py write FIRMWARE.hex      erase, program and verify
    picprog.py verify FIRMWARE.hex
    picprog.py read DUMP.hex
    picprog.py erase
    picprog.py vpp [SECONDS]           MCLR at VPP, for checking it with a meter

The GPIO is accessed through /dev/gpiomem, so no packages are needed; the
user must be in the gpio group (the default user on Raspberry Pi OS is).
"""
from __future__ import annotations

import argparse
import mmap
import os
import sys
import time
from dataclasses import dataclass, field

# ICSP commands, 6 bits, sent LSB first.
LOAD_CONFIG = 0x00
LOAD_PROG = 0x02
LOAD_DATA = 0x03
READ_PROG = 0x04
READ_DATA = 0x05
INCREMENT = 0x06
BEGIN_ERASE_PROG = 0x08
BEGIN_PROG_ONLY = 0x18
END_PROG = 0x17           # PIC16F87XA only
BULK_SETUP1 = 0x01        # PIC16F87X only
BULK_SETUP2 = 0x07        # PIC16F87X only
CHIP_ERASE = 0x1F         # PIC16F87XA only

BLANK = 0x3FFF
CONFIG_ADDR = 0x2007
LVP_BIT = 1 << 7

# Waits, in microseconds. The specifications give 1 us for the delays
# between commands (100 ns at VDD >= 4.5 V) and maxima of 4 ms (87X
# erase or program), 8-10 ms (87XA erase/program and chip erase); the
# values here have some margin.
HALF_CLOCK = 5
GAP = 5
T_87X_PROG = 5_000
T_87X_ERASE_PROG = 10_000
T_87XA_PROG = 10_000
T_87XA_CHIP_ERASE = 15_000
T_ENTRY = 100             # thld0, 5 us
T_RESET = 10_000          # MCLR low before entry


@dataclass(frozen=True)
class Device:
    name: str
    family: str           # "87X" or "87XA"
    id: int               # device ID with the revision bits zero
    flash: int            # program memory words
    eeprom: int           # data EEPROM bytes

    @property
    def id_mask(self) -> int:
        # The 87X has a 9-bit device field above 5 revision bits, the 87XA
        # a 10-bit one above 4.
        return 0x3FE0 if self.family == "87X" else 0x3FF0


DEVICES = [
    Device("PIC16F873", "87X", 0x0960, 4096, 128),
    Device("PIC16F874", "87X", 0x0920, 4096, 128),
    Device("PIC16F876", "87X", 0x09E0, 8192, 256),
    Device("PIC16F877", "87X", 0x09A0, 8192, 256),
    Device("PIC16F873A", "87XA", 0x0E40, 4096, 128),
    Device("PIC16F874A", "87XA", 0x0E60, 4096, 128),
    Device("PIC16F876A", "87XA", 0x0E00, 8192, 256),
    Device("PIC16F877A", "87XA", 0x0E20, 8192, 256),
]


def identify(device_id: int) -> Device | None:
    for device in DEVICES:
        if device_id & device.id_mask == device.id:
            return device
    return None


class ProgrammerError(Exception):
    pass


# ---------------------------------------------------------------- hex files

@dataclass
class Image:
    flash: dict[int, int] = field(default_factory=dict)    # word address -> word
    ids: dict[int, int] = field(default_factory=dict)      # 0..3 -> word
    config: int | None = None
    eeprom: dict[int, int] = field(default_factory=dict)   # address -> byte


def read_hex(text: str) -> Image:
    """Parses an Intel HEX file as written by gpasm, MPASM or XC8 (INHX8M or
    INHX32): byte addresses are twice the word addresses, words are little
    endian, the ID locations are at 0x2000-0x2003, the config word at
    0x2007 and the data EEPROM at 0x2100."""
    data: dict[int, int] = {}
    base = 0
    for number, line in enumerate(text.splitlines(), 1):
        line = line.strip()
        if not line:
            continue
        if not line.startswith(":"):
            raise ProgrammerError(f"line {number}: not an Intel HEX record")
        record = bytes.fromhex(line[1:])
        if len(record) < 5 or len(record) != record[0] + 5 or sum(record) & 0xFF:
            raise ProgrammerError(f"line {number}: bad length or checksum")
        count, address, kind = record[0], record[1] << 8 | record[2], record[3]
        payload = record[4:4 + count]
        if kind == 0:
            for i, byte in enumerate(payload):
                data[base + address + i] = byte
        elif kind == 1:
            break
        elif kind == 2:
            base = (payload[0] << 8 | payload[1]) << 4
        elif kind == 4:
            base = (payload[0] << 8 | payload[1]) << 16
    image = Image()
    for word_address in sorted({address >> 1 for address in data}):
        low = data.get(2 * word_address, 0xFF)
        high = data.get(2 * word_address + 1, 0xFF)
        word = (high << 8 | low) & BLANK
        if word_address < 0x2000:
            image.flash[word_address] = word
        elif word_address < 0x2004:
            image.ids[word_address - 0x2000] = word
        elif word_address == CONFIG_ADDR:
            image.config = word
        elif 0x2100 <= word_address < 0x2200:
            image.eeprom[word_address - 0x2100] = low
    return image


def write_hex(image: Image) -> str:
    words: dict[int, int] = dict(image.flash)
    words.update({0x2000 + i: word for i, word in image.ids.items()})
    if image.config is not None:
        words[CONFIG_ADDR] = image.config
    words.update({0x2100 + address: byte for address, byte in image.eeprom.items()})
    data = {}
    for address, word in words.items():
        data[2 * address] = word & 0xFF
        data[2 * address + 1] = word >> 8

    def record(kind: int, address: int, payload: bytes) -> str:
        body = bytes([len(payload), address >> 8, address & 0xFF, kind]) + payload
        return ":" + (body + bytes([-sum(body) & 0xFF])).hex().upper()

    lines = [record(4, 0, b"\x00\x00")]
    addresses = sorted(data)
    i = 0
    while i < len(addresses):
        start = addresses[i]
        run = bytearray([data[start]])
        i += 1
        while i < len(addresses) and addresses[i] == start + len(run) and len(run) < 16:
            run.append(data[addresses[i]])
            i += 1
        lines.append(record(0, start, bytes(run)))
    lines.append(record(1, 0, b""))
    return "\n".join(lines) + "\n"


# --------------------------------------------------------------------- GPIO

class RpiGpio:
    """The BCM2835/2837 (Pi 3) or BCM2711 (Pi 4) GPIO block, through
    /dev/gpiomem. Pins are BCM numbers."""
    GPFSEL0, GPSET0, GPCLR0, GPLEV0 = 0x00, 0x1C, 0x28, 0x34
    GPPUD, GPPUDCLK0 = 0x94, 0x98           # Pi 3 pull-up/down
    GPIO_PUP_PDN_CNTRL0 = 0xE4              # Pi 4 pull-up/down

    def __init__(self, pins: dict[str, int]):
        try:
            fd = os.open("/dev/gpiomem", os.O_RDWR | os.O_SYNC)
        except OSError as error:
            raise ProgrammerError(f"cannot open /dev/gpiomem ({error.strerror}): this needs a "
                                  "Raspberry Pi 3 or 4 and a user in the gpio group") from None
        self.map = mmap.mmap(fd, 4096, mmap.MAP_SHARED, mmap.PROT_READ | mmap.PROT_WRITE)
        os.close(fd)
        self.regs = memoryview(self.map).cast("I")
        self.pins = pins
        try:
            with open("/proc/device-tree/compatible", "rb") as file:
                self.bcm2711 = b"bcm2711" in file.read()
        except OSError:
            self.bcm2711 = False
        for pin in pins.values():
            self._no_pull(pin)

    def _reg(self, offset: int) -> int:
        return offset // 4

    def _function(self, pin: int, output: bool) -> None:
        reg = self._reg(self.GPFSEL0) + pin // 10
        shift = pin % 10 * 3
        self.regs[reg] = self.regs[reg] & ~(7 << shift) | (int(output) << shift)

    def _no_pull(self, pin: int) -> None:
        if self.bcm2711:
            reg = self._reg(self.GPIO_PUP_PDN_CNTRL0) + pin // 16
            self.regs[reg] &= ~(3 << pin % 16 * 2)
        else:
            self.regs[self._reg(self.GPPUD)] = 0
            time.sleep(0.0001)
            self.regs[self._reg(self.GPPUDCLK0)] = 1 << pin
            time.sleep(0.0001)
            self.regs[self._reg(self.GPPUDCLK0)] = 0

    def drive(self, name: str, level: bool) -> None:
        pin = self.pins[name]
        self.regs[self._reg(self.GPSET0 if level else self.GPCLR0)] = 1 << pin
        self._function(pin, True)

    def release(self, name: str) -> None:
        self._function(self.pins[name], False)

    def read(self, name: str) -> bool:
        return bool(self.regs[self._reg(self.GPLEV0)] >> self.pins[name] & 1)

    def delay_us(self, us: float) -> None:
        if us >= 1000:
            time.sleep(us / 1e6)
            return
        end = time.perf_counter_ns() + int(us * 1000)
        while time.perf_counter_ns() < end:
            pass

    def close(self) -> None:
        # Inputs again: VPP_EN's default pull-down keeps MCLR at 0 V.
        for name in self.pins:
            self.release(name)


# --------------------------------------------------------------------- ICSP

class Icsp:
    """The serial programming protocol on PGC/PGD. io has drive(name,
    level), release(name), read(name) and delay_us(us)."""

    def __init__(self, io, lvp: bool = False):
        self.io = io
        self.lvp = lvp

    def enter(self) -> None:
        io = self.io
        io.drive("VPP", False)
        io.drive("PGC", False)
        io.drive("PGD", False)
        io.drive("PGM", False)
        io.delay_us(T_RESET)
        if self.lvp:
            io.drive("PGM", True)
            io.delay_us(GAP)
        io.drive("VPP", True)
        io.delay_us(T_ENTRY)

    def exit(self) -> None:
        io = self.io
        io.drive("PGC", False)
        io.drive("PGD", False)
        io.drive("VPP", False)
        io.delay_us(GAP)
        io.drive("PGM", False)
        io.delay_us(T_RESET)

    def _clock(self, bit: int) -> None:
        # The PIC latches PGD on the falling edge of PGC.
        io = self.io
        io.drive("PGD", bool(bit))
        io.drive("PGC", True)
        io.delay_us(HALF_CLOCK)
        io.drive("PGC", False)
        io.delay_us(HALF_CLOCK)

    def command(self, command: int) -> None:
        for i in range(6):
            self._clock(command >> i & 1)
        self.io.delay_us(GAP)

    def load(self, command: int, word: int) -> None:
        self.command(command)
        frame = (word & BLANK) << 1          # start bit 0, 14 bits, stop bit 0
        for i in range(16):
            self._clock(frame >> i & 1)
        self.io.delay_us(GAP)

    def read(self, command: int) -> int:
        self.command(command)
        io = self.io
        io.release("PGD")
        frame = 0
        for i in range(16):
            # The PIC drives PGD from the second rising edge to the 16th.
            io.drive("PGC", True)
            io.delay_us(HALF_CLOCK)
            frame |= int(io.read("PGD")) << i
            io.drive("PGC", False)
            io.delay_us(HALF_CLOCK)
        io.drive("PGD", False)
        io.delay_us(GAP)
        return frame >> 1 & BLANK

    def increment(self, times: int = 1) -> None:
        for _ in range(times):
            self.command(INCREMENT)

    def wait(self, us: int) -> None:
        self.io.delay_us(us)


# ---------------------------------------------------------------- algorithms

@dataclass
class Configuration:
    ids: list[int]
    device_id: int
    config: int


def read_configuration(icsp: Icsp) -> Configuration:
    icsp.enter()
    try:
        icsp.load(LOAD_CONFIG, BLANK)                # PC = 0x2000
        ids = []
        for i in range(4):
            if i:
                icsp.increment()
            ids.append(icsp.read(READ_PROG))
        icsp.increment(3)
        device_id = icsp.read(READ_PROG)
        icsp.increment()
        config = icsp.read(READ_PROG)
    finally:
        icsp.exit()
    return Configuration(ids, device_id, config)


def detect(icsp: Icsp) -> tuple[Device, Configuration]:
    configuration = read_configuration(icsp)
    device = identify(configuration.device_id)
    if device is None:
        if configuration.device_id in (0, BLANK):
            hint = ("the low-voltage mode needs the LVP bit, which the chip may have off: try "
                    "without --lvp" if icsp.lvp else
                    "check the wiring, the 5 V supply and VPP (12-13 V on MCLR during programming)")
            raise ProgrammerError(f"no PIC answers (device ID {configuration.device_id:#06x}): {hint}")
        raise ProgrammerError(f"unsupported device ID {configuration.device_id:#06x}")
    return device, configuration


def erase(icsp: Icsp, device: Device) -> None:
    """Erases program memory, data EEPROM and the config word, including
    code protection."""
    icsp.enter()
    try:
        if device.family == "87X":
            # DS39025F 2.5.2: works whether or not the chip is protected.
            icsp.load(LOAD_CONFIG, BLANK)
            icsp.increment(7)                        # 0x2007
            icsp.command(BULK_SETUP1)
            icsp.command(BULK_SETUP2)
            icsp.command(BEGIN_ERASE_PROG)
            icsp.wait(T_87X_ERASE_PROG)
            icsp.command(BULK_SETUP1)
            icsp.command(BULK_SETUP2)
        else:
            icsp.command(CHIP_ERASE)                 # PC in user memory: the IDs stay
            icsp.wait(T_87XA_CHIP_ERASE)
    finally:
        icsp.exit()


def program_flash(icsp: Icsp, device: Device, flash: dict[int, int], progress=None) -> None:
    """Writes program memory, which must be erased. Blank words are skipped."""
    top = max((address for address, word in flash.items() if word != BLANK), default=-1)
    if top >= device.flash:
        raise ProgrammerError(f"the image has code at {top:#06x}, beyond the "
                              f"{device.flash} words of the {device.name}")
    icsp.enter()
    try:
        if device.family == "87X":
            for address in range(top + 1):
                word = flash.get(address, BLANK)
                if word != BLANK:
                    icsp.load(LOAD_PROG, word)
                    icsp.command(BEGIN_PROG_ONLY)
                    icsp.wait(T_87X_PROG)
                if progress:
                    progress(address + 1, top + 1)
                icsp.increment()
        else:
            # Eight words at a time into the write latches (DS39589C 2.4).
            for base in range(0, top + 1, 8):
                words = [flash.get(base + i, BLANK) for i in range(8)]
                if all(word == BLANK for word in words):
                    icsp.increment(8)
                    continue
                for i, word in enumerate(words):
                    if i:
                        icsp.increment()
                    icsp.load(LOAD_PROG, word)
                icsp.command(BEGIN_ERASE_PROG)
                icsp.wait(T_87XA_PROG)
                if progress:
                    progress(base + 8, top + 1)
                icsp.increment()
    finally:
        icsp.exit()


def program_eeprom(icsp: Icsp, device: Device, eeprom: dict[int, int]) -> None:
    top = max((address for address, byte in eeprom.items() if byte != 0xFF), default=-1)
    if top >= device.eeprom:
        raise ProgrammerError(f"the image has EEPROM data at {top:#04x}, beyond the "
                              f"{device.eeprom} bytes of the {device.name}")
    if top < 0:
        return
    icsp.enter()
    try:
        for address in range(top + 1):
            byte = eeprom.get(address, 0xFF)
            if byte != 0xFF:
                icsp.load(LOAD_DATA, byte)
                icsp.command(BEGIN_ERASE_PROG)
                icsp.wait(T_87X_ERASE_PROG)
            icsp.increment()
    finally:
        icsp.exit()


def program_configuration(icsp: Icsp, device: Device, ids: dict[int, int], config: int) -> None:
    icsp.enter()
    try:
        icsp.load(LOAD_CONFIG, BLANK)                # PC = 0x2000
        if ids:
            for i in range(4):
                if i:
                    icsp.increment()
                icsp.load(LOAD_PROG, ids.get(i, BLANK))
                if device.family == "87X":
                    icsp.command(BEGIN_ERASE_PROG)
                    icsp.wait(T_87X_ERASE_PROG)
            if device.family == "87XA":
                icsp.command(BEGIN_ERASE_PROG)       # the four ID words at once
                icsp.wait(T_87XA_PROG)
            icsp.increment(4)
        else:
            icsp.increment(7)
        icsp.load(LOAD_PROG, config)                 # 0x2007
        icsp.command(BEGIN_ERASE_PROG)
        icsp.wait(T_87X_ERASE_PROG)
    finally:
        icsp.exit()


def read_flash(icsp: Icsp, count: int, progress=None) -> list[int]:
    words = []
    icsp.enter()
    try:
        for address in range(count):
            words.append(icsp.read(READ_PROG))
            if progress and address % 64 == 63:
                progress(address + 1, count)
            icsp.increment()
    finally:
        icsp.exit()
    return words


def read_eeprom(icsp: Icsp, count: int) -> list[int]:
    data = []
    icsp.enter()
    try:
        for _ in range(count):
            data.append(icsp.read(READ_DATA) & 0xFF)
            icsp.increment()
    finally:
        icsp.exit()
    return data


def read_image(icsp: Icsp, device: Device, progress=None) -> Image:
    flash = read_flash(icsp, device.flash, progress)
    eeprom = read_eeprom(icsp, device.eeprom)
    configuration = read_configuration(icsp)
    return Image(flash={a: w for a, w in enumerate(flash) if w != BLANK},
                 ids={i: w for i, w in enumerate(configuration.ids) if w != BLANK},
                 config=configuration.config,
                 eeprom={a: b for a, b in enumerate(eeprom) if b != 0xFF})


def verify(icsp: Icsp, device: Device, image: Image, progress=None, configuration=True) -> list[str]:
    """Compares the whole chip with the image (words the image doesn't have
    must be blank) and returns the differences."""
    errors = []
    flash = read_flash(icsp, device.flash, progress)
    for address, word in enumerate(flash):
        expected = image.flash.get(address, BLANK)
        if word != expected:
            errors.append(f"program memory {address:#06x}: read {word:#06x}, expected {expected:#06x}")
    eeprom = read_eeprom(icsp, device.eeprom)
    for address, byte in enumerate(eeprom):
        expected = image.eeprom.get(address, 0xFF)
        if byte != expected:
            errors.append(f"EEPROM {address:#04x}: read {byte:#04x}, expected {expected:#04x}")
    if configuration:
        chip = read_configuration(icsp)
        for i, word in image.ids.items():
            if chip.ids[i] != word:
                errors.append(f"ID {i}: read {chip.ids[i]:#06x}, expected {word:#06x}")
        if image.config is not None and chip.config != image.config:
            errors.append(f"config word: read {chip.config:#06x}, expected {image.config:#06x}")
    return errors


def write(icsp: Icsp, image: Image, keep_lvp: bool = False, check: bool = True, log=print,
          progress=None) -> Device:
    config = image.config if image.config is not None else BLANK
    if keep_lvp:
        config |= LVP_BIT
    if icsp.lvp and not config & LVP_BIT:
        raise ProgrammerError("the image's config word turns low-voltage programming off "
                              "(_LVP_OFF), after which --lvp can't reach the chip again; "
                              "program without --lvp (needs VPP), or pass --keep-lvp (RB3 "
                              "then stays the PGM pin, not an I/O pin)")
    image = Image(image.flash, image.ids, config, image.eeprom)
    device, _ = detect(icsp)
    log(f"{device.name}: erasing")
    erase(icsp, device)
    log("programming program memory")
    program_flash(icsp, device, image.flash, progress)
    if image.eeprom:
        log("programming data EEPROM")
        program_eeprom(icsp, device, image.eeprom)
    if check:
        # Before the config word, whose code protection would hide the rest.
        log("verifying")
        errors = verify(icsp, device, image, progress, configuration=False)
        if errors:
            raise ProgrammerError("verify failed:\n  " + "\n  ".join(errors[:20]))
    log(f"programming the config word {config:#06x}")
    program_configuration(icsp, device, image.ids, config)
    chip = read_configuration(icsp)
    if check and (chip.config != config or any(chip.ids[i] != w for i, w in image.ids.items())):
        raise ProgrammerError(f"config word: read {chip.config:#06x}, expected {config:#06x}")
    return device


# ---------------------------------------------------------------------- CLI

def describe_config(device: Device, config: int) -> str:
    oscillators = ["LP", "XT", "HS", "RC"]
    flags = [oscillators[config & 3],
             "WDT on" if config & 4 else "WDT off",
             "PWRT off" if config & 8 else "PWRT on",
             "BOR on" if config & 0x40 else "BOR off",
             "LVP on" if config & LVP_BIT else "LVP off",
             "CPD off" if config & 0x100 else "data EEPROM protected"]
    protected = (config & 0x3030 != 0x3030) if device.family == "87X" else not config & 0x2000
    flags.append("code protected" if protected else "CP off")
    return ", ".join(flags)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--lvp", action="store_true",
                        help="low-voltage programming: MCLR to 5 V, RB3 high (needs the LVP bit)")
    parser.add_argument("--keep-lvp", action="store_true",
                        help="set the LVP bit in the config word written")
    parser.add_argument("--pins", default="17,27,22,23", metavar="PGC,PGD,PGM,VPP",
                        help="BCM GPIO numbers (default 17,27,22,23)")
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("id", help="identify the chip and show its config word")
    commands.add_parser("erase", help="erase everything, including code protection")
    for name, text in (("write", "erase, program and verify"), ("verify", "compare with a hex file")):
        sub = commands.add_parser(name, help=text)
        sub.add_argument("hex")
        if name == "write":
            sub.add_argument("--no-verify", action="store_true")
    sub = commands.add_parser("read", help="read the chip into a hex file")
    sub.add_argument("hex")
    sub = commands.add_parser("vpp", help="hold MCLR at VPP for a while, to measure it")
    sub.add_argument("seconds", type=float, nargs="?", default=10)
    args = parser.parse_args(argv)

    pins = dict(zip(["PGC", "PGD", "PGM", "VPP"], map(int, args.pins.split(","))))
    if len(pins) != 4:
        parser.error("--pins takes four GPIO numbers")

    def progress(done: int, total: int) -> None:
        print(f"\r  {done}/{total} words", end="" if done < total else "\n", flush=True)

    try:
        gpio = RpiGpio(pins)
        icsp = Icsp(gpio, lvp=args.lvp)
        try:
            if args.command == "vpp":
                print(f"MCLR at VPP for {args.seconds:g} s (PGC, PGD low)")
                icsp.enter()
                try:
                    gpio.delay_us(args.seconds * 1e6)
                finally:
                    icsp.exit()
            elif args.command == "id":
                device, configuration = detect(icsp)
                print(f"{device.name}, revision {configuration.device_id & ~device.id_mask & 0x3FFF}")
                print(f"config word {configuration.config:#06x}: "
                      f"{describe_config(device, configuration.config)}")
                print("ID locations " + " ".join(f"{word:#06x}" for word in configuration.ids))
            elif args.command == "erase":
                device, _ = detect(icsp)
                erase(icsp, device)
                print(f"{device.name} erased")
            elif args.command == "write":
                start = time.monotonic()
                with open(args.hex) as file:
                    image = read_hex(file.read())
                device = write(icsp, image, args.keep_lvp, not args.no_verify, progress=progress)
                print(f"{device.name} programmed{'' if args.no_verify else ' and verified'} "
                      f"in {time.monotonic() - start:.0f} s")
            elif args.command == "verify":
                with open(args.hex) as file:
                    image = read_hex(file.read())
                device, _ = detect(icsp)
                errors = verify(icsp, device, image, progress)
                for error in errors[:20]:
                    print(error)
                print(f"{device.name}: {'verify failed' if errors else 'verified'}")
                return 1 if errors else 0
            elif args.command == "read":
                device, _ = detect(icsp)
                image = read_image(icsp, device, progress)
                with open(args.hex, "w") as file:
                    file.write(write_hex(image))
                print(f"{device.name} read into {args.hex}")
        finally:
            gpio.close()
    except ProgrammerError as error:
        print(f"picprog: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
