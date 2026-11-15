// ST7920 graphic LCD controller on a 128x64 module ("12864"), after the
// Sitronix datasheet V4.0 (2008-08-18).
//
// Emulated:
// - 8-bit and 4-bit parallel bus (PSB high or open) and the serial bus (PSB
//   low; RS is CS, RW is SID, E is SCLK; write only, as on the chip).
// - Busy flag and execution times: 72 us per instruction or data byte,
//   1.6 ms for Display Clear, at fosc = 540 kHz (scaled for other fosc).
//   Anything written while busy is ignored, as is anything written in the
//   40 ms after power-on or while RST is low.
// - Basic and extended instruction sets: DL, RE and G with the datasheet's
//   rules (DL and RE can't change in one Function Set; G only changes while
//   RE stays 1), Display Clear, Return Home, Entry Mode, Display Control,
//   cursor moves, CGRAM/DDRAM/GDRAM addresses, Standby, Reverse, Scroll or
//   RAM Address Select.
// - Reading the busy flag and AC, and RAM reads with the dummy read the
//   datasheet asks for after an address is set.
// - DDRAM, CGRAM and GDRAM with random contents at power-on; the text layer
//   (half-width characters and CGRAM characters) XOR the graphics layer.
// - The datasheet's bus timing at VDD = 4.5 V: E cycle (TC), E pulse (TPW),
//   address setup (TAS), data setup (TDSW); serial clock cycle and pulse
//   widths. A violation is reported; the transfer still happens.
//
// Not emulated, reported if used: 16x16 CGROM (Chinese) characters, the
// cursor, blinking, display shift and vertical scroll. The half-width font
// isn't the HCGROM's (see st7920_font.rs).
//
// The datasheet leaves some things open; the choices made here:
// - A data byte takes 72 us like an instruction, the first byte of a
//   16-bit word too.
// - Display Control's D switches the whole display, graphics included.
// - Reading BF and AC gives the horizontal address as AC for GDRAM.
// - The second "Set GDRAM Address" byte must follow the first; any other
//   instruction starts over with the vertical address.
// - PSB and RST read high when open (the module's pull-ups). DB0..DB7
//   open during a write latch as 0 and are reported.
use crate::Display;
use crate::common::component::{ Component, Ctx };
use crate::common::net::Level;
use crate::common::time::{ Time, NS, US, MS };
use super::st7920_font;

const PINS: [&str; 13] = ["RS", "RW", "E", "DB0", "DB1", "DB2", "DB3", "DB4", "DB5", "DB6", "DB7", "PSB", "RST"];
const RS: usize = 0;
const RW: usize = 1;
const E: usize = 2;
const DB0: usize = 3;
const DB7: usize = 10;
const PSB: usize = 11;
const RST: usize = 12;

pub const DEFAULT_FOSC: u64 = 540_000;
// Execution times at 540 kHz.
const T_EXEC: Time = 72 * US;
const T_CLEAR: Time = 1600 * US;
// Power-on to the first instruction.
const T_POWER_ON: Time = 40 * MS;
// Parallel bus timing, VDD = 4.5 V.
const TC: Time = 1200 * NS;
const TPW: Time = 140 * NS;
const TAS: Time = 10 * NS;
const TDSW: Time = 40 * NS;
// Serial bus timing, VDD = 4.5 V.
const TSCYC: Time = 400 * NS;
const TSHW: Time = 200 * NS;

// Colours of a blue module: lit pixels are white.
const BACKGROUND: u32 = 0x2848b8;
const LIT: u32 = 0xe8f0ff;
pub const WIDTH: usize = 128;
pub const HEIGHT: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ram {
    Ddram,
    Cgram,
    Gdram
}

// The serial receiver: 8-bit groups after a sync of five 1 bits.
#[derive(Default)]
struct Serial {
    bits: u8,
    count: u8,
    synced: bool,
    rw: bool,
    rs: bool,
    upper: Option<u8>,
    last_rise: Option<Time>,
    last_edge: Time
}

pub struct ST7920 {
    fosc: u64,
    // Memories: 64 words of DDRAM, 4 characters of 16 rows of CGRAM, and
    // 64 rows of 16 words of GDRAM.
    ddram: [u16; 64],
    cgram: [u16; 64],
    gdram: [[u16; 16]; 64],
    // Registers.
    target: Ram,
    ac: u8,
    gd_y: u8,
    gd_x: u8,
    gd_horizontal_next: bool,
    pending: Option<u8>,
    dr: u8,
    read_word: u16,
    dummy_read: bool,
    read_low_next: bool,
    dl: bool,
    re: bool,
    g: bool,
    display_on: bool,
    cursor: bool,
    blink: bool,
    increment: bool,
    shift: bool,
    sr: bool,
    scroll: u8,
    reverse: u8,
    standby: bool,
    busy_until: Time,
    ready_at: Time,
    // Bus.
    rs: bool,
    rw: bool,
    e: bool,
    psb: bool,
    rst: bool,
    data: u8,
    floating: u8,
    rs_rw_changed: Time,
    data_changed: Time,
    e_rise: Option<Time>,
    // 4-bit mode: the first nibble of a byte was transferred.
    half: bool,
    upper: u8,
    read_byte: u8,
    driven: Option<u8>,
    serial: Serial,
    // Reports.
    reported: Vec<&'static str>,
    warnings: u32,
    dirty: bool
}

impl ST7920 {
    pub fn new(fosc: u64) -> Self {
        let mut lcd = Self {
            fosc, ddram: [0; 64], cgram: [0; 64], gdram: [[0; 16]; 64],
            target: Ram::Ddram, ac: 0, gd_y: 0, gd_x: 0, gd_horizontal_next: false, pending: None,
            dr: 0, read_word: 0, dummy_read: true, read_low_next: false,
            dl: true, re: false, g: false, display_on: false, cursor: false, blink: false,
            increment: true, shift: false, sr: false, scroll: 0, reverse: 0, standby: false,
            busy_until: 0, ready_at: T_POWER_ON,
            rs: false, rw: false, e: false, psb: true, rst: true, data: 0, floating: 0xFF,
            rs_rw_changed: 0, data_changed: 0, e_rise: None, half: false, upper: 0, read_byte: 0,
            driven: None, serial: Serial::default(), reported: vec![], warnings: 0, dirty: true
        };
        lcd.power_on();
        lcd
    }

    // Power-on: RAM holds whatever it powers up with (a fixed pseudo-random
    // pattern here), registers take their initial values.
    fn power_on(&mut self) {
        let mut seed: u32 = 0x7920_1234;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u16
        };
        for word in self.ddram.iter_mut().chain(self.cgram.iter_mut()).chain(self.gdram.iter_mut().flatten()) {
            *word = next();
        }
        self.reset_registers();
        self.ready_at = T_POWER_ON;
    }

    // Initial values (datasheet: "Initial Setting").
    fn reset_registers(&mut self) {
        self.target = Ram::Ddram;
        self.ac = 0;
        self.gd_horizontal_next = false;
        self.pending = None;
        self.dummy_read = true;
        self.dl = true;
        self.re = false;
        self.g = false;
        self.display_on = false;
        self.cursor = false;
        self.blink = false;
        self.increment = true;
        self.shift = false;
        self.sr = false;
        self.scroll = 0;
        self.reverse = 0;
        self.standby = false;
        self.busy_until = 0;
        self.half = false;
        self.serial = Serial::default();
        self.dirty = true;
    }

    // Times are for 540 kHz.
    fn exec_time(&self, t: Time) -> Time {
        t * DEFAULT_FOSC / self.fosc
    }

    fn warn(&mut self, now: Time, kind: &'static str, msg: String) {
        self.warnings += 1;
        if !self.reported.contains(&kind) {
            self.reported.push(kind);
            eprintln!("{:.3} ms: ST7920: {msg} (reported once)", now as f64 / MS as f64);
        }
    }

    // Whether the chip takes an instruction or data byte now.
    fn accepts(&mut self, now: Time) -> bool {
        if now < self.ready_at {
            self.warn(now, "early", format!("write {:.1} ms after power-on, before the 40 ms the datasheet asks for: ignored",
                                            now as f64 / MS as f64));
            false
        } else if now < self.busy_until {
            self.warn(now, "busy", format!("write while busy ({:.1} us left): ignored",
                                           (self.busy_until - now) as f64 / US as f64));
            false
        } else {
            true
        }
    }

    fn busy(&self, now: Time) -> bool {
        now < self.busy_until || now < self.ready_at
    }

    // ---- Instructions and data ----

    fn execute(&mut self, rs: bool, byte: u8, now: Time) {
        let mut time = T_EXEC;
        if rs {
            self.write_data(byte, now);
        } else {
            if self.standby {
                self.standby = false;
                self.dirty = true;
            }
            if !(self.re && byte & 0x80 != 0) {
                self.gd_horizontal_next = false;
            }
            if byte == 0x01 && !self.re {
                time = T_CLEAR;
            }
            if self.re { self.extended(byte, now) } else { self.basic(byte, now) }
        }
        self.busy_until = now + self.exec_time(time);
    }

    fn basic(&mut self, byte: u8, now: Time) {
        match byte {
            0x00 => { },
            0x01 => {
                self.ddram = [0x2020; 64];
                self.set_address(Ram::Ddram, 0);
                self.increment = true;
                self.dirty = true;
            },
            0x02 | 0x03 => self.set_address(Ram::Ddram, 0),
            0x04..=0x07 => {
                self.increment = byte & 2 != 0;
                self.shift = byte & 1 != 0;
                if self.shift {
                    self.warn(now, "shift", "display shift on write isn't emulated".into());
                }
            },
            0x08..=0x0F => {
                self.display_on = byte & 4 != 0;
                self.cursor = byte & 2 != 0;
                self.blink = byte & 1 != 0;
                if self.cursor || self.blink {
                    self.warn(now, "cursor", "the cursor and blinking aren't emulated".into());
                }
                self.dirty = true;
            },
            0x10..=0x1F => {
                if byte & 0x08 != 0 {
                    self.warn(now, "shift", "display shift isn't emulated".into());
                } else {
                    self.ac = self.next_ac(self.ac, byte & 0x04 != 0);
                }
            },
            0x20..=0x3F => self.function_set(byte, now),
            0x40..=0x7F => {
                if self.sr {
                    self.warn(now, "cgram-sr", "Set CGRAM Address with SR = 1: ignored".into());
                } else {
                    self.set_address(Ram::Cgram, byte & 0x3F);
                }
            },
            0x80..=0xFF => self.set_address(Ram::Ddram, byte & 0x3F)
        }
    }

    fn extended(&mut self, byte: u8, now: Time) {
        match byte {
            0x00 => { },
            0x01 => {
                self.standby = true;
                self.dirty = true;
            },
            0x02 | 0x03 => self.sr = byte & 1 != 0,
            0x04..=0x07 => {
                self.reverse ^= 1 << (byte & 3);
                self.dirty = true;
            },
            0x08..=0x1F => self.warn(now, "undefined", format!("undefined extended instruction {byte:#04x}: ignored")),
            0x20..=0x3F => self.function_set(byte, now),
            0x40..=0x7F => {
                if self.sr {
                    self.scroll = byte & 0x3F;
                    if self.scroll != 0 {
                        self.warn(now, "scroll", "vertical scroll isn't emulated".into());
                    }
                } else {
                    self.warn(now, "scroll-sr", "Set Scroll Address with SR = 0: ignored".into());
                }
            },
            0x80..=0xFF => {
                if self.gd_horizontal_next {
                    self.gd_x = byte & 0x0F;
                    self.set_address(Ram::Gdram, 0);
                    self.gd_horizontal_next = false;
                } else {
                    self.gd_y = byte & 0x3F;
                    self.gd_horizontal_next = true;
                }
            }
        }
    }

    fn function_set(&mut self, byte: u8, now: Time) {
        let dl = byte & 0x10 != 0;
        let re = byte & 0x04 != 0;
        if dl != self.dl && re != self.re {
            self.warn(now, "dl-re", format!("Function Set {byte:#04x} changes DL and RE at once: only DL changes"));
        } else {
            if self.re && re && self.g != (byte & 0x02 != 0) {
                self.g = byte & 0x02 != 0;
                self.dirty = true;
            }
            self.re = re;
        }
        if dl != self.dl {
            self.dl = dl;
            self.half = false;
        }
    }

    fn set_address(&mut self, ram: Ram, ac: u8) {
        self.target = ram;
        self.ac = ac;
        self.pending = None;
        self.dummy_read = true;
    }

    // The next DDRAM/CGRAM address.
    fn next_ac(&self, ac: u8, up: bool) -> u8 {
        (if up { ac.wrapping_add(1) } else { ac.wrapping_sub(1) }) & 0x3F
    }

    fn advance(&mut self) {
        match self.target {
            Ram::Gdram => self.gd_x = (self.gd_x + 1) & 0x0F,
            _ => self.ac = self.next_ac(self.ac, self.increment)
        }
    }

    fn word(&mut self) -> &mut u16 {
        match self.target {
            Ram::Ddram => &mut self.ddram[self.ac as usize],
            Ram::Cgram => &mut self.cgram[self.ac as usize],
            Ram::Gdram => &mut self.gdram[self.gd_y as usize][self.gd_x as usize]
        }
    }

    // Data comes in 16-bit words, high byte first.
    fn write_data(&mut self, byte: u8, now: Time) {
        self.dummy_read = true;
        let Some(high) = self.pending.take() else {
            self.pending = Some(byte);
            return;
        };
        let value = (high as u16) << 8 | byte as u16;
        if self.target == Ram::Ddram && (high >= 0x80 || (high == 0 && byte >= 0x10)) {
            self.warn(now, "cgrom", format!("character code {value:#06x} isn't a half-width or CGRAM character; 16x16 CGROM characters aren't emulated"));
        }
        *self.word() = value;
        self.advance();
        self.dirty = true;
    }

    // What a read cycle puts on the bus, decided when E rises.
    fn read_value(&mut self, now: Time) -> u8 {
        if !self.rs {
            let ac = if self.target == Ram::Gdram { self.gd_x } else { self.ac };
            return (self.busy(now) as u8) << 7 | ac;
        }
        if self.busy(now) {
            self.warn(now, "busy-read", "data read while busy".into());
        }
        self.dr
    }

    // A data read is over: the data register moves on. The first read
    // after an address is set (or after a write) is the dummy read.
    fn data_read_done(&mut self, now: Time) {
        if self.busy(now) {
            return;
        }
        if self.dummy_read {
            self.dummy_read = false;
            self.read_word = *self.word();
            self.dr = (self.read_word >> 8) as u8;
            self.read_low_next = true;
        } else if self.read_low_next {
            self.dr = self.read_word as u8;
            self.read_low_next = false;
        } else {
            self.advance();
            self.read_word = *self.word();
            self.dr = (self.read_word >> 8) as u8;
            self.read_low_next = true;
        }
        self.busy_until = now + self.exec_time(T_EXEC);
    }

    // ---- Parallel bus ----

    fn e_rose(&mut self, now: Time) {
        if let Some(last) = self.e_rise {
            if now - last < TC {
                self.warn(now, "tc", format!("E cycle {} ns, under TC = 1200 ns", (now - last) / NS));
            }
        }
        if now - self.rs_rw_changed < TAS {
            self.warn(now, "tas", "RS/RW changed less than TAS = 10 ns before E rose".into());
        }
        self.e_rise = Some(now);
        if self.rw {
            if self.dl || !self.half {
                self.read_byte = self.read_value(now);
            }
        }
    }

    fn e_fell(&mut self, now: Time) {
        if let Some(rise) = self.e_rise {
            if now - rise < TPW {
                self.warn(now, "tpw", format!("E pulse {} ns, under TPW = 140 ns", (now - rise) / NS));
            }
        }
        if now - self.data_changed < TDSW && !self.rw {
            self.warn(now, "tdsw", "data changed less than TDSW = 40 ns before E fell".into());
        }
        if now < self.ready_at && self.rw {
            return;
        }
        if self.rw {
            let done = self.dl || self.half;
            if !self.dl {
                self.half = !self.half;
            }
            if done && self.rs {
                self.data_read_done(now);
            }
            return;
        }
        let bits = if self.dl { 0xFF } else { 0xF0 };
        if self.floating & bits != 0 {
            self.warn(now, "floating", format!("write with open data lines (mask {:#04x}), read as 0", self.floating & bits));
        }
        if !self.accepts(now) {
            return;
        }
        if self.dl {
            self.execute(self.rs, self.data, now);
        } else if !self.half {
            self.upper = self.data & 0xF0;
            self.half = true;
        } else {
            self.half = false;
            self.execute(self.rs, self.upper | self.data >> 4, now);
        }
    }

    // Drives DB0..DB7 (DB4..DB7 in 4-bit mode) while a read is strobed.
    fn update_drive(&mut self, ctx: &mut Ctx) {
        let want = if self.psb && self.rst && self.rw && self.e {
            Some(if self.dl { self.read_byte } else if self.half { self.read_byte << 4 } else { self.read_byte & 0xF0 })
        } else {
            None
        };
        if want == self.driven {
            return;
        }
        let pins = if self.dl { 0..8 } else { 4..8 };
        for bit in 0..8 {
            let level = match want {
                Some(value) if pins.contains(&bit) => Level::from_bit(value & (1 << bit) != 0),
                _ => Level::Z
            };
            ctx.drive(DB0 + bit, level);
        }
        self.driven = want;
    }

    // ---- Serial bus ----

    fn sclk_rose(&mut self, now: Time) {
        let s = &mut self.serial;
        let mut faults = vec![];
        if let Some(last) = s.last_rise {
            if now - last < TSCYC {
                faults.push(("tscyc", format!("SCLK cycle {} ns, under 400 ns", (now - last) / NS)));
            }
        }
        if now - s.last_edge < TSHW {
            faults.push(("tslw", format!("SCLK low {} ns, under 200 ns", (now - s.last_edge) / NS)));
        }
        s.last_rise = Some(now);
        s.last_edge = now;
        s.bits = s.bits << 1 | self.rw as u8;
        s.count += 1;
        let group = if s.count == 8 { s.count = 0; Some(s.bits) } else { None };
        for (kind, msg) in faults {
            self.warn(now, kind, msg);
        }
        let Some(bits) = group else { return };
        if bits & 0xF8 == 0xF8 {
            self.serial.synced = true;
            self.serial.rw = bits & 4 != 0;
            self.serial.rs = bits & 2 != 0;
            self.serial.upper = None;
            if self.serial.rw {
                self.warn(now, "serial-read", "the serial bus has no reads: ignored".into());
            }
        } else if !self.serial.synced {
            self.warn(now, "serial-sync", "serial data before a sync: ignored".into());
        } else if !self.serial.rw {
            match self.serial.upper.take() {
                None => self.serial.upper = Some(bits & 0xF0),
                Some(upper) => {
                    if self.accepts(now) {
                        self.execute(self.serial.rs, upper | bits >> 4, now);
                    }
                }
            }
        }
    }
}

impl Display for ST7920 {
    fn size(&self) -> (usize, usize) {
        (WIDTH, HEIGHT)
    }

    // The 128x64 panel shows the top 32 rows of a 256-wide plane folded in
    // two: plane x 0..127 on rows 0..31, x 128..255 on rows 32..63. The
    // text layer has 16 characters (16 px) per DDRAM line, lines 0 and 1
    // in plane rows 0..15 and 16..31; the graphics layer is GDRAM row y,
    // word x. The two are XORed.
    fn render(&mut self, pixels: &mut [u32]) -> bool {
        if !self.dirty {
            return false;
        }
        self.dirty = false;
        let on = self.display_on && !self.standby;
        for y in 0..HEIGHT {
            let row = y % 32;
            for x in 0..WIDTH {
                let px = x + if y >= 32 { 128 } else { 0 };
                let lit = on && (self.text_pixel(px, row) ^ (self.g && self.gdram[row][px / 16] & (0x8000 >> (px % 16)) != 0));
                pixels[y * WIDTH + x] = if lit { LIT } else { BACKGROUND };
            }
        }
        true
    }
}

impl ST7920 {
    fn text_pixel(&self, px: usize, row: usize) -> bool {
        let line = row / 16;
        let code = self.ddram[line * 16 + px / 16];
        let (high, low) = ((code >> 8) as u8, code as u8);
        let r = row % 16;
        let pixel = if high == 0 && low < 0x10 {
            self.cgram[((low as usize >> 1) & 3) * 16 + r] & (0x8000 >> (px % 16)) != 0
        } else if high >= 0x80 {
            false
        } else {
            let ch = if px % 16 < 8 { high } else { low };
            (st7920_font::FIRST..0x7F).contains(&ch)
                && st7920_font::GLYPHS[(ch - st7920_font::FIRST) as usize][r] & (0x80 >> (px % 8)) != 0
        };
        pixel ^ (self.reverse & (1 << line) != 0)
    }
}

impl Component for ST7920 {

    fn init(&mut self) { }

    fn pins(&self) -> &[&'static str] {
        &PINS
    }

    fn warnings(&self) -> u32 {
        self.warnings
    }

    fn input_changed(&mut self, pin: usize, level: Level, ctx: &mut Ctx) {
        let now = ctx.now();
        match pin {
            DB0..=DB7 => {
                let bit = 1 << (pin - DB0);
                self.data = if level.is_high() { self.data | bit } else { self.data & !bit };
                self.floating = if matches!(level, Level::Z | Level::X) { self.floating | bit } else { self.floating & !bit };
                // The chip's own drive doesn't count as a data change.
                if self.driven.is_none() {
                    self.data_changed = now;
                }
                return;
            },
            PSB => {
                self.psb = level != Level::Low;
                self.serial = Serial::default();
                self.half = false;
            },
            RST => {
                let rst = level != Level::Low;
                if rst && !self.rst {
                    self.reset_registers();
                }
                self.rst = rst;
            },
            RS => {
                self.rs = level.is_high();
                self.rs_rw_changed = now;
                if !self.psb && !self.rs {
                    // CS low resets the serial receiver.
                    self.serial = Serial { last_rise: self.serial.last_rise, last_edge: self.serial.last_edge, ..Serial::default() };
                }
            },
            RW => {
                self.rw = level.is_high();
                self.rs_rw_changed = now;
            },
            E => {
                let e = level.is_high();
                if e != self.e {
                    self.e = e;
                    if !self.rst {
                        // In reset: the bus is ignored.
                    } else if !self.psb {
                        if !self.rs {
                            // CS low: SCLK is ignored.
                        } else if e {
                            self.sclk_rose(now);
                        } else {
                            if now - self.serial.last_edge < TSHW {
                                self.warn(now, "tshw", format!("SCLK high {} ns, under 200 ns", (now - self.serial.last_edge) / NS));
                            }
                            self.serial.last_edge = now;
                        }
                    } else if e {
                        self.e_rose(now);
                    } else {
                        self.e_fell(now);
                    }
                }
            },
            _ => return
        }
        self.update_drive(ctx);
    }

    fn as_display(&self) -> Option<&dyn Display> { Some(self) }
    fn as_display_mut(&mut self) -> Option<&mut dyn Display> { Some(self) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Drives the LCD's pins directly, one event at a time.
    struct Bus {
        lcd: ST7920,
        now: Time,
        out: u8
    }

    impl Bus {
        fn new() -> Self {
            let mut bus = Self { lcd: ST7920::new(DEFAULT_FOSC), now: 0, out: 0 };
            for pin in [RS, RW, E] {
                bus.set(pin, false);
            }
            bus.data(0);
            bus.now = 50 * MS;
            bus
        }

        fn set(&mut self, pin: usize, high: bool) {
            let mut ctx = Ctx::new(self.now);
            self.lcd.input_changed(pin, Level::from_bit(high), &mut ctx);
            for (pin, level, _) in ctx.take_drives() {
                let bit = 1 << (pin - DB0);
                match level {
                    Level::High => self.out |= bit,
                    Level::Low => self.out &= !bit,
                    _ => { }
                }
            }
            self.now += 250 * NS;
        }

        fn data(&mut self, value: u8) {
            for bit in 0..8 {
                self.set(DB0 + bit, value & (1 << bit) != 0);
            }
        }

        fn wait(&mut self, t: Time) {
            self.now += t;
        }

        // One write cycle; in 4-bit mode, value's upper nibble.
        fn strobe(&mut self, rs: bool, value: u8) {
            self.set(RS, rs);
            self.set(RW, false);
            self.data(value);
            self.set(E, true);
            self.set(E, false);
            self.wait(TC);
        }

        fn write(&mut self, rs: bool, value: u8) {
            self.strobe(rs, value);
            self.wait(T_EXEC);
        }

        fn write4(&mut self, rs: bool, value: u8) {
            self.strobe(rs, value);
            self.strobe(rs, value << 4);
            self.wait(T_EXEC);
        }

        fn read(&mut self, rs: bool) -> u8 {
            self.set(RS, rs);
            self.set(RW, true);
            self.set(E, true);
            let value = self.out;
            self.set(E, false);
            self.set(RW, false);
            self.wait(TC);
            value
        }

        fn read4(&mut self, rs: bool) -> u8 {
            let high = self.read(rs) & 0xF0;
            high | self.read(rs) >> 4
        }

        fn pixels(&mut self) -> Vec<bool> {
            let mut pixels = vec![0u32; WIDTH * HEIGHT];
            self.lcd.dirty = true;
            self.lcd.render(&mut pixels);
            pixels.iter().map(|&p| p == LIT).collect()
        }

        fn init8(&mut self) {
            for cmd in [0x30, 0x30, 0x0C, 0x01, 0x06, 0x34, 0x36] {
                self.write(false, cmd);
                self.wait(T_CLEAR);
            }
        }
    }

    #[test]
    fn graphics_in_8_bit_mode() {
        let mut bus = Bus::new();
        bus.init8();
        // Row 33 of the panel: GDRAM row 1, words 8.. (right half).
        bus.write(false, 0x81);
        bus.write(false, 0x88);
        bus.write(true, 0xA5);
        bus.write(true, 0x01);
        bus.write(true, 0x80);
        bus.write(true, 0x00);
        let px = bus.pixels();
        let row: String = (0..24).map(|x| if px[33 * WIDTH + x] { '#' } else { '.' }).collect();
        assert_eq!(row, "#.#..#.#.......##.......");
        assert_eq!(bus.lcd.gdram[1][8], 0xA501);
        assert_eq!(bus.lcd.warnings, 0);
    }

    #[test]
    fn writes_while_busy_or_too_early_are_ignored() {
        let mut bus = Bus::new();
        bus.now = 10 * MS;
        bus.write(false, 0x0C);
        assert!(!bus.lcd.display_on, "power-on wait");
        bus.now = 50 * MS;
        bus.write(false, 0x01);
        // Display Clear takes 1.6 ms: BF stays up and Display ON is lost.
        assert_eq!(bus.read(false) & 0x80, 0x80);
        bus.write(false, 0x0C);
        assert!(!bus.lcd.display_on);
        bus.wait(T_CLEAR);
        assert_eq!(bus.read(false), 0x00, "BF and AC after the clear");
        bus.write(false, 0x0C);
        assert!(bus.lcd.display_on);
        assert_eq!(bus.lcd.warnings, 2);
    }

    #[test]
    fn busy_flag_and_address_counter() {
        let mut bus = Bus::new();
        bus.init8();
        bus.write(false, 0x30);
        bus.strobe(false, 0x93);
        assert_eq!(bus.read(false), 0x80 | 0x13, "busy right after the instruction");
        bus.wait(T_EXEC);
        assert_eq!(bus.read(false), 0x13);
        bus.write(true, b'A');
        bus.write(true, b'B');
        assert_eq!(bus.read(false), 0x14);
        bus.write(false, 0x04);                 // decrement
        bus.write(true, b'C');
        bus.write(true, b'D');
        assert_eq!(bus.read(false), 0x13);
        assert_eq!(bus.lcd.ddram[0x14], 0x4344);
    }

    #[test]
    fn reads_need_a_dummy_read() {
        let mut bus = Bus::new();
        bus.init8();
        bus.write(false, 0x30);
        bus.write(false, 0x80);
        for byte in [0x12, 0x34, 0x56, 0x78] {
            bus.write(true, byte);
        }
        bus.write(false, 0x80);
        bus.read(true);
        bus.wait(T_EXEC);
        let mut got = vec![];
        for _ in 0..4 {
            got.push(bus.read(true));
            bus.wait(T_EXEC);
        }
        assert_eq!(got, [0x12, 0x34, 0x56, 0x78]);
    }

    // The datasheet's 4-bit sequence: the chip starts in 8-bit mode, so the
    // first nibble is a whole Function Set and the second one, sent while
    // that instruction runs, is lost.
    #[test]
    fn datasheet_4_bit_initialisation() {
        let mut bus = Bus::new();
        bus.strobe(false, 0x20);
        bus.strobe(false, 0x00);
        bus.wait(100 * US);
        assert!(!bus.lcd.dl);
        for cmd in [0x20, 0x0C, 0x01] {
            bus.write4(false, cmd);
            bus.wait(100 * US);
        }
        bus.wait(10 * MS);
        bus.write4(false, 0x06);
        assert!(bus.lcd.display_on && !bus.lcd.half);
        assert_eq!(bus.read4(false), 0x00);
        bus.write4(false, 0x24);
        bus.write4(false, 0x26);
        assert!(bus.lcd.re && bus.lcd.g && !bus.lcd.dl);
        bus.write4(false, 0x80);
        bus.write4(false, 0x80);
        bus.write4(true, 0xFF);
        bus.write4(true, 0x00);
        assert_eq!(bus.lcd.gdram[0][0], 0xFF00);
        assert_eq!(bus.lcd.warnings, 1, "the lost nibble");
    }

    // Nibbles 3, 3, 3, 2 get to aligned 4-bit mode from any state.
    #[test]
    fn nibble_resync() {
        for state in 0..3 {
            let mut bus = Bus::new();
            if state < 2 {
                bus.write(false, 0x20);         // 4-bit; state 2 stays 8-bit
            }
            if state == 1 {
                bus.strobe(false, 0x80);        // half a byte
            }
            bus.wait(T_EXEC);
            for nibble in [0x30, 0x30, 0x30, 0x20] {
                bus.strobe(false, nibble);
                bus.wait(T_CLEAR);
            }
            assert!(!bus.lcd.dl && !bus.lcd.half, "state {state}");
            bus.write4(false, 0x0C);
            assert!(bus.lcd.display_on, "state {state}");
        }
    }

    #[test]
    fn function_set_rules() {
        let mut bus = Bus::new();
        bus.init8();
        assert!(bus.lcd.g);
        bus.write(false, 0x30);
        assert!(!bus.lcd.re && bus.lcd.g, "leaving RE keeps G");
        bus.write(false, 0x36);
        assert!(bus.lcd.re && bus.lcd.g);
        bus.write(false, 0x34);
        assert!(!bus.lcd.g, "G changes while RE stays 1");
        bus.write(false, 0x30);
        bus.write(false, 0x36);
        assert!(bus.lcd.re && !bus.lcd.g, "entering RE doesn't change G");
        bus.write(false, 0x22);
        assert!(!bus.lcd.dl && bus.lcd.re, "DL and RE at once: RE stays");
        bus.wait(T_EXEC);
        bus.write4(false, 0x26);
        assert!(!bus.lcd.dl && bus.lcd.re && bus.lcd.g);
    }

    #[test]
    fn text_layer_xors_graphics() {
        let mut bus = Bus::new();
        bus.init8();
        bus.write(false, 0x34);                 // graphics off
        bus.write(false, 0x30);
        bus.write(false, 0x80);
        bus.write(true, b'H');
        bus.write(true, b' ');
        // A CGRAM character: an all-lit 16x16 cell at word 1.
        bus.write(false, 0x40);
        for _ in 0..32 {
            bus.write(true, 0xFF);
        }
        bus.write(false, 0x81);
        bus.write(true, 0x00);
        bus.write(true, 0x00);
        let px = bus.pixels();
        let glyph = st7920_font::GLYPHS[(b'H' - st7920_font::FIRST) as usize];
        for y in 0..16 {
            let row = (0..8).fold(0, |row, x| row | (px[y * WIDTH + x] as u8) << (7 - x));
            assert_eq!(row, glyph[y], "row {y} of H");
            assert!(!(8..16).any(|x| px[y * WIDTH + x]), "the space");
        }
        assert!((16..32).all(|x| px[5 * WIDTH + x]));
        // The graphics layer over the CGRAM character punches a hole.
        bus.write(false, 0x34);
        bus.write(false, 0x36);
        bus.write(false, 0x85);
        bus.write(false, 0x81);
        bus.write(true, 0x80);
        bus.write(true, 0x00);
        let px = bus.pixels();
        assert!(!px[5 * WIDTH + 16] && px[5 * WIDTH + 17]);
    }

    #[test]
    fn serial_bus_writes() {
        let mut bus = Bus::new();
        bus.set(PSB, false);
        bus.set(RS, true);                      // CS
        let mut send = |bus: &mut Bus, rs: bool, byte: u8| {
            let bits = [0xF8 | (rs as u8) << 1, byte & 0xF0, byte << 4];
            for group in bits {
                for i in (0..8).rev() {
                    bus.set(RW, group & (1 << i) != 0);
                    bus.set(E, true);
                    bus.set(E, false);
                }
            }
            bus.wait(T_CLEAR);
        };
        for cmd in [0x30, 0x0C, 0x34, 0x36, 0x80, 0x80] {
            send(&mut bus, false, cmd);
        }
        send(&mut bus, true, 0xC3);
        send(&mut bus, true, 0x3C);
        assert!(bus.lcd.display_on && bus.lcd.g);
        assert_eq!(bus.lcd.gdram[0][0], 0xC33C);
        assert_eq!(bus.lcd.warnings, 0);
    }

    #[test]
    fn bus_timing_is_checked() {
        let mut bus = Bus::new();
        bus.set(RS, false);
        bus.set(RW, false);
        bus.data(0x0C);
        let mut ctx = Ctx::new(bus.now);
        bus.lcd.input_changed(E, Level::High, &mut ctx);
        let mut ctx = Ctx::new(bus.now + 100 * NS);
        bus.lcd.input_changed(E, Level::Low, &mut ctx);
        assert!(bus.lcd.display_on, "the write still happens");
        assert_eq!(bus.lcd.reported, ["tpw"]);
    }

    #[test]
    fn reset_pin_holds_the_chip() {
        let mut bus = Bus::new();
        bus.init8();
        bus.set(RST, false);
        bus.write(false, 0x01);
        assert_eq!(bus.lcd.ddram[0], 0x2020, "cleared before the reset");
        bus.write(false, 0x08);
        assert!(bus.lcd.display_on, "ignored in reset");
        bus.set(RST, true);
        assert!(!bus.lcd.display_on && !bus.lcd.g && bus.lcd.dl && !bus.lcd.re);
    }
}
