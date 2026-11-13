// A test SPI master for the SPI memory models: drives a chip's pins
// directly, without a board, keeping track of the level on SO and running
// the wakeups the chip asks for (an EEPROM's write cycle). The chip's
// first pins must be CS, SO, SI, SCK and HOLD, in that order.
use crate::common::component::{ Component, Ctx };
use crate::common::net::Level;
use crate::common::time::{ Time, NS };

pub const CS: usize = 0;
pub const SO: usize = 1;
pub const SI: usize = 2;
pub const SCK: usize = 3;

pub const HALF_PERIOD: Time = 50 * NS;

const READ: u8 = 0x03;
const WRITE: u8 = 0x02;

pub struct Master<C: Component> {
    pub chip: C,
    pub now: Time,
    // Drives of SO, as (time, level), oldest first.
    pub so: Vec<(Time, Level)>,
    wakeups: Vec<Time>
}

impl<C: Component> Master<C> {
    // Deselects the chip, with SCK low (mode 0).
    pub fn new(chip: C) -> Self {
        let mut master = Self { chip, now: 0, so: vec![], wakeups: vec![] };
        master.set(CS, Level::High);
        master.set(SCK, Level::Low);
        master
    }

    fn take(&mut self, mut ctx: Ctx) {
        self.so.extend(ctx.take_drives().into_iter().map(|(pin, level, time)| {
            assert_eq!(pin, SO);
            (time, level)
        }));
        self.wakeups.extend(ctx.take_wakeups());
        self.wakeups.sort();
    }

    pub fn set(&mut self, pin: usize, level: Level) {
        let mut ctx = Ctx::new(self.now);
        self.chip.input_changed(pin, level, &mut ctx);
        self.take(ctx);
    }

    // Lets time pass, calling the chip's wake() for every wakeup due.
    pub fn wait(&mut self, time: Time) {
        let end = self.now + time;
        while let Some(&due) = self.wakeups.first().filter(|due| **due <= end) {
            self.wakeups.remove(0);
            self.now = self.now.max(due);
            let mut ctx = Ctx::new(self.now);
            self.chip.wake(&mut ctx);
            self.take(ctx);
        }
        self.now = end;
    }

    pub fn so_level(&self) -> Level {
        self.so.iter().filter(|(time, _)| *time <= self.now).last().map_or(Level::Z, |(_, level)| *level)
    }

    // Clocks out the top bits of byte, MSB first; returns the bits read.
    pub fn bits(&mut self, byte: u8, count: u32) -> u8 {
        let mut read = 0;
        for bit in (8 - count..8).rev() {
            self.set(SI, Level::from_bit(byte & (1 << bit) != 0));
            self.set(SCK, Level::High);
            read = (read << 1) | self.so_level().is_high() as u8;
            self.wait(HALF_PERIOD);
            self.set(SCK, Level::Low);
            self.wait(HALF_PERIOD);
        }
        read
    }

    pub fn xfer(&mut self, byte: u8) -> u8 {
        self.bits(byte, 8)
    }

    // Runs one transaction; returns the bytes read during it.
    pub fn transaction(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.set(CS, Level::Low);
        self.wait(HALF_PERIOD);
        let read = bytes.iter().map(|byte| self.xfer(*byte)).collect();
        self.set(CS, Level::High);
        self.wait(HALF_PERIOD);
        read
    }

    pub fn write(&mut self, addr: u32, data: &[u8]) {
        let mut bytes = vec![WRITE, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8];
        bytes.extend_from_slice(data);
        self.transaction(&bytes);
    }

    pub fn read(&mut self, addr: u32, len: usize) -> Vec<u8> {
        let mut bytes = vec![READ, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8];
        bytes.resize(4 + len, 0);
        self.transaction(&bytes)[4..].to_vec()
    }
}
