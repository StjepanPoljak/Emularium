// The serial loader of boards/tinystories (loader.asm, load_weights.py) on
// the emulated board, through the UART part's TCP port: first the protocol
// by hand, then the host script writing a made-up image across a chip
// boundary. Needs gpasm; the script test also needs python3 with pyserial.
use std::io::{ Read, Write };
use std::net::{ TcpListener, TcpStream };
use std::path::Path;
use std::process::Command;
use std::time::{ Duration, Instant };
use crate::Board;
use crate::hw::eeprom25lc1024::SIZE;

const DIR: &str = "boards/tinystories";
const PAGE: usize = 256;

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

// loader.toml with a freshly assembled loader.hex, on a free TCP port.
fn board(dir: &Path) -> (Board, u16) {
    let hex = dir.join("loader.hex");
    let out = Command::new("gpasm").args(["-p", "p16f876", "-o"]).arg(&hex).arg(format!("{DIR}/loader.asm"))
        .output().expect("gpasm isn't installed");
    assert!(out.status.success(), "gpasm: {}", String::from_utf8_lossy(&out.stdout));
    let port = free_port();
    let text = std::fs::read_to_string(format!("{DIR}/loader.toml")).unwrap()
        .replace("\"loader.hex\"", &format!("{:?}", hex.to_str().unwrap()))
        .replace("tcp = \"5555\"", &format!("tcp = \"{port}\""));
    let mut board = Board::new();
    crate::boardfile::build(&mut board, &text, Path::new(DIR)).unwrap();
    board.init();
    (board, port)
}

fn connect(board: &mut Board, port: u16) -> TcpStream {
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_nonblocking(true).unwrap();
    // Let the listener thread take the client before the board talks.
    std::thread::sleep(Duration::from_millis(50));
    board.step();
    stream
}

// Sends request and runs the board until n reply bytes are back.
fn exchange(board: &mut Board, stream: &mut TcpStream, request: &[u8], n: usize) -> Vec<u8> {
    stream.write_all(request).unwrap();
    let mut reply = vec![];
    let start = Instant::now();
    while reply.len() < n {
        assert!(start.elapsed() < Duration::from_secs(60), "got {} of {n} bytes: {reply:?}", reply.len());
        for _ in 0..10_000 {
            board.step();
        }
        let mut buf = [0u8; 512];
        match stream.read(&mut buf) {
            Ok(k) => reply.extend_from_slice(&buf[..k]),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => (),
            Err(err) => panic!("{err}")
        }
    }
    assert_eq!(reply.len(), n, "{reply:?}");
    reply
}

fn crc16(data: &[u8]) -> u16 {
    data.iter().fold(0xffff, |mut crc: u16, &byte| {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
        crc
    })
}

fn memory(board: &mut Board, part: &str) -> Vec<u8> {
    let idx = board.find_component(part).unwrap();
    board.get_component(idx).unwrap().memory().unwrap().to_vec()
}

fn write_cmd(addr: usize, data: &[u8], crc: u16) -> Vec<u8> {
    [&[b'W', (addr >> 16) as u8, (addr >> 8) as u8, addr as u8][..], data, &crc.to_be_bytes()].concat()
}

#[test]
fn loader_protocol() {
    assert_eq!(crc16(b"123456789"), 0x29b1);
    let dir = tempfile::tempdir().unwrap();
    let (mut board, port) = board(dir.path());
    let mut stream = connect(&mut board, port);

    assert_eq!(exchange(&mut board, &mut stream, b"?", 5), b"EMLD1");
    assert_eq!(exchange(&mut board, &mut stream, b"x", 1), b"!");

    // The first page of EE1, and EE2's last.
    let page: Vec<u8> = (0..PAGE).map(|i| (i * 7 + 3) as u8).collect();
    for addr in [SIZE, 3 * SIZE - PAGE] {
        assert_eq!(exchange(&mut board, &mut stream, &write_cmd(addr, &page, crc16(&page)), 1), b"K");
        let read = exchange(&mut board, &mut stream, &[b'R', (addr >> 16) as u8, (addr >> 8) as u8, addr as u8], PAGE + 2);
        assert_eq!(read[..PAGE], page[..]);
        assert_eq!(read[PAGE..], crc16(&page).to_be_bytes());
    }
    assert_eq!(memory(&mut board, "U3")[..PAGE], page[..]);
    assert_eq!(memory(&mut board, "U4")[SIZE - PAGE..], page[..]);
    assert!(memory(&mut board, "U2").iter().all(|&b| b == 0xff));

    // A bad CRC: written anyway, and said so.
    let other: Vec<u8> = page.iter().rev().copied().collect();
    assert_eq!(exchange(&mut board, &mut stream, &write_cmd(0, &other, crc16(&other) ^ 1), 1), b"E");
    assert_eq!(memory(&mut board, "U2")[..PAGE], other[..]);
    assert_eq!(board.contentions(), 0);
}

// load_weights.py against the emulated board: the last page of EE0, the
// first of EE1 and a half page at the end of the image.
#[test]
fn load_weights_script_fills_the_eeproms() {
    let has_pyserial = Command::new("python3").args(["-c", "import serial"]).status().is_ok_and(|s| s.success());
    if !has_pyserial {
        eprintln!("skipped: needs python3 with pyserial");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (mut board, port) = board(dir.path());
    let start = SIZE - PAGE;
    let image: Vec<u8> = (0..SIZE + PAGE + PAGE / 2).map(|i| (i ^ (i >> 8) ^ (i >> 16)) as u8).collect();
    let path = dir.path().join("image.bin");
    std::fs::write(&path, &image).unwrap();

    let mut script = Command::new("python3")
        .arg(format!("{DIR}/load_weights.py"))
        .arg(format!("socket://localhost:{port}"))
        .arg(&path)
        .args(["--from", &start.to_string()])
        .spawn().unwrap();
    let begin = Instant::now();
    let status = loop {
        if let Some(status) = script.try_wait().unwrap() {
            break status;
        }
        assert!(begin.elapsed() < Duration::from_secs(120), "load_weights.py didn't finish");
        for _ in 0..10_000 {
            board.step();
        }
    };
    assert!(status.success(), "load_weights.py failed");

    let (ee0, ee1) = (memory(&mut board, "U2"), memory(&mut board, "U3"));
    assert_eq!(ee0[start..], image[start..SIZE]);
    assert!(ee0[..start].iter().all(|&b| b == 0xff));
    assert_eq!(ee1[..PAGE + PAGE / 2], image[SIZE..]);
    // The last page is padded with erased bytes.
    assert!(ee1[PAGE + PAGE / 2..].iter().all(|&b| b == 0xff));
}
