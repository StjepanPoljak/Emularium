// A serial port on the host side, like a USB-UART adapter: 8N1 frames at
// a set baud rate on TX (to the board's RX) and RX (from the board's TX).
// The bytes come from and go to a pair of channels; Uart::tcp() connects
// them to a TCP port instead, so a host program can use the emulated board
// like a real one (pyserial: serial_for_url("socket://localhost:PORT")).
use std::collections::BTreeSet;
use std::io::{ Read, Write };
use std::net::{ TcpListener, TcpStream };
use std::sync::{ Arc, Mutex };
use std::sync::mpsc::{ self, Receiver, Sender, TryRecvError };

use crate::common::component::{ Component, Ctx };
use crate::common::net::Level;
use crate::common::time::{ Time, S, US };

const PINS: [&str; 2] = ["TX", "RX"];
const TX: usize = 0;
const RX: usize = 1;
// How often an idle TX looks for new bytes.
const POLL: Time = 100 * US;

enum RxState {
    Idle,
    // Bits sampled so far (start bit excluded), LSB first.
    Bits(u8, u16)
}

pub struct Uart {
    bit: Time,
    input: Receiver<u8>,
    output: Sender<u8>,
    // The frame being sent: bits left, LSB first, and how many.
    tx: Option<(u16, u8)>,
    tx_next: Time,
    rx: RxState,
    rx_next: Time,
    rx_level: bool,
    // Wakeups asked for and not yet due.
    wakeups: BTreeSet<Time>,
    // Frames received with a stop bit of 0.
    pub framing_errors: u64
}

impl Uart {
    // A port at baud bits per second: bytes sent to the returned Sender go
    // out on TX, bytes received on RX come out of the returned Receiver.
    pub fn new(baud: u64) -> (Self, Sender<u8>, Receiver<u8>) {
        let (to_board, input) = mpsc::channel();
        let (output, from_board) = mpsc::channel();
        let uart = Self { bit: S / baud, input, output, tx: None, tx_next: 0, rx: RxState::Idle, rx_next: 0,
                          rx_level: true, wakeups: BTreeSet::new(), framing_errors: 0 };
        (uart, to_board, from_board)
    }

    // A port that serves TCP port on localhost, one client at a time. While
    // nobody is connected, what the board sends is dropped.
    pub fn tcp(baud: u64, port: u16) -> std::io::Result<Self> {
        let (uart, to_board, from_board) = Self::new(baud);
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let client: Arc<Mutex<Option<TcpStream>>> = Arc::new(Mutex::new(None));

        let writer = client.clone();
        std::thread::spawn(move || {
            for byte in from_board {
                if let Some(stream) = writer.lock().unwrap().as_mut() {
                    let _ = stream.write_all(&[byte]);
                }
            }
        });
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = stream.set_nodelay(true);
                let Ok(mut reader) = stream.try_clone() else { continue };
                *client.lock().unwrap() = Some(stream);
                let mut buf = [0u8; 4096];
                while let Ok(n @ 1..) = reader.read(&mut buf) {
                    if buf[..n].iter().any(|&byte| to_board.send(byte).is_err()) {
                        return;
                    }
                }
                *client.lock().unwrap() = None;
            }
        });
        Ok(uart)
    }

    fn wake_at(&mut self, ctx: &mut Ctx, time: Time) {
        if self.wakeups.insert(time) {
            ctx.wake_after(time - ctx.now());
        }
    }

    // Sends the next bit of the frame, or starts the next frame.
    fn tx_step(&mut self, ctx: &mut Ctx) {
        if self.tx.is_none() {
            match self.input.try_recv() {
                Ok(byte) => self.tx = Some(((byte as u16) << 1 | 0x200, 10)),
                Err(TryRecvError::Empty) => {
                    self.tx_next = ctx.now() + POLL;
                    return self.wake_at(ctx, self.tx_next);
                },
                Err(TryRecvError::Disconnected) => return
            }
        }
        let (frame, left) = self.tx.unwrap();
        ctx.drive(TX, Level::from_bit(frame & 1 != 0));
        self.tx = if left > 1 { Some((frame >> 1, left - 1)) } else { None };
        self.tx_next = ctx.now() + self.bit;
        self.wake_at(ctx, self.tx_next);
    }

    fn rx_sample(&mut self, ctx: &mut Ctx) {
        let RxState::Bits(count, shift) = self.rx else { return };
        let bit = self.rx_level as u16;
        if count == 0 && bit == 1 {
            self.rx = RxState::Idle;                // not a start bit after all
            return;
        }
        if count < 9 {
            // The start bit is sample 0: shift it out at the end.
            self.rx = RxState::Bits(count + 1, shift | (bit << count));
            self.rx_next = ctx.now() + self.bit;
            return self.wake_at(ctx, self.rx_next);
        }
        if bit == 0 {
            self.framing_errors += 1;
        }
        let _ = self.output.send((shift >> 1) as u8);
        self.rx = RxState::Idle;
    }
}

impl Component for Uart {
    fn init(&mut self) { }

    fn pins(&self) -> &[&'static str] {
        &PINS
    }

    fn start(&mut self, ctx: &mut Ctx) {
        ctx.drive(TX, Level::High);
        self.tx_next = ctx.now() + POLL;
        self.wake_at(ctx, self.tx_next);
    }

    fn wake(&mut self, ctx: &mut Ctx) {
        let now = ctx.now();
        self.wakeups.retain(|&time| time > now);
        if self.tx_next == now {
            self.tx_step(ctx);
        }
        if self.rx_next == now {
            self.rx_sample(ctx);
        }
    }

    fn input_changed(&mut self, pin: usize, level: Level, ctx: &mut Ctx) {
        if pin != RX {
            return;
        }
        // An undriven line idles high, as with the adapter's pull-up.
        self.rx_level = level != Level::Low;
        if matches!(self.rx, RxState::Idle) && !self.rx_level {
            // A start bit: sample in the middle of each bit.
            self.rx = RxState::Bits(0, 0);
            self.rx_next = ctx.now() + self.bit / 2;
            self.wake_at(ctx, self.rx_next);
        }
    }
}
