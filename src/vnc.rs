// A minimal VNC (RFB 3.3-3.8) server for one emulated display: no
// authentication, raw encoding, whole-frame updates, any true-colour pixel
// format the viewer asks for. Key presses go to the board as key codes.
// Listens on 127.0.0.1 only, since there's no password.
use std::io::{ self, Read, Write };
use std::net::{ TcpListener, TcpStream };
use std::sync::{ Arc, Condvar, Mutex };
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use crate::parts::{ KEY_LEFT, KEY_RIGHT, KEY_UP, KEY_DOWN };

// How long a connection waits for a new frame before checking again
// whether its viewer has gone.
const POLL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct PixelFormat {
    bits_per_pixel: u8,
    depth: u8,
    big_endian: bool,
    true_colour: bool,
    max: [u16; 3],
    shift: [u8; 3]
}

// 0x00RRGGBB, little endian: the frame's own layout.
const SERVER_FORMAT: PixelFormat = PixelFormat {
    bits_per_pixel: 32, depth: 24, big_endian: false, true_colour: true, max: [255; 3], shift: [16, 8, 0]
};

impl PixelFormat {
    fn to_bytes(&self) -> [u8; 16] {
        let mut b = [0; 16];
        b[0] = self.bits_per_pixel;
        b[1] = self.depth;
        b[2] = self.big_endian as u8;
        b[3] = self.true_colour as u8;
        for c in 0..3 {
            b[4 + 2 * c..6 + 2 * c].copy_from_slice(&self.max[c].to_be_bytes());
            b[10 + c] = self.shift[c];
        }
        b
    }

    fn from_bytes(b: &[u8; 16]) -> Self {
        let max = |c: usize| u16::from_be_bytes([b[4 + 2 * c], b[5 + 2 * c]]);
        Self { bits_per_pixel: b[0], depth: b[1], big_endian: b[2] != 0, true_colour: b[3] != 0,
               max: [max(0), max(1), max(2)], shift: [b[10], b[11], b[12]] }
    }

    // Appends an 0xRRGGBB pixel in this format.
    fn put(&self, rgb: u32, out: &mut Vec<u8>) {
        let mut value = 0u32;
        for c in 0..3 {
            let level = (rgb >> (16 - 8 * c)) & 0xff;
            value |= (level * self.max[c] as u32 + 127) / 255 << self.shift[c];
        }
        match (self.bits_per_pixel, self.big_endian) {
            (8, _) => out.push(value as u8),
            (16, false) => out.extend_from_slice(&(value as u16).to_le_bytes()),
            (16, true) => out.extend_from_slice(&(value as u16).to_be_bytes()),
            (_, false) => out.extend_from_slice(&value.to_le_bytes()),
            (_, true) => out.extend_from_slice(&value.to_be_bytes())
        }
    }
}

// Key code for an X keysym, as the keypad models expect them: the
// character, or one of the parts::KEY_* codes.
fn key_code(keysym: u32) -> Option<u8> {
    match keysym {
        0x20..=0x7e => Some(keysym as u8),
        0xff08 => Some(8),
        0xff09 => Some(b'\t'),
        0xff0d | 0xff8d => Some(b'\r'),
        0xff1b => Some(27),
        0xff51 => Some(KEY_LEFT),
        0xff52 => Some(KEY_UP),
        0xff53 => Some(KEY_RIGHT),
        0xff54 => Some(KEY_DOWN),
        _ => None
    }
}

struct Frame {
    pixels: Vec<u32>,
    // Bumped by every publish(), so connections know what they've sent.
    version: u64
}

struct Shared {
    name: String,
    width: usize,
    height: usize,
    frame: Mutex<Frame>,
    changed: Condvar,
    keys: Sender<u8>
}

pub struct VncServer {
    shared: Arc<Shared>,
    pub port: u16
}

impl VncServer {
    // Starts listening; frames are width x height.
    pub fn start(name: &str, port: u16, width: usize, height: usize, keys: Sender<u8>) -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .map_err(|err| io::Error::new(err.kind(), format!("VNC port {port}: {err}")))?;
        let shared = Arc::new(Shared {
            name: name.to_string(), width, height,
            frame: Mutex::new(Frame { pixels: vec![0; width * height], version: 0 }),
            changed: Condvar::new(), keys
        });
        let accept_shared = shared.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let shared = accept_shared.clone();
                thread::spawn(move || {
                    // A viewer closing its window ends up here; nothing to report.
                    let _ = serve(stream, shared);
                });
            }
        });
        Ok(Self { shared, port })
    }

    // Replaces the frame (0xRRGGBB, row-major) shown to every viewer.
    pub fn publish(&self, pixels: &[u32]) {
        let mut frame = self.shared.frame.lock().unwrap();
        frame.pixels.copy_from_slice(pixels);
        frame.version += 1;
        self.shared.changed.notify_all();
    }
}

// What a viewer asked for and hasn't got yet.
struct Request {
    format: PixelFormat,
    pending: bool,
    // The viewer wants the frame even if it hasn't changed.
    full: bool,
    closed: bool
}

fn read_u16(r: &mut impl Read) -> io::Result<u16> {
    let mut b = [0; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_be_bytes(b))
}

fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

fn skip(r: &mut impl Read, n: u64) -> io::Result<()> {
    io::copy(&mut r.take(n), &mut io::sink()).map(|_| ())
}

fn serve(stream: TcpStream, shared: Arc<Shared>) -> io::Result<()> {
    stream.set_nodelay(true)?;
    let (mut r, mut w) = (stream.try_clone()?, stream);

    // Handshake: version, security "None", ClientInit, ServerInit.
    w.write_all(b"RFB 003.008\n")?;
    let mut version = [0; 12];
    r.read_exact(&mut version)?;
    if &version[..8] != b"RFB 003." {
        return Err(io::Error::other("not an RFB 3.x viewer"));
    }
    let minor: u32 = std::str::from_utf8(&version[8..11]).ok().and_then(|m| m.parse().ok()).unwrap_or(3);
    if minor >= 7 {
        w.write_all(&[1, 1])?;
        let mut choice = [0];
        r.read_exact(&mut choice)?;
        if minor >= 8 {
            w.write_all(&0u32.to_be_bytes())?;
        }
    } else {
        w.write_all(&1u32.to_be_bytes())?;
    }
    let mut shared_flag = [0];
    r.read_exact(&mut shared_flag)?;
    let mut init = vec![];
    init.extend_from_slice(&(shared.width as u16).to_be_bytes());
    init.extend_from_slice(&(shared.height as u16).to_be_bytes());
    init.extend_from_slice(&SERVER_FORMAT.to_bytes());
    init.extend_from_slice(&(shared.name.len() as u32).to_be_bytes());
    init.extend_from_slice(shared.name.as_bytes());
    w.write_all(&init)?;

    let request = Arc::new(Mutex::new(Request { format: SERVER_FORMAT, pending: false, full: false, closed: false }));
    let writer = {
        let (shared, request) = (shared.clone(), request.clone());
        thread::spawn(move || send_frames(w, &shared, &request))
    };
    let result = read_messages(&mut r, &shared, &request);
    request.lock().unwrap().closed = true;
    let _ = writer.join();
    result
}

// Handles the viewer's messages until it disconnects.
fn read_messages(r: &mut TcpStream, shared: &Shared, request: &Mutex<Request>) -> io::Result<()> {
    loop {
        let mut kind = [0];
        r.read_exact(&mut kind)?;
        match kind[0] {
            // SetPixelFormat
            0 => {
                let mut b = [0; 19];
                r.read_exact(&mut b)?;
                let format = PixelFormat::from_bytes(b[3..].try_into().unwrap());
                if !format.true_colour || ![8, 16, 32].contains(&format.bits_per_pixel) {
                    return Err(io::Error::other("only true-colour 8, 16 or 32 bpp formats are supported"));
                }
                request.lock().unwrap().format = format;
            },
            // SetEncodings: raw is always allowed, so the list doesn't matter.
            2 => {
                skip(r, 1)?;
                let count = read_u16(r)?;
                skip(r, 4 * count as u64)?;
            },
            // FramebufferUpdateRequest: the region is ignored, every update
            // is the whole frame.
            3 => {
                let mut b = [0; 9];
                r.read_exact(&mut b)?;
                let mut req = request.lock().unwrap();
                req.pending = true;
                req.full |= b[0] == 0;
                drop(req);
                shared.changed.notify_all();
            },
            // KeyEvent: presses only, as the keypad models expect.
            4 => {
                let mut b = [0; 3];
                r.read_exact(&mut b)?;
                let keysym = read_u32(r)?;
                if b[0] != 0 {
                    if let Some(code) = key_code(keysym) {
                        let _ = shared.keys.send(code);
                    }
                }
            },
            // PointerEvent
            5 => skip(r, 5)?,
            // ClientCutText
            6 => {
                skip(r, 3)?;
                let len = read_u32(r)?;
                skip(r, len as u64)?;
            },
            kind => return Err(io::Error::other(format!("unknown message type {kind}")))
        }
    }
}

// Sends the frame whenever the viewer has asked for one and either wants it
// regardless or it changed since the last one sent.
fn send_frames(mut w: TcpStream, shared: &Shared, request: &Mutex<Request>) -> io::Result<()> {
    let mut sent = None;
    let mut out = vec![];
    loop {
        let (format, pixels, version) = {
            let mut frame = shared.frame.lock().unwrap();
            loop {
                let mut req = request.lock().unwrap();
                if req.closed {
                    return Ok(());
                }
                if req.pending && (req.full || sent != Some(frame.version)) {
                    req.pending = false;
                    req.full = false;
                    break (req.format, frame.pixels.clone(), frame.version);
                }
                drop(req);
                frame = shared.changed.wait_timeout(frame, POLL).unwrap().0;
            }
        };
        out.clear();
        out.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0]);
        out.extend_from_slice(&(shared.width as u16).to_be_bytes());
        out.extend_from_slice(&(shared.height as u16).to_be_bytes());
        out.extend_from_slice(&0i32.to_be_bytes());
        for pixel in pixels {
            format.put(pixel, &mut out);
        }
        if let Err(err) = w.write_all(&out) {
            request.lock().unwrap().closed = true;
            return Err(err);
        }
        sent = Some(version);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn converts_pixel_formats() {
        let mut out = vec![];
        SERVER_FORMAT.put(0x123456, &mut out);
        assert_eq!(out, [0x56, 0x34, 0x12, 0]);
        // RGB565, big endian.
        let rgb565 = PixelFormat { bits_per_pixel: 16, depth: 16, big_endian: true, true_colour: true,
                                   max: [31, 63, 31], shift: [11, 5, 0] };
        out.clear();
        rgb565.put(0xff8000, &mut out);
        assert_eq!(out, (31u16 << 11 | 32 << 5).to_be_bytes());
        assert_eq!(PixelFormat::from_bytes(&rgb565.to_bytes()), rgb565);
    }

    // A viewer connects, gets a frame, then an update after publish(),
    // and its key press reaches the board.
    #[test]
    fn serves_frames_and_keys() {
        let (tx, rx) = channel();
        let server = (5990..6090).find_map(|port| VncServer::start("test", port, 2, 1, tx.clone()).ok()).unwrap();
        server.publish(&[0x112233, 0x445566]);

        let mut s = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut version = [0; 12];
        s.read_exact(&mut version).unwrap();
        assert_eq!(&version, b"RFB 003.008\n");
        s.write_all(b"RFB 003.008\n").unwrap();
        let mut types = [0; 2];
        s.read_exact(&mut types).unwrap();
        assert_eq!(types, [1, 1]);
        s.write_all(&[1]).unwrap();
        assert_eq!(read_u32(&mut s).unwrap(), 0);
        s.write_all(&[1]).unwrap();
        let mut init = [0; 24];
        s.read_exact(&mut init).unwrap();
        assert_eq!(&init[..4], &[0, 2, 0, 1]);
        let mut name = [0; 4];
        s.read_exact(&mut name).unwrap();
        assert_eq!(&name, b"test");

        let mut frame = || {
            s.write_all(&[3, 1, 0, 0, 0, 0, 0, 2, 0, 1]).unwrap();
            let mut update = [0; 16 + 8];
            s.read_exact(&mut update).unwrap();
            assert_eq!(&update[..16], &[0, 0, 0, 1, 0, 0, 0, 0, 0, 2, 0, 1, 0, 0, 0, 0]);
            update[16..].to_vec()
        };
        assert_eq!(frame(), [0x33, 0x22, 0x11, 0, 0x66, 0x55, 0x44, 0]);
        server.publish(&[0xffffff, 0]);
        assert_eq!(frame(), [0xff, 0xff, 0xff, 0, 0, 0, 0, 0]);

        s.write_all(&[4, 1, 0, 0, 0, 0, 0, b'a']).unwrap();
        s.write_all(&[4, 1, 0, 0, 0, 0, 0xff, 0x51]).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(b'a'));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(KEY_LEFT));
    }
}
