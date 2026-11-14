"""Tests picprog against the simulated chip in sim.py: python3 -m pytest"""
from pathlib import Path

import pytest

import picprog
from picprog import BLANK, Icsp, Image, ProgrammerError
from sim import SimError, SimPic

BOARDS = Path(__file__).resolve().parents[2] / "boards" / "tinystories"
CHIPS = {"PIC16F876": (0x09E0 | 7, "87X"), "PIC16F876A": (0x0E00 | 3, "87XA")}


def chip(name="PIC16F876A", **options):
    device_id, family = CHIPS[name]
    return SimPic(device_id, family, **options)


def quiet(*_):
    pass


def image_of(sim):
    return Image(flash={a: w for a, w in enumerate(sim.flash) if w != BLANK},
                 ids={i: w for i, w in enumerate(sim.ids) if w != BLANK},
                 config=sim.config,
                 eeprom={a: b for a, b in enumerate(sim.eeprom) if b != 0xFF})


def test_hex_round_trip():
    image = picprog.read_hex((BOARDS / "tinystories.hex").read_text())
    assert image.config == 0x3F32                   # HS, WDT off, PWRT on, BOR off, LVP off
    assert len(image.flash) > 1000
    again = picprog.read_hex(picprog.write_hex(image))
    assert again == image


def test_hex_rejects_bad_checksum():
    with pytest.raises(ProgrammerError):
        picprog.read_hex(":0200000400FFFA\n")


@pytest.mark.parametrize("name", CHIPS)
def test_identifies(name):
    device, configuration = picprog.detect(Icsp(chip(name)))
    assert device.name == name
    assert configuration.config == BLANK


def test_no_chip():
    sim = chip()
    sim.device_id = BLANK                           # nothing drives PGD: the pull-up
    with pytest.raises(ProgrammerError, match="no PIC answers"):
        picprog.detect(Icsp(sim))


@pytest.mark.parametrize("name", CHIPS)
@pytest.mark.parametrize("firmware", ["tinystories.hex", "loader.hex"])
def test_writes_and_verifies_over_old_contents(name, firmware):
    image = picprog.read_hex((BOARDS / firmware).read_text())
    sim = chip(name, fill=True, config=0x0F02)      # old code, code-protected
    icsp = Icsp(sim)
    picprog.write(icsp, image, log=quiet)
    assert image_of(sim) == Image(image.flash, {}, image.config, {})
    assert picprog.verify(icsp, picprog.detect(icsp)[0], image) == []
    assert "enter" in sim.events and not sim.programming


@pytest.mark.parametrize("name", CHIPS)
def test_ids_eeprom_and_read_back(name):
    image = Image(flash={0: 0x2805, 5: 0x0008, 4095: 0x3000, 8191: 0x0123},
                  ids={0: 0x3FF1, 1: 0x3FF2, 2: 0x3FF3, 3: 0x3FF4}, config=0x3F3A,
                  eeprom={0: 0x12, 7: 0x00, 255: 0xAB})
    sim = chip(name, fill=True)
    icsp = Icsp(sim)
    device = picprog.write(icsp, image, log=quiet)
    assert image_of(sim) == image
    assert picprog.read_image(icsp, device) == image


def test_verify_reports_differences():
    image = picprog.read_hex((BOARDS / "loader.hex").read_text())
    sim = chip()
    icsp = Icsp(sim)
    picprog.write(icsp, image, log=quiet)
    sim.flash[3] ^= 1
    sim.eeprom[9] = 0
    errors = picprog.verify(icsp, picprog.detect(icsp)[0], image)
    assert len(errors) == 2 and "0x0003" in errors[0] and "EEPROM 0x09" in errors[1]


def test_too_big_for_the_chip():
    sim = SimPic(0x0E40, "87XA", flash=4096, eeprom=128)        # PIC16F873A
    with pytest.raises(ProgrammerError, match="beyond the 4096 words"):
        picprog.write(Icsp(sim), Image(flash={5000: 0}, config=BLANK), log=quiet)


def test_low_voltage_programming():
    image = picprog.read_hex((BOARDS / "loader.hex").read_text())   # _LVP_OFF
    sim = chip(vpp="5v")                            # a new chip: LVP on
    icsp = Icsp(sim, lvp=True)
    with pytest.raises(ProgrammerError, match="--keep-lvp"):
        picprog.write(icsp, image, log=quiet)
    picprog.write(icsp, image, keep_lvp=True, log=quiet)
    assert sim.config == image.config | picprog.LVP_BIT
    assert sim.flash[:len(image.flash)] == [image.flash[a] for a in range(len(image.flash))]


def test_low_voltage_needs_the_lvp_bit():
    sim = chip(vpp="5v", config=0x3F32)             # LVP off: only VPP gets in
    with pytest.raises(ProgrammerError, match="try without --lvp"):
        picprog.detect(Icsp(sim, lvp=True))


def test_sim_catches_short_waits(monkeypatch):
    monkeypatch.setattr(picprog, "T_87XA_PROG", 2_000)
    image = picprog.read_hex((BOARDS / "loader.hex").read_text())
    with pytest.raises(SimError, match="write"):
        picprog.write(Icsp(chip()), image, log=quiet)
