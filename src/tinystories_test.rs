// Runs boards/tinystories with eeprom_test.asm: the PIC reads the three
// 25LC1024 EEPROMs that hold the weight image. The board gets a made-up
// image of the real one's size, so the test doesn't need
// tinystories/export.py's output.
use std::fs::File;
use std::io::{ BufWriter, Write };
use std::path::Path;
use crate::Board;
use crate::common::time::{ Time, MS, US };
use crate::hw::eeprom25lc1024::{ SIGNATURE, SIZE };

const DIR: &str = "boards/tinystories";
const IMAGE: &str = "../../tinystories/out/weights_n1_k0.bin";
// The single-PIC image from tinystories/export.py.
const IMAGE_LEN: usize = 269_880;

// File registers of eeprom_test.asm.
const DONE: usize = 0x20;
const SIG0: usize = 0x21;
const STAT0: usize = 0x24;
const BUFFER: usize = 0x40;
const READ_LEN: usize = 16;
// Where the firmware reads each chip.
const ADDR: [usize; 3] = [0, 0, 0x1e30];

fn mem(board: &mut Board, pic_idx: usize, addr: usize) -> u8 {
    let mcu = board.get_component(pic_idx).unwrap().as_mcu().unwrap();
    mcu.dump_mem(addr, 0).unwrap().values[0] as u8
}

// A header like export.py's, then xorshift bytes.
fn image() -> Vec<u8> {
    let mut x: u32 = 0x2545f491;
    let mut image = b"TS26".to_vec();
    image.resize_with(IMAGE_LEN, || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x as u8
    });
    image
}

// The board with eeprom_test.hex and image in the EEPROMs; returns it and
// the PIC's index.
fn board(image: &[u8], dir: &Path) -> (Board, usize) {
    let path = dir.join("weights.bin");
    std::fs::write(&path, image).unwrap();
    let text = std::fs::read_to_string(format!("{DIR}/tinystories.toml")).unwrap();
    assert_eq!(text.matches(IMAGE).count(), 3);
    let text = text.replace(IMAGE, path.to_str().unwrap())
        .replace("tinystories.hex", "eeprom_test.hex");
    let mut board = Board::new();
    crate::boardfile::build(&mut board, &text, Path::new(DIR)).unwrap();
    let pic_idx = board.find_component("U1").unwrap();
    (board, pic_idx)
}

// Runs eeprom_test.asm to the end, tracing the named nets to trace if given.
fn run_eeprom_test(image: &[u8], trace: Option<(Box<dyn Write>, &[&str])>) -> (Board, usize) {
    let dir = tempfile::tempdir().unwrap();
    let (mut board, pic_idx) = board(image, dir.path());
    if let Some((out, names)) = trace {
        board.trace(out, names).unwrap();
    }
    board.init();
    let timeout: Time = 20 * MS;
    while mem(&mut board, pic_idx, DONE) != 1 {
        assert!(board.now() < timeout, "firmware didn't finish");
        board.step();
    }
    board.stop_trace().unwrap();
    (board, pic_idx)
}

#[test]
fn pic_reads_the_weight_eeproms() {
    let image = image();
    let (mut board, pic_idx) = run_eeprom_test(&image, None);
    eprintln!("eeprom_test.asm done after {} us", board.now() / US);

    for chip in 0..3 {
        assert_eq!(mem(&mut board, pic_idx, SIG0 + chip), SIGNATURE, "EE{chip} signature");
        assert_eq!(mem(&mut board, pic_idx, STAT0 + chip), 0, "EE{chip} status");
        let start = chip * SIZE + ADDR[chip];
        let expected: Vec<u8> = (start..start + READ_LEN).map(|i| image.get(i).copied().unwrap_or(0xff)).collect();
        let read: Vec<u8> = (0..READ_LEN).map(|i| mem(&mut board, pic_idx, BUFFER + chip * READ_LEN + i)).collect();
        assert_eq!(read, expected, "EE{chip} at {:#07x}", ADDR[chip]);
    }
    // EE0 starts with the header; EE2's read spans the end of the image.
    assert_eq!(mem(&mut board, pic_idx, BUFFER), b'T');
    assert_eq!(mem(&mut board, pic_idx, BUFFER + 2 * READ_LEN + 8), 0xff);
    assert_eq!(board.contentions(), 0);
}

// Runs sigrok's SPI decoder (mode 0) on a VCD file, with cs as chip
// select; returns the transactions as (MOSI bytes, MISO bytes).
fn sigrok_spi(vcd: &Path, cs: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
    let decode = |class: &str| -> Vec<(u64, Vec<u8>)> {
        let out = std::process::Command::new("sigrok-cli")
            .args(["-i", vcd.to_str().unwrap(), "-I", "vcd:downsample=1000",
                   "-P", &format!("spi:clk=SPI_SCK:mosi=SPI_MOSI:miso=SPI_MISO:cs={cs}:cpol=0:cpha=0"),
                   "-A", &format!("spi={class}"), "--protocol-decoder-samplenum"])
            .output()
            .expect("sigrok-cli isn't installed");
        assert!(out.status.success(), "sigrok-cli: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().lines().map(|line| {
            let (samples, text) = line.split_once(" spi-1:").unwrap_or_else(|| panic!("sigrok-cli: {line}"));
            let start = samples.split_once('-').unwrap().0.parse().unwrap();
            (start, text.split_whitespace().map(|byte| u8::from_str_radix(byte, 16).unwrap()).collect())
        }).collect()
    };
    let warnings = decode("warnings");
    assert!(warnings.is_empty(), "sigrok warnings on {cs}: {warnings:?}");
    let (mosi, miso) = (decode("mosi-transfer"), decode("miso-transfer"));
    assert_eq!(mosi.len(), miso.len());
    mosi.into_iter().zip(miso).map(|((t0, mosi), (t1, miso))| {
        assert_eq!(t0, t1);
        (mosi, miso)
    }).filter(|(mosi, _)| !mosi.is_empty()).collect()
}

// Decodes the firmware's traffic to each EEPROM with sigrok, independently
// of the emulator, and checks it against the 25LC1024 datasheet: RDID
// clocks out the signature after a dummy address, RDSR the status, READ
// the data after the address; SO stays high-Z (read as 0) until then.
#[test]
fn sigrok_decodes_eeprom_traffic() {
    let dir = tempfile::tempdir().unwrap();
    let vcd = dir.path().join("spi.vcd");
    let out = Box::new(BufWriter::new(File::create(&vcd).unwrap()));
    let image = image();
    let cs = ["EE0_CS", "EE1_CS", "EE2_CS"];
    run_eeprom_test(&image, Some((out, &["SPI_SCK", "SPI_MOSI", "SPI_MISO", cs[0], cs[1], cs[2]])));

    for chip in 0..3 {
        let addr = ADDR[chip];
        let start = chip * SIZE + addr;
        let data: Vec<u8> = (start..start + READ_LEN).map(|i| image.get(i).copied().unwrap_or(0xff)).collect();
        let read = [vec![0x03, 0, (addr >> 8) as u8, addr as u8], vec![0; READ_LEN]].concat();
        let expected = vec![
            (vec![0xab, 0, 0, 0, 0], vec![0, 0, 0, 0, SIGNATURE]),
            (vec![0x05, 0], vec![0, 0]),
            (read, [vec![0; 4], data].concat()),
        ];
        assert_eq!(sigrok_spi(&vcd, cs[chip]), expected, "{}", cs[chip]);
    }
}

// How fast the emulator runs this board: simulated time per wall-clock
// second, with the PIC at 20 MHz spinning after eeprom_test.asm.
// cargo test --release emulator_speed -- --ignored --nocapture
#[test]
#[ignore]
fn emulator_speed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("weights.bin");
    std::fs::write(&path, image()).unwrap();
    let text = std::fs::read_to_string(format!("{DIR}/tinystories.toml")).unwrap().replace(IMAGE, path.to_str().unwrap());
    let mut board = Board::new();
    crate::boardfile::build(&mut board, &text, Path::new(DIR)).unwrap();
    board.init();
    let start = std::time::Instant::now();
    while board.now() < 500 * MS {
        board.step();
    }
    let wall = start.elapsed().as_secs_f64();
    eprintln!("500 ms simulated in {wall:.2} s: {:.2}x real time", 0.5 / wall);
}
