"""A PIC16F87X(A) in programming mode, seen from its ICSP pins, for testing
picprog without hardware. It stands in for RpiGpio (drive, release, read,
delay_us) and follows the programming specifications (DS39025F, DS39589C)
as picprog reads them, so it catches mistakes in bit order, framing,
addressing and timing, not misreadings of the specifications.

Time is virtual: it advances only in delay_us. Anything the chip would not
tolerate (a command while it is still writing, PGD driven by both sides,
too short a gap between commands, a wrong entry sequence) raises SimError.
"""
from __future__ import annotations

import random

BLANK = 0x3FFF


class SimError(Exception):
    pass


class SimPic:
    def __init__(self, device_id: int, family: str, flash: int = 8192, eeprom: int = 256,
                 vpp: str = "high", config: int = BLANK, fill: bool = False):
        self.family = family
        self.vpp = vpp                      # what Q2 switches onto MCLR: "high" or "5v"
        self.device_id = device_id
        rng = random.Random(1)
        self.flash = [rng.randrange(BLANK) if fill else BLANK for _ in range(flash)]
        self.eeprom = [rng.randrange(256) if fill else 0xFF for _ in range(eeprom)]
        self.ids = [BLANK] * 4
        self.config = config
        self.now = 0.0                      # us
        self.host = {"PGC": False, "PGD": False, "PGM": False, "VPP": False}
        self.host_drives_pgd = True
        self.programming = False
        self.events: list[str] = []
        self.lvp_entry = False
        self.entered_at = 0.0
        self._reset()

    # -- the io interface

    def drive(self, name: str, level: bool) -> None:
        if name == "PGD":
            if self.device_drives:
                raise SimError(f"PGD driven by the host while the PIC drives it (t={self.now} us)")
            self.host_drives_pgd = True
        old = self.host[name]
        self.host[name] = level
        if name == "VPP" and level != old:
            self._mclr(level)
        elif name == "PGC" and level != old and self.programming:
            self._rising() if level else self._falling()
        elif name == "PGM" and self.programming and self.lvp_entry and not level:
            self._exit()

    def release(self, name: str) -> None:
        if name != "PGD":
            raise SimError(f"{name} released")
        self.host_drives_pgd = False

    def read(self, name: str) -> bool:
        if name != "PGD":
            raise SimError(f"{name} read")
        if self.device_drives:
            return self.device_bit
        if self.host_drives_pgd:
            return self.host["PGD"]
        return True                         # the level shifter's pull-up

    def delay_us(self, us: float) -> None:
        self.now += us

    # -- the chip

    def _reset(self) -> None:
        self.pc = 0
        self.latches = [BLANK] * 8
        self.latch = BLANK
        self.data_latch = 0xFF
        self.last_load = "prog"
        self.bits = 0
        self.count = 0
        self.phase = "command"
        self.command = None
        self.device_drives = False
        self.device_bit = True
        self.busy_until = 0.0
        self.ready_at = 0.0
        self.previous = None
        self.armed = False                  # 87X: Bulk Erase Setup1, Setup2
        self.bulk = None                    # 87XA: Bulk Erase Program/Data
        self.programming_only_since = None

    def _mclr(self, high: bool) -> None:
        if not high:
            self._exit()
            return
        if self.vpp == "high":
            if self.host["PGC"] or self.host["PGD"]:
                raise SimError("MCLR raised to VPP with PGC or PGD high")
            self.lvp_entry = False
        else:
            if not (self.host["PGM"] and self.config & 0x80):
                return                      # stays in reset: MCLR at VDD is a plain run
            self.lvp_entry = True
        self.programming = True
        self.entered_at = self.now
        self._reset()
        self.events.append("enter")

    def _exit(self) -> None:
        if self.programming:
            if self.now < self.busy_until:
                raise SimError("left programming mode during a write")
            self.events.append("exit")
        self.programming = False
        self._reset()

    def _clock_check(self) -> None:
        if self.now < self.entered_at + 5:
            raise SimError("clocked within thld0 of entering programming mode")
        if self.now < self.busy_until:
            raise SimError(f"clocked {self.busy_until - self.now} us before the write was over")
        if self.now < self.ready_at:
            raise SimError("less than 1 us between a command and what follows it")

    def _rising(self) -> None:
        self._clock_check()
        if self.phase == "read":
            # Cycle count+1: bits go out on the 2nd to 15th rising edges.
            if self.count == 1:
                if self.host_drives_pgd:
                    raise SimError("PGD still driven by the host at the start of a read")
                self.device_drives = True
            if 1 <= self.count <= 14:
                self.device_bit = bool(self.out >> (self.count - 1) & 1)
            elif self.count == 15:
                self.device_drives = False
            self.count += 1

    def _falling(self) -> None:
        if self.phase == "read":
            if self.count == 16:
                self._end_frame()
            return
        if not self.host_drives_pgd:
            raise SimError("PGD floating while the PIC latches it")
        self.bits |= int(self.host["PGD"]) << self.count
        self.count += 1
        if self.phase == "command" and self.count == 6:
            command, self.bits, self.count = self.bits, 0, 0
            self._command(command)
        elif self.phase == "data" and self.count == 16:
            word = self.bits >> 1 & BLANK
            self._load(self.command, word)
            self._end_frame()

    def _end_frame(self) -> None:
        self.phase, self.bits, self.count = "command", 0, 0
        self.ready_at = self.now + 1

    def _command(self, command: int) -> None:
        self.events.append(f"cmd {command:#04x}")
        self.ready_at = self.now + 1
        self.command = command
        armed = self.previous == 0x01 and command == 0x07
        self.previous = command
        low = command & 0x0F if self.family == "87X" else command
        if low in (0x00, 0x02, 0x03):
            self.phase = "data"
        elif low in (0x04, 0x05):
            self.phase = "read"
            self.out = self._fetch(low)
        elif low == 0x06:
            region = self.pc & 0x2000
            self.pc = region | (self.pc + 1) & 0x1FFF
        elif command == 0x08:
            self._begin_erase_program()
        elif command == 0x18:
            self._program_only()
        elif self.family == "87X" and command in (0x01, 0x07):
            pass
        elif self.family == "87XA" and command == 0x17:
            if self.programming_only_since is None:
                raise SimError("End Programming without Begin Programming Only")
            if self.now < self.programming_only_since + 1000:
                raise SimError("Begin Programming Only ended before tprog1 (1 ms)")
            self.programming_only_since = None
        elif self.family == "87XA" and command in (0x09, 0x0B):
            self.bulk = "prog" if command == 0x09 else "data"
        elif self.family == "87XA" and command == 0x1F:
            self._erase(program=True, data=True, config=True, ids=self.pc >= 0x2000)
            self.busy_until = self.now + 10_000
        else:
            raise SimError(f"unknown command {command:#04x} for the {self.family}")
        self.armed = armed

    def _load(self, command: int, word: int) -> None:
        low = command & 0x0F if self.family == "87X" else command
        if low == 0x00:
            self.pc = 0x2000
        if low == 0x03:
            self.data_latch = word & 0xFF
            self.last_load = "data"
        else:
            self.latch = word
            self.latches[self.pc & 7] = word
            self.last_load = "prog"

    def _fetch(self, low: int) -> int:
        if low == 0x05:
            return self.eeprom[self.pc % len(self.eeprom)]
        if self.pc < 0x2000:
            return self.flash[self.pc % len(self.flash)]
        offset = self.pc - 0x2000
        if offset < 4:
            return self.ids[offset]
        return {6: self.device_id, 7: self.config}.get(offset, BLANK)

    def _erase(self, program=False, data=False, config=False, ids=False) -> None:
        self.events.append(f"erase prog={program} data={data} config={config} ids={ids}")
        if program:
            self.flash = [BLANK] * len(self.flash)
        if data:
            self.eeprom = [0xFF] * len(self.eeprom)
        if config:
            self.config = BLANK
        if ids:
            self.ids = [BLANK] * 4

    def _store(self, address: int, word: int, erase: bool) -> None:
        def merge(old: int) -> int:
            return word if erase else old & word
        if address < 0x2000:
            self.flash[address] = merge(self.flash[address])
        elif address - 0x2000 < 4:
            self.ids[address - 0x2000] = merge(self.ids[address - 0x2000])
        elif address == 0x2007:
            self.config = merge(self.config)

    def _write(self, erase: bool) -> None:
        if self.last_load == "data":
            address = self.pc % len(self.eeprom)
            self.eeprom[address] = self.data_latch if erase else self.eeprom[address] & self.data_latch
        elif self.family == "87X":
            self._store(self.pc, self.latch, erase)
        elif self.pc < 0x2000:
            base = self.pc & ~7
            for i in range(8):
                self._store(base + i, self.latches[i], erase)
        elif self.pc == 0x2007:
            self._store(0x2007, self.latches[7], erase)
        elif self.pc < 0x2004:
            for i in range(4):
                self._store(0x2000 + i, self.latches[i], erase)
        self.latches = [BLANK] * 8

    def _begin_erase_program(self) -> None:
        if self.family == "87X":
            if self.armed:                  # Setup1, Setup2 just before
                if self.pc == 0x2007:
                    self._erase(program=True, data=True, config=True, ids=True)
                elif self.last_load == "data":
                    self._erase(data=True)
                else:
                    self._erase(program=True)
            else:
                self._write(erase=True)
            self.busy_until = self.now + 8_000
        else:
            if self.bulk:
                self._erase(program=self.bulk == "prog", data=self.bulk == "data")
                self.bulk = None
            else:
                self._write(erase=True)
            self.busy_until = self.now + 8_000

    def _program_only(self) -> None:
        self._write(erase=False)
        if self.family == "87X":
            self.busy_until = self.now + 4_000
        else:
            self.programming_only_since = self.now
