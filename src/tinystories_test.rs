// Runs boards/tinystories with eeprom_test.asm: the PIC reads the three
// 25LC1024 EEPROMs that hold the weight image. The board gets a made-up
// image of the real one's size, so the test doesn't need
// tinystories/export.py's output.
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

#[test]
fn pic_reads_the_weight_eeproms() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("weights.bin");
    let image = image();
    std::fs::write(&path, &image).unwrap();
    let text = std::fs::read_to_string(format!("{DIR}/tinystories.toml")).unwrap();
    assert_eq!(text.matches(IMAGE).count(), 3);
    let text = text.replace(IMAGE, path.to_str().unwrap());

    let mut board = Board::new();
    crate::boardfile::build(&mut board, &text, Path::new(DIR)).unwrap();
    let pic_idx = board.find_component("U1").unwrap();
    board.init();
    let timeout: Time = 20 * MS;
    while mem(&mut board, pic_idx, DONE) != 1 {
        assert!(board.now() < timeout, "firmware didn't finish");
        board.step();
    }
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
