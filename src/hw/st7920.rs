use crate::Display;
use crate::common::byte_data::ByteData;
use crate::common::component::{ Component, Ctx };
use crate::common::net::Level;

const PINS: [&str; 11] = ["RS", "RW", "E", "DB0", "DB1", "DB2", "DB3", "DB4", "DB5", "DB6", "DB7"];
const RW: usize = 1;
const E: usize = 2;
const DB0: usize = 3;

// Byte driven on the data bus during a read. The game waits for bit 7 to be
// *set* (BTFSS PORTCSTATUS, BF in LCD_Wait), the inverse of the datasheet busy
// flag, and polls with RS = 1 when writing data. Answer every read with it.
const READ_VALUE: u8 = 0x80;

// Colours of a blue ST7920 module: lit pixels are white.
const BACKGROUND: u32 = 0x2848b8;
const LIT: u32 = 0xe8f0ff;
pub const WIDTH: usize = 128;
pub const HEIGHT: usize = 64;

fn byte_to_array(byte: u8, array: &mut [u8]) {
    for i in 0..=7 {
        array[7 - i] = byte & (1 << i);
    }
}

#[derive(PartialEq, Eq, Clone, Debug)]
enum ST7920DataState {
    VerticalAddress,
    HorizontalAddress,
    DataPart1,
    DataPart2
}

impl ST7920DataState {
    fn is_data(&self) -> bool {
        *self == ST7920DataState::DataPart1 || *self == ST7920DataState::DataPart2
    }
}

struct Coordinate {
    VA: usize,
    HA: usize
}

pub struct ST7920 {
    pub screen: [[u8; 256]; 64],
    pub state: ST7920DataState,
    pub coord: Coordinate,
    // The screen changed since the last render().
    dirty: bool,
    rw: bool,
    e: bool,
    // Level of DB0..DB7, as seen on the bus.
    data: u8,
    // Whether the data bus is driven (during a read).
    reading: bool
}

impl ST7920 {
    pub fn new() -> Self {
        Self { screen: [[0u8; 256]; 64],
               state: ST7920DataState::VerticalAddress,
               coord: Coordinate { VA: 0, HA: 0 },
               dirty: true,
               rw: false,
               e: false,
               data: 0,
               reading: false }
    }

    // Decodes a byte written by the MCU (RW = 0, latched when E falls).
    // RS is ignored: commands and data are told apart by their order.
    fn write(&mut self, value: u8) {
        if value & 0x80 != 0 && !self.state.is_data() {
            if self.state == ST7920DataState::VerticalAddress {
                self.coord.VA = (value & 0x7f) as usize;
                self.state = ST7920DataState::HorizontalAddress;
            } else {
                self.coord.HA = (((value & 0x7f) as usize) * 16) as usize;
                self.state = ST7920DataState::DataPart1;
            }
        } else if self.state == ST7920DataState::DataPart1 {
            byte_to_array(value, &mut self.screen[self.coord.VA][self.coord.HA..=(self.coord.HA + 7)]);
            self.state = ST7920DataState::DataPart2;
        } else if self.state == ST7920DataState::DataPart2 {
            byte_to_array(value, &mut self.screen[self.coord.VA][(self.coord.HA + 8)..=(self.coord.HA + 15)]);
            self.state = ST7920DataState::VerticalAddress;
            self.dirty = true;
        }
    }
}

// GDRAM is 32 rows of 256 pixels: the left half of each row is the top
// half of the screen, the right half the bottom.
impl Display for ST7920 {
    fn size(&self) -> (usize, usize) {
        (WIDTH, HEIGHT)
    }

    fn render(&mut self, pixels: &mut [u32]) -> bool {
        if !self.dirty {
            return false;
        }
        for y in 0..HEIGHT {
            let (row, offset) = if y < HEIGHT / 2 { (y, 0) } else { (y - HEIGHT / 2, WIDTH) };
            for x in 0..WIDTH {
                pixels[y * WIDTH + x] = if self.screen[row][offset + x] != 0 { LIT } else { BACKGROUND };
            }
        }
        self.dirty = false;
        true
    }
}

impl Component for ST7920 {

    fn init(&mut self) { }

    fn pins(&self) -> &[&'static str] {
        &PINS
    }

    fn input_changed(&mut self, pin: usize, level: Level, ctx: &mut Ctx) {
        if pin >= DB0 {
            let bit = 1 << (pin - DB0);
            self.data = if level.is_high() { self.data | bit } else { self.data & !bit };
            return;
        }

        let e = self.e;
        match pin {
            RW => self.rw = level.is_high(),
            E => self.e = level.is_high(),
            _ => return
        }
        if e && !self.e && !self.rw {
            self.write(self.data);
        }

        // The data bus is driven only while a read is strobed (RW = 1, E = 1).
        let reading = self.rw && self.e;
        if reading != self.reading {
            self.reading = reading;
            for bit in 0..8 {
                let level = if reading { Level::from_bit(READ_VALUE & (1 << bit) != 0) } else { Level::Z };
                ctx.drive(DB0 + bit, level);
            }
        }
    }

    fn as_display(&self) -> Option<&dyn Display> { Some(self) }
    fn as_display_mut(&mut self) -> Option<&mut dyn Display> { Some(self) }
}
