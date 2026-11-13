"""Checks tinystories.lst: every CALL/GOTO must reach its target with the
PCLATH page bits the code assumes (page 0 code: PCLATH<3> clear; page 1
code: set, except right after C0's BCF). Also checks the code stays in
its page."""
import re
import sys

lst = open(sys.argv[1] if len(sys.argv) > 1 else "tinystories.lst").read()
words = []          # (address, opcode word, source)
for line in lst.splitlines():
    m = re.match(r"^([0-9A-F]{4})   ([0-9A-F]{4}) ", line)
    if m:
        words.append((int(m.group(1), 16), int(m.group(2), 16), line[20:].strip()))
syms = {m.group(1): int(m.group(2), 16)
        for m in re.finditer(r"^(\w+)\s+ADDRESS\s+([0-9A-F]+)", lst, re.M)}
bad = 0
prev = None
for addr, w, src in words:
    if addr >= 0x1000:
        continue
    op = w >> 11
    if op in (0b100, 0b101) and (w >> 12) == 0b10:     # CALL 100kkk, GOTO 101kkk
        target_low = w & 0x7FF
        page = addr >> 11
        bit3 = page
        if prev is not None and prev[1] == 0x118A:          # BCF PCLATH,3
            bit3 = 0                                     # BCF PCLATH,3 just before
        if prev is not None and prev[1] == 0x158A:      # BSF PCLATH,3 just before
            bit3 = 1
        target = (bit3 << 11) | target_low
        name = [k for k, v in syms.items() if v == target]
        kind = "CALL" if op == 0b100 else "GOTO"
        # The target must be a code address in the page we jump to.
        expected_page = target >> 11
        if target >= 0x1000 or (not name and "$" not in src):
            print(f"{addr:04X}: {kind} to {target:04X}, no label there: {src}")
            bad += 1
        elif "$" in src and (target >> 11) != page:
            print(f"{addr:04X}: relative {kind} leaves its page: {src}")
            bad += 1
    prev = (addr, w)
for label in ("PAGE0_END", "PAGE1_END"):
    print(f"{label} = {syms.get(label, 0):04X}")
if syms.get("PAGE0_END", 0) > 0x800 or syms.get("PAGE1_END", 0) > 0x1000:
    print("code overflows its page")
    bad += 1
print("OK" if not bad else f"{bad} problems")
sys.exit(1 if bad else 0)
