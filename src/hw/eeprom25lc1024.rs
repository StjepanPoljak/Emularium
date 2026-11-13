use crate::common::component::{ Component, Ctx };
use crate::common::net::Level;
use crate::common::time::{ Time, NS, US, MS };

const PINS: [&str; 6] = ["CS", "SO", "SI", "SCK", "HOLD", "WP"];
const CS: usize = 0;
const SO: usize = 1;
const SI: usize = 2;
const SCK: usize = 3;
const HOLD: usize = 4;
const WP: usize = 5;

pub const SIZE: usize = 128 * 1024;
// 24-bit addresses; the upper 7 bits are ignored.
const ADDR_MASK: u32 = SIZE as u32 - 1;
const PAGE_SIZE: u32 = 256;
const SECTOR_SIZE: u32 = 32 * 1024;
const ERASED: u8 = 0xff;

const READ: u8 = 0x03;
const WRITE: u8 = 0x02;
const WREN: u8 = 0x06;
const WRDI: u8 = 0x04;
const RDSR: u8 = 0x05;
const WRSR: u8 = 0x01;
const PE: u8 = 0x42;
const SE: u8 = 0xd8;
const CE: u8 = 0xc7;
const RDID: u8 = 0xab;
const DPD: u8 = 0xb9;

// Electronic signature, read with RDID.
pub const SIGNATURE: u8 = 0x29;

// Status register bits. WIP and WEL are read-only; WRSR sets the rest.
const WIP: u8 = 0x01;
const WEL: u8 = 0x02;
const BP: u8 = 0x0c;
const WPEN: u8 = 0x80;

// Timing from the datasheet (DS22064C, AC characteristics, 4.5-5.5 V):
// self-timed cycles for a write (byte or page, and WRSR), page erase,
// sector erase and chip erase; CS high to standby after RDID; output valid
// after SCK falls, output disable after CS rises, HOLD low to SO high-Z
// and HOLD high to output valid.
const T_WC: Time = 6 * MS;
const T_PE: Time = 6 * MS;
const T_SE: Time = 10 * MS;
const T_CE: Time = 10 * MS;
const T_REL: Time = 100 * US;
const T_V: Time = 25 * NS;
const T_DIS: Time = 25 * NS;
const T_HZ: Time = 15 * NS;
const T_HV: Time = 15 * NS;

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Phase {
    Command,
    // Address bytes still to come, then the rest of the command.
    Address(u8),
    ReadData,
    WriteData,
    ReadStatus,
    WriteStatus,
    Signature,
    // The command is complete and takes effect when CS rises; any more
    // bits cancel it.
    Armed,
    // Until CS rises: after an ignored or unknown command.
    Ignore
}

// What happens when CS rises at the end of an armed command.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Action {
    Wren,
    Wrdi,
    WriteStatus(u8),
    PageErase(u32),
    SectorErase(u32),
    ChipErase,
    DeepPowerDown
}

// Microchip 25LC1024: 128K x 8 SPI EEPROM, SPI mode 0 or 3, erased to
// 0xff. Like the 23LC1024, SI is sampled on the rising edge of SCK and SO
// changes after the falling edge; CS, HOLD and WP only act when driven low.
// WRITE, WRSR and the erases need WREN first, and start when CS rises
// after a whole number of bytes; the chip is then busy for the cycle time,
// answering only RDSR. A page write wraps within its 256-byte page. After
// DPD, only RDID is answered; CS rising after its opcode releases the chip,
// which is back in standby T_REL later. A refused write or erase (protected
// block, WP) leaves WEL set: the datasheet resets it only on success.
pub struct EEPROM25LC1024 {
    mem: Vec<u8>,
    // WPEN and BP1:BP0, as set with WRSR.
    status: u8,
    wel: bool,
    busy: bool,
    asleep: bool,
    // Commands are ignored until then, after a release from DPD.
    standby_at: Time,
    now: Time,
    wp_low: bool,
    selected: bool,
    held: bool,
    sck: bool,
    si: bool,
    phase: Phase,
    cmd: u8,
    addr: u32,
    action: Option<Action>,
    // Bytes of a WRITE, as (address, byte), written when CS rises.
    page: Vec<(u32, u8)>,
    shift_in: u8,
    bits_in: u8,
    shift_out: u8,
    bits_out: u8,
    // Level SO drives while selected and not held.
    so: Level
}

impl EEPROM25LC1024 {
    pub fn new() -> Self {
        Self { mem: vec![ERASED; SIZE],
               status: 0,
               wel: false,
               busy: false,
               asleep: false,
               standby_at: 0,
               now: 0,
               wp_low: false,
               selected: false,
               held: false,
               sck: false,
               si: false,
               phase: Phase::Command,
               cmd: 0,
               addr: 0,
               action: None,
               page: vec![],
               shift_in: 0,
               bits_in: 0,
               shift_out: 0,
               bits_out: 0,
               so: Level::Z }
    }

    // Programs the start of the array with data, as a programmer would
    // before the chip goes on the board. Bytes past the array are dropped.
    pub fn load(&mut self, data: &[u8]) {
        let len = data.len().min(SIZE);
        self.mem[..len].copy_from_slice(&data[..len]);
    }

    #[allow(unused)]
    pub fn mem(&self) -> &[u8] {
        &self.mem
    }

    fn status_register(&self) -> u8 {
        self.status | if self.wel { WEL } else { 0 } | if self.busy { WIP } else { 0 }
    }

    // Whether BP1:BP0 protect addr: none, the upper quarter, the upper
    // half or the whole array.
    fn protected(&self, addr: u32) -> bool {
        match (self.status & BP) >> 2 {
            0 => false,
            1 => addr >= 0x18000,
            2 => addr >= 0x10000,
            _ => true
        }
    }

    fn set_so(&mut self, level: Level, delay: Time, ctx: &mut Ctx) {
        self.so = level;
        ctx.drive_after(SO, level, delay);
    }

    fn arm(&mut self, action: Action) {
        self.action = Some(action);
        self.phase = Phase::Armed;
    }

    fn command(&mut self, cmd: u8) {
        self.cmd = cmd;
        // Asleep, the chip only wakes for RDID; busy, it only answers RDSR.
        let releasing = self.now < self.standby_at;
        if (self.asleep && cmd != RDID) || (self.busy && cmd != RDSR) || releasing {
            if releasing {
                eprintln!("25LC1024: command {cmd:#04x} within T_REL of leaving deep power-down, ignored");
            }
            self.phase = Phase::Ignore;
            return;
        }
        let needs_wel = matches!(cmd, WRITE | WRSR | PE | SE | CE);
        if needs_wel && !self.wel {
            eprintln!("25LC1024: command {cmd:#04x} without WREN, ignored");
            self.phase = Phase::Ignore;
            return;
        }
        match cmd {
            READ | WRITE | PE | SE | RDID => self.phase = Phase::Address(3),
            RDSR => self.phase = Phase::ReadStatus,
            WRSR => self.phase = Phase::WriteStatus,
            WREN => self.arm(Action::Wren),
            WRDI => self.arm(Action::Wrdi),
            CE => self.arm(Action::ChipErase),
            DPD => self.arm(Action::DeepPowerDown),
            _ => {
                eprintln!("25LC1024: unknown command {cmd:#04x}, ignored");
                self.phase = Phase::Ignore;
            }
        }
    }

    fn byte_in(&mut self, byte: u8) {
        match self.phase {
            Phase::Command => self.command(byte),
            Phase::Address(left) => {
                self.addr = ((self.addr << 8) | byte as u32) & ADDR_MASK;
                if left > 1 {
                    self.phase = Phase::Address(left - 1);
                    return;
                }
                match self.cmd {
                    READ => self.phase = Phase::ReadData,
                    WRITE => {
                        self.page.clear();
                        self.phase = Phase::WriteData;
                    },
                    PE => self.arm(Action::PageErase(self.addr)),
                    SE => self.arm(Action::SectorErase(self.addr)),
                    _ => self.phase = Phase::Signature
                }
            },
            Phase::WriteData => {
                self.page.push((self.addr, byte));
                let page_start = self.addr & !(PAGE_SIZE - 1);
                self.addr = page_start | ((self.addr + 1) & (PAGE_SIZE - 1));
            },
            Phase::WriteStatus => self.arm(Action::WriteStatus(byte)),
            Phase::Armed => {
                self.action = None;
                self.phase = Phase::Ignore;
            },
            Phase::ReadData | Phase::ReadStatus | Phase::Signature | Phase::Ignore => { }
        }
    }

    // Next bit on SO, after a falling edge of SCK.
    fn bit_out(&mut self, ctx: &mut Ctx) {
        if self.bits_out == 0 {
            self.shift_out = match self.phase {
                Phase::ReadData => {
                    let byte = self.mem[self.addr as usize];
                    self.addr = (self.addr + 1) & ADDR_MASK;
                    byte
                },
                Phase::ReadStatus => self.status_register(),
                Phase::Signature => SIGNATURE,
                _ => return
            };
            self.bits_out = 8;
        }
        let level = Level::from_bit(self.shift_out & 0x80 != 0);
        self.shift_out <<= 1;
        self.bits_out -= 1;
        self.set_so(level, T_V, ctx);
    }

    fn start_cycle(&mut self, time: Time, ctx: &mut Ctx) {
        self.busy = true;
        ctx.wake_after(time);
    }

    fn fill(&mut self, start: u32, len: u32) {
        self.mem[start as usize..(start + len) as usize].fill(ERASED);
    }

    // CS rose: a write or an armed command takes effect, if CS rose after
    // a whole number of bytes.
    fn deselected(&mut self, ctx: &mut Ctx) {
        let action = self.action.take();
        let whole_bytes = self.bits_in == 0;
        // RDID releases the chip once its opcode is in, signature read or not.
        if self.asleep && self.cmd == RDID && self.phase != Phase::Command && self.phase != Phase::Ignore {
            self.asleep = false;
            self.standby_at = self.now + T_REL;
        }
        if !whole_bytes {
            return;
        }
        if self.phase == Phase::WriteData && !self.page.is_empty() {
            if self.protected(self.page[0].0) {
                eprintln!("25LC1024: write to protected address {:#07x}, ignored", self.page[0].0);
                return;
            }
            for (addr, byte) in std::mem::take(&mut self.page) {
                self.mem[addr as usize] = byte;
            }
            self.start_cycle(T_WC, ctx);
            return;
        }
        let Some(action) = action else { return };
        match action {
            Action::Wren => self.wel = true,
            Action::Wrdi => self.wel = false,
            Action::DeepPowerDown => self.asleep = true,
            Action::WriteStatus(byte) => {
                if self.status & WPEN != 0 && self.wp_low {
                    eprintln!("25LC1024: WRSR with WPEN set and WP low, ignored");
                    return;
                }
                self.status = byte & (WPEN | BP);
                self.start_cycle(T_WC, ctx);
            },
            Action::PageErase(addr) | Action::SectorErase(addr) => {
                if self.protected(addr) {
                    eprintln!("25LC1024: erase of protected address {addr:#07x}, ignored");
                    return;
                }
                let (size, time) = if matches!(action, Action::PageErase(_)) { (PAGE_SIZE, T_PE) } else { (SECTOR_SIZE, T_SE) };
                self.fill(addr & !(size - 1), size);
                self.start_cycle(time, ctx);
            },
            Action::ChipErase => {
                if self.status & BP != 0 {
                    eprintln!("25LC1024: chip erase with block protection on, ignored");
                    return;
                }
                self.fill(0, SIZE as u32);
                self.start_cycle(T_CE, ctx);
            }
        }
    }
}

impl Component for EEPROM25LC1024 {
    fn init(&mut self) { }

    fn memory(&self) -> Option<&[u8]> {
        Some(&self.mem)
    }

    fn pins(&self) -> &[&'static str] {
        &PINS
    }

    // The write or erase cycle is over.
    fn wake(&mut self, _ctx: &mut Ctx) {
        self.busy = false;
        self.wel = false;
    }

    fn input_changed(&mut self, pin: usize, level: Level, ctx: &mut Ctx) {
        self.now = ctx.now();
        match pin {
            CS => {
                let selected = level == Level::Low;
                if selected == self.selected {
                    return;
                }
                self.selected = selected;
                if selected {
                    self.phase = Phase::Command;
                    self.action = None;
                    (self.bits_in, self.bits_out) = (0, 0);
                } else {
                    if self.so != Level::Z {
                        self.set_so(Level::Z, T_DIS, ctx);
                    }
                    self.deselected(ctx);
                }
            },
            SI => self.si = level.is_high(),
            SCK => {
                let sck = level.is_high();
                if sck == self.sck {
                    return;
                }
                self.sck = sck;
                if !self.selected || self.held {
                    return;
                }
                if sck {
                    self.shift_in = (self.shift_in << 1) | self.si as u8;
                    self.bits_in = (self.bits_in + 1) % 8;
                    if self.bits_in == 0 {
                        self.byte_in(self.shift_in);
                    }
                } else if matches!(self.phase, Phase::ReadData | Phase::ReadStatus | Phase::Signature) {
                    self.bit_out(ctx);
                }
            },
            HOLD => {
                let held = level == Level::Low;
                if held == self.held {
                    return;
                }
                self.held = held;
                if self.selected && self.so != Level::Z {
                    let (level, delay) = if held { (Level::Z, T_HZ) } else { (self.so, T_HV) };
                    ctx.drive_after(SO, level, delay);
                }
            },
            WP => self.wp_low = level == Level::Low,
            _ => { }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::spi_master;

    type Master = spi_master::Master<EEPROM25LC1024>;

    fn master() -> Master {
        Master::new(EEPROM25LC1024::new())
    }

    fn status(m: &mut Master) -> u8 {
        m.transaction(&[RDSR, 0])[1]
    }

    // WREN, then the write, then waits for the write cycle to end.
    fn write_enabled(m: &mut Master, addr: u32, data: &[u8]) {
        m.transaction(&[WREN]);
        m.write(addr, data);
        m.wait(T_WC);
    }

    #[test]
    fn erased_or_loaded() {
        let mut m = master();
        assert_eq!(m.read(0, 2), vec![0xff, 0xff]);
        m.chip.load(&[1, 2, 3]);
        assert_eq!(m.read(0, 4), vec![1, 2, 3, 0xff]);
        // Reads wrap at the end of the array; the upper address bits are ignored.
        assert_eq!(m.read(0xffffff, 2), vec![0xff, 1]);
        assert_eq!(m.so_level(), Level::Z);
    }

    #[test]
    fn write_needs_wren_and_takes_a_write_cycle() {
        let mut m = master();
        m.write(0x10, &[7]);
        assert_eq!(m.read(0x10, 1), vec![0xff], "written without WREN");
        assert_eq!(status(&mut m), 0);

        m.transaction(&[WREN]);
        assert_eq!(status(&mut m), WEL);
        m.write(0x10, &[7, 8]);
        assert_eq!(status(&mut m), WEL | WIP);
        // Busy: only RDSR answers.
        assert_eq!(m.read(0x10, 1), vec![0]);
        m.transaction(&[WREN]);
        m.wait(T_WC);
        assert_eq!(status(&mut m), 0, "WEL clears after the write cycle");
        assert_eq!(m.read(0x10, 2), vec![7, 8]);
    }

    #[test]
    fn rdsr_can_be_read_repeatedly() {
        let mut m = master();
        m.transaction(&[WREN]);
        m.write(0, &[1]);
        assert_eq!(m.transaction(&[RDSR, 0, 0]), vec![0, WEL | WIP, WEL | WIP]);
    }

    #[test]
    fn page_write_wraps_within_page() {
        let mut m = master();
        write_enabled(&mut m, 0x1fe, &[1, 2, 3, 4]);
        assert_eq!(m.chip.mem()[0x1fe..0x200], [1, 2]);
        assert_eq!(m.chip.mem()[0x100..0x102], [3, 4]);
        assert_eq!(m.chip.mem()[0x200], 0xff);
    }

    #[test]
    fn cs_must_rise_after_whole_bytes() {
        let mut m = master();
        m.transaction(&[WREN]);
        m.set(spi_master::CS, Level::Low);
        for byte in [WRITE, 0, 0, 0, 0x55] {
            m.xfer(byte);
        }
        m.bits(0, 3);
        m.set(spi_master::CS, Level::High);
        assert_eq!(status(&mut m), WEL, "no write cycle");
        assert_eq!(m.read(0, 1), vec![0xff]);
        // Nor does WREN take effect with a stray bit.
        m.transaction(&[WRDI]);
        m.set(spi_master::CS, Level::Low);
        m.xfer(WREN);
        m.bits(0, 1);
        m.set(spi_master::CS, Level::High);
        assert_eq!(status(&mut m), 0);
    }

    #[test]
    fn block_protection() {
        let mut m = master();
        m.transaction(&[WREN]);
        m.transaction(&[WRSR, 0x04]);
        m.wait(T_WC);
        assert_eq!(status(&mut m), 0x04);
        // Upper quarter protected.
        write_enabled(&mut m, 0x18000, &[1]);
        write_enabled(&mut m, 0x17fff, &[2]);
        assert_eq!(m.read(0x17fff, 2), vec![2, 0xff]);
        m.transaction(&[WREN]);
        m.transaction(&[CE]);
        assert_eq!(status(&mut m), WEL | 0x04, "no chip erase while protected; WEL stays set");
        assert_eq!(m.read(0x17fff, 1), vec![2]);
    }

    #[test]
    fn wpen_and_wp_lock_the_status_register() {
        let mut m = master();
        m.transaction(&[WREN]);
        m.transaction(&[WRSR, WPEN]);
        m.wait(T_WC);
        m.set(WP, Level::Low);
        m.transaction(&[WREN]);
        m.transaction(&[WRSR, 0x0c]);
        assert_eq!(status(&mut m), WEL | WPEN);
        m.set(WP, Level::High);
        m.transaction(&[WREN]);
        m.transaction(&[WRSR, WPEN | 0x0c]);
        m.wait(T_WC);
        assert_eq!(status(&mut m), WPEN | 0x0c);
    }

    #[test]
    fn erases() {
        let mut m = master();
        m.chip.load(&vec![0; SIZE]);
        m.transaction(&[WREN]);
        m.transaction(&[PE, 0x00, 0x01, 0x23]);
        m.wait(T_PE);
        assert_eq!(m.read(0xff, 3), vec![0, 0xff, 0xff]);
        assert_eq!(m.read(0x1ff, 2), vec![0xff, 0]);

        m.transaction(&[WREN]);
        m.transaction(&[SE, 0x00, 0x90, 0x00]);
        assert_eq!(status(&mut m), WEL | WIP);
        m.wait(T_SE);
        assert_eq!(m.read(0x7fff, 2), vec![0, 0xff]);
        assert_eq!(m.read(0xffff, 2), vec![0xff, 0]);

        m.transaction(&[WREN]);
        m.transaction(&[CE]);
        m.wait(T_CE);
        assert!(m.chip.mem().iter().all(|byte| *byte == 0xff));
    }

    #[test]
    fn deep_power_down_until_rdid() {
        let mut m = master();
        m.chip.load(&[0x5a]);
        m.transaction(&[DPD]);
        assert_eq!(m.read(0, 1), vec![0], "asleep");
        assert_eq!(m.transaction(&[RDID, 0, 0, 0, 0, 0]), vec![0, 0, 0, 0, SIGNATURE, SIGNATURE]);
        assert_eq!(m.read(0, 1), vec![0], "within T_REL");
        m.wait(T_REL);
        assert_eq!(m.read(0, 1), vec![0x5a]);
    }

    #[test]
    fn rdid_opcode_alone_releases_deep_power_down() {
        let mut m = master();
        m.chip.load(&[0x5a]);
        m.transaction(&[DPD]);
        m.transaction(&[RDID]);
        m.wait(T_REL);
        assert_eq!(m.read(0, 1), vec![0x5a]);
        // Awake, RDID just reads the signature.
        assert_eq!(m.transaction(&[RDID, 0, 0, 0, 0]), vec![0, 0, 0, 0, SIGNATURE]);
        assert_eq!(m.read(0, 1), vec![0x5a]);
    }

    #[test]
    fn dpd_needs_cs_high_after_its_opcode() {
        let mut m = master();
        m.chip.load(&[0x5a]);
        m.transaction(&[DPD, 0]);
        assert_eq!(m.read(0, 1), vec![0x5a], "not asleep");
    }
}
