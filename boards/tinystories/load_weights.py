#!/usr/bin/env python3
"""Writes the weight image into the TinyStories board's EEPROMs through
loader.asm, then reads it all back to check it.

Usage: load_weights.py PORT [IMAGE] [--from ADDR] [--no-verify]
  PORT   a serial port (/dev/ttyUSB0) or, for the emulator's loader.toml,
         socket://localhost:5555
  IMAGE  default: tinystories/out/weights_n1_k0.bin
  --from start at this image address (256-aligned), e.g. to resume

Needs pyserial (pip install pyserial). The protocol is in loader.asm.
"""

import argparse
import os
import sys
import time

import serial

PAGE = 256
CAPACITY = 3 * 128 * 1024
HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_IMAGE = os.path.join(HERE, "..", "..", "tinystories", "out", "weights_n1_k0.bin")


def crc16(data):
    """CRC-16/CCITT-FALSE, as loader.asm computes it."""
    crc = 0xFFFF
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) if crc & 0x8000 else crc << 1
            crc &= 0xFFFF
    return crc


class Loader:
    def __init__(self, port, baud):
        self.port = serial.serial_for_url(port, baudrate=baud, timeout=2)

    def read(self, n):
        data = self.port.read(n)
        if len(data) != n:
            raise TimeoutError(f"expected {n} bytes from the board, got {len(data)}")
        return data

    def hello(self, seconds=10):
        """Waits until the loader answers."""
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            self.port.reset_input_buffer()
            self.port.write(b"?")
            self.port.timeout = 0.5
            reply = self.port.read(5)
            self.port.timeout = 2
            if reply == b"EMLD1":
                time.sleep(0.05)
                self.port.reset_input_buffer()
                return
        raise TimeoutError("no answer from loader.asm; is it flashed and running?")

    @staticmethod
    def addr(a):
        return bytes([a >> 16, (a >> 8) & 0xFF, a & 0xFF])

    def write_page(self, a, data):
        crc = crc16(data)
        self.port.write(b"W" + self.addr(a) + data + bytes([crc >> 8, crc & 0xFF]))
        return self.read(1)

    def read_page(self, a):
        self.port.write(b"R" + self.addr(a))
        reply = self.read(PAGE + 2)
        data, crc = reply[:PAGE], reply[PAGE] << 8 | reply[PAGE + 1]
        if crc16(data) != crc:
            raise IOError(f"bad CRC reading {a:#07x}")
        return data


def pages(image, start):
    for a in range(start, len(image), PAGE):
        yield a, image[a:a + PAGE].ljust(PAGE, b"\xff")


def progress(what, done, total, t0):
    rate = done / max(time.monotonic() - t0, 1e-6)
    sys.stdout.write(f"\r{what}: {done}/{total} bytes, {rate / 1024:.1f} KB/s ")
    sys.stdout.flush()


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("port")
    ap.add_argument("image", nargs="?", default=DEFAULT_IMAGE)
    ap.add_argument("--from", dest="start", type=lambda s: int(s, 0), default=0)
    ap.add_argument("--baud", type=int, default=115200)
    ap.add_argument("--no-verify", action="store_true")
    a = ap.parse_args()

    image = open(a.image, "rb").read()
    if len(image) > CAPACITY:
        sys.exit(f"{a.image}: {len(image)} bytes, the EEPROMs hold {CAPACITY}")
    if a.start % PAGE:
        sys.exit("--from must be a multiple of 256")
    total = len(image) - a.start

    loader = Loader(a.port, a.baud)
    loader.hello()
    print(f"loader.asm is listening; writing {total} bytes from {a.start:#07x}")

    t0 = time.monotonic()
    for a_, data in pages(image, a.start):
        for attempt in range(3):
            reply = loader.write_page(a_, data)
            if reply == b"K":
                break
            print(f"\npage {a_:#07x}: reply {reply!r}, sending it again")
        else:
            sys.exit(f"page {a_:#07x} failed three times; resume with --from {a_:#x}")
        progress("written", min(a_ + PAGE, len(image)) - a.start, total, t0)
    print()

    if a.no_verify:
        return
    t0 = time.monotonic()
    bad = []
    for a_, data in pages(image, a.start):
        if loader.read_page(a_) != data:
            bad.append(a_)
        progress("verified", min(a_ + PAGE, len(image)) - a.start, total, t0)
    print()
    if bad:
        sys.exit(f"{len(bad)} pages differ, the first at {bad[0]:#07x}")
    print("the image is in the EEPROMs")


if __name__ == "__main__":
    main()
