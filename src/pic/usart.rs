// The PIC16F87x USART in asynchronous mode (datasheet DS30292D, section
// 10): TX on RC6, RX on RC7, 8 or 9 data bits, the baud rate generator
// (SPBRG, TXSTA.BRGH), the transmit buffer and shift register, and the
// two-byte receive FIFO with framing and overrun errors. Synchronous mode
// (TXSTA.SYNC) isn't emulated. The flags it owns (PIR1.TXIF and RCIF,
// TXSTA.TRMT, RCSTA.FERR, OERR and RX9D) are worked out on every read.
use std::collections::VecDeque;

// TXSTA bits
pub const TX9: u8 = 0x40;
pub const TXEN: u8 = 0x20;
pub const BRGH: u8 = 0x04;
pub const TRMT: u8 = 0x02;
pub const TX9D: u8 = 0x01;
// RCSTA bits
pub const SPEN: u8 = 0x80;
pub const RX9: u8 = 0x40;
pub const CREN: u8 = 0x10;
pub const FERR: u8 = 0x04;
pub const OERR: u8 = 0x02;
pub const RX9D: u8 = 0x01;
// PIR1 bits
pub const RCIF: u8 = 0x20;
pub const TXIF: u8 = 0x10;

// A received byte: data, the ninth bit, and whether its stop bit was 0.
#[derive(Clone, Copy)]
struct RxByte {
    data: u8,
    ninth: bool,
    ferr: bool
}

enum Rx {
    Idle,
    // Counting down to the middle of the start bit.
    Start(u32),
    // Bits received so far (data bits, LSB first), cycles to the next
    // sample.
    Bits { count: u8, shift: u16, wait: u32 }
}

pub struct Usart {
    txreg: Option<u16>,
    // Bits still to send (start, data, stop), LSB first, and how many.
    tsr: Option<(u16, u8)>,
    tx_wait: u32,
    tx_level: bool,
    rx: Rx,
    fifo: VecDeque<RxByte>,
    oerr: bool,
    // TX pin changes not yet driven: (cycle of the last tick(), level).
    pub tx_events: Vec<(u32, bool)>
}

impl Usart {
    pub fn new() -> Self {
        Self { txreg: None, tsr: None, tx_wait: 0, tx_level: true, rx: Rx::Idle,
               fifo: VecDeque::new(), oerr: false, tx_events: vec![] }
    }

    // Bit time in instruction cycles (Fosc / 4): Fosc / (16 (SPBRG + 1))
    // with BRGH, Fosc / (64 (SPBRG + 1)) without.
    fn bit_cycles(txsta: u8, spbrg: u8) -> u32 {
        let per = if txsta & BRGH != 0 { 4 } else { 16 };
        per * (spbrg as u32 + 1)
    }

    // Whether the USART drives RC6 (SPEN and TXEN), and its level.
    pub fn tx_pin(&self, txsta: u8, rcsta: u8) -> Option<bool> {
        (rcsta & SPEN != 0 && txsta & TXEN != 0).then_some(self.tx_level)
    }

    pub fn pir1_flags(&self, txsta: u8) -> u8 {
        let mut flags = 0;
        if !self.fifo.is_empty() {
            flags |= RCIF;
        }
        if self.txreg.is_none() && txsta & TXEN != 0 {
            flags |= TXIF;
        }
        flags
    }

    pub fn txsta_flags(&self) -> u8 {
        if self.tsr.is_none() { TRMT } else { 0 }
    }

    pub fn rcsta_flags(&self) -> u8 {
        let mut flags = 0;
        if self.oerr {
            flags |= OERR;
        }
        if let Some(byte) = self.fifo.front() {
            if byte.ferr {
                flags |= FERR;
            }
            if byte.ninth {
                flags |= RX9D;
            }
        }
        flags
    }

    // RCREG: the oldest received byte; reading it with pop takes it out
    // of the FIFO.
    pub fn rcreg(&self) -> u8 {
        self.fifo.front().map_or(0, |byte| byte.data)
    }

    pub fn pop(&mut self) {
        self.fifo.pop_front();
    }

    pub fn write_txreg(&mut self, val: u8, txsta: u8) {
        self.txreg = Some(val as u16 | if txsta & TX9D != 0 { 0x100 } else { 0 });
    }

    // After a write to TXSTA or RCSTA: clearing TXEN or SPEN resets the
    // transmitter, clearing CREN or SPEN the receiver and OERR.
    pub fn control_written(&mut self, txsta: u8, rcsta: u8) {
        if rcsta & SPEN == 0 || txsta & TXEN == 0 {
            self.tsr = None;
            self.tx_level = true;
        }
        if rcsta & SPEN == 0 {
            self.txreg = None;
            self.fifo.clear();
        }
        if rcsta & SPEN == 0 || rcsta & CREN == 0 {
            self.rx = Rx::Idle;
            self.oerr = false;
        }
        if let Some(level) = self.tx_pin(txsta, rcsta) {
            // RC6 is the USART's (again): it drives the idle level.
            self.tx_events.push((0, level));
        }
    }

    // Runs the USART for cycles instruction cycles; rx is the level on RC7.
    pub fn tick(&mut self, cycles: u32, txsta: u8, rcsta: u8, spbrg: u8, rx: bool) {
        if rcsta & SPEN == 0 {
            return;
        }
        let bit = Self::bit_cycles(txsta, spbrg);
        for cycle in 0..cycles {
            if txsta & TXEN != 0 {
                self.tick_tx(cycle, bit, txsta);
            }
            if rcsta & CREN != 0 && !self.oerr {
                self.tick_rx(bit, rcsta, rx);
            }
        }
    }

    fn set_tx(&mut self, cycle: u32, level: bool) {
        if level != self.tx_level {
            self.tx_level = level;
            self.tx_events.push((cycle, level));
        }
    }

    fn tick_tx(&mut self, cycle: u32, bit: u32, txsta: u8) {
        if self.tsr.is_none() {
            // TXREG moves to the shift register as soon as it's empty.
            let Some(data) = self.txreg.take() else { return };
            let nine = txsta & TX9 != 0;
            let data_bits = if nine { 9 } else { 8 };
            let data = data & ((1 << data_bits) - 1);
            // start bit 0, data, stop bit 1
            let frame = (data << 1) | (1 << (data_bits + 1));
            self.tsr = Some((frame, data_bits + 2));
            self.tx_wait = 0;
        }
        if self.tx_wait > 0 {
            self.tx_wait -= 1;
            return;
        }
        let (frame, left) = self.tsr.unwrap();
        if left == 0 {
            self.tsr = None;
            // The next byte, if any, starts right after the stop bit.
            self.tick_tx(cycle, bit, txsta);
            return;
        }
        self.set_tx(cycle, frame & 1 != 0);
        self.tsr = Some((frame >> 1, left - 1));
        self.tx_wait = bit - 1;
    }

    fn tick_rx(&mut self, bit: u32, rcsta: u8, rx: bool) {
        let data_bits = if rcsta & RX9 != 0 { 9 } else { 8 };
        self.rx = match self.rx {
            Rx::Idle if !rx => Rx::Start(bit / 2),
            Rx::Idle => Rx::Idle,
            Rx::Start(0) if rx => Rx::Idle,      // a glitch, not a start bit
            Rx::Start(0) => Rx::Bits { count: 0, shift: 0, wait: bit - 1 },
            Rx::Start(n) => Rx::Start(n - 1),
            Rx::Bits { count, shift, wait } if wait > 0 => Rx::Bits { count, shift, wait: wait - 1 },
            Rx::Bits { count, shift, .. } if count < data_bits => Rx::Bits {
                count: count + 1,
                shift: shift | ((rx as u16) << count),
                wait: bit - 1
            },
            Rx::Bits { shift, .. } => {
                // The stop bit.
                let byte = RxByte { data: shift as u8, ninth: shift & 0x100 != 0, ferr: !rx };
                if self.fifo.len() < 2 {
                    self.fifo.push_back(byte);
                } else {
                    self.oerr = true;
                }
                Rx::Idle
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: u8 = SPEN | CREN;

    // TX levels per cycle while sending, with a bit time of 4 cycles
    // (SPBRG 0, BRGH).
    fn send(usart: &mut Usart, txsta: u8, cycles: u32) -> Vec<bool> {
        let mut level = usart.tx_level;
        let mut levels = vec![];
        for _ in 0..cycles {
            usart.tick(1, txsta, ON, 0, true);
            for (_, l) in usart.tx_events.drain(..) {
                level = l;
            }
            levels.push(level);
        }
        levels
    }

    fn bits(levels: &[bool]) -> Vec<bool> {
        levels.chunks(4).map(|bit| {
            assert!(bit.iter().all(|&l| l == bit[0]), "a bit changed within its time: {levels:?}");
            bit[0]
        }).collect()
    }

    #[test]
    fn sends_frames_back_to_back() {
        let mut usart = Usart::new();
        let txsta = TXEN | BRGH;
        assert_eq!(usart.pir1_flags(txsta), TXIF);
        usart.write_txreg(0x35, txsta);
        assert_eq!(usart.pir1_flags(txsta), 0);
        let first = send(&mut usart, txsta, 1);
        // TXREG went to the shift register: room for the next byte.
        assert_eq!(usart.pir1_flags(txsta), TXIF);
        assert_eq!(usart.txsta_flags(), 0);
        usart.write_txreg(0xc0, txsta);
        let levels = [first, send(&mut usart, txsta, 79)].concat();
        let l = |byte: u8| (0..8).map(move |i| byte & (1 << i) != 0);
        let expected: Vec<bool> = [false].into_iter().chain(l(0x35)).chain([true, false]).chain(l(0xc0)).chain([true]).collect();
        assert_eq!(bits(&levels), expected);
        assert_eq!(usart.txsta_flags(), 0, "still in the last stop bit");
        send(&mut usart, txsta, 1);
        assert_eq!(usart.txsta_flags(), TRMT);
    }

    #[test]
    fn sends_the_ninth_bit() {
        let mut usart = Usart::new();
        let txsta = TXEN | BRGH | TX9 | TX9D;
        usart.write_txreg(0x00, txsta);
        let levels = bits(&send(&mut usart, txsta, 48));
        assert_eq!(levels, [vec![false; 9], vec![true; 3]].concat());
    }

    // Feeds a waveform of bits (4 cycles each) to RX.
    fn receive(usart: &mut Usart, rcsta: u8, bits: &[bool]) {
        for &bit in bits {
            for _ in 0..4 {
                usart.tick(1, BRGH, rcsta, 0, bit);
            }
        }
    }

    fn frame(byte: u8, stop: bool) -> Vec<bool> {
        [false].into_iter().chain((0..8).map(|i| byte & (1 << i) != 0)).chain([stop, true]).collect()
    }

    #[test]
    fn receives_into_a_two_byte_fifo() {
        let mut usart = Usart::new();
        receive(&mut usart, ON, &[true, true]);
        receive(&mut usart, ON, &frame(0xa5, true));
        assert_eq!(usart.pir1_flags(0), RCIF);
        receive(&mut usart, ON, &frame(0x3c, false));
        assert_eq!((usart.rcreg(), usart.rcsta_flags()), (0xa5, 0));
        receive(&mut usart, ON, &frame(0x01, true));
        // The third byte is lost, and so is everything after it until CREN
        // is cleared.
        assert_eq!(usart.rcsta_flags(), OERR);
        usart.pop();
        assert_eq!((usart.rcreg(), usart.rcsta_flags()), (0x3c, FERR | OERR));
        usart.pop();
        receive(&mut usart, ON, &frame(0x02, true));
        assert_eq!(usart.pir1_flags(0), 0);
        usart.control_written(0, SPEN);
        usart.control_written(0, ON);
        assert_eq!(usart.rcsta_flags(), 0);
        receive(&mut usart, ON, &frame(0x03, true));
        assert_eq!(usart.rcreg(), 0x03);
    }

    #[test]
    fn ignores_glitches_and_a_disabled_receiver() {
        let mut usart = Usart::new();
        // Low for less than half a bit.
        usart.tick(1, BRGH, ON, 0, false);
        receive(&mut usart, ON, &[true; 12]);
        receive(&mut usart, SPEN, &frame(0x55, true));
        assert_eq!(usart.pir1_flags(0), 0);
    }
}
