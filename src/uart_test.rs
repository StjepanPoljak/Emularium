// The PIC's USART on a board, against the host-side Uart: a small echo
// firmware at 20 MHz and 113,636 baud (SPBRG 10, BRGH: the closest to
// 115,200, 1.4% slow), talking to a port at 115,200. sigrok's UART
// decoder checks the PIC's frames independently of the emulator.
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::{ Receiver, Sender };
use crate::Board;
use crate::common::byte_data::ByteData;
use crate::common::time::MS;
use crate::hw::uart::Uart;
use crate::pic::pic16f876::PIC16F876;

// Says OK, then sends back every byte it gets plus one.
const ECHO: &str = r#"
	LIST P=16F876
	#include <p16f876.inc>
	__CONFIG _HS_OSC & _WDT_OFF & _PWRTE_ON & _BODEN_OFF & _LVP_OFF & _CP_OFF
	ORG	0
	bsf	STATUS,RP0
	movlw	.10
	movwf	SPBRG
	movlw	B'00100100'		; TXEN, BRGH
	movwf	TXSTA
	movlw	B'11000000'		; RC6, RC7 inputs, as the USART wants
	movwf	TRISC
	bcf	STATUS,RP0
	movlw	B'10010000'		; SPEN, CREN
	movwf	RCSTA
	movlw	'O'
	call	PUT
	movlw	'K'
	call	PUT
LOOP:
	btfss	PIR1,RCIF
	goto	LOOP
	movf	RCREG,W
	addlw	1
	call	PUT
	goto	LOOP
PUT:
	btfss	PIR1,TXIF
	goto	PUT
	movwf	TXREG
	return
	END
"#;

fn assemble(dir: &Path, name: &str, source: &str) -> ByteData {
    let asm = dir.join(format!("{name}.asm"));
    std::fs::write(&asm, source).unwrap();
    let hex = dir.join(format!("{name}.hex"));
    let out = Command::new("gpasm").args(["-p", "p16f876", "-o"]).arg(&hex).arg(&asm)
        .output().expect("gpasm isn't installed");
    assert!(out.status.success(), "gpasm: {}", String::from_utf8_lossy(&out.stdout));
    ByteData::new_from_intel_hex(hex.to_str().unwrap()).unwrap()
}

fn board(dir: &Path) -> (Board, Sender<u8>, Receiver<u8>) {
    let mut pic = PIC16F876::new();
    crate::common::mcu::MCU::load_rom(&mut pic, &assemble(dir, "echo", ECHO));
    pic.set_fosc(20_000_000);
    let (uart, to_board, from_board) = Uart::new(115_200);
    let mut board = Board::new();
    board.add_component("U1", Box::new(pic));
    board.add_component("P1", Box::new(uart));
    board.net("SER_TX", &["U1.RC6", "P1.RX"]);
    board.net("SER_RX", &["P1.TX", "U1.RC7"]);
    (board, to_board, from_board)
}

#[test]
fn pic_usart_talks_to_the_host() {
    let dir = tempfile::tempdir().unwrap();
    let (mut board, to_board, from_board) = board(dir.path());
    let vcd = dir.path().join("uart.vcd");
    board.trace(Box::new(BufWriter::new(File::create(&vcd).unwrap())), &["SER_TX", "SER_RX"]).unwrap();
    board.init();
    // Back to back: the PIC echoes as fast as the bytes come.
    let sent: Vec<u8> = (0..40u8).map(|i| i.wrapping_mul(37)).collect();
    sent.iter().for_each(|&byte| to_board.send(byte).unwrap());
    while board.now() < 6 * MS {
        board.step();
    }
    board.stop_trace().unwrap();

    let expected: Vec<u8> = [b"OK".to_vec(), sent.iter().map(|b| b.wrapping_add(1)).collect()].concat();
    assert_eq!(from_board.try_iter().collect::<Vec<u8>>(), expected);
    assert_eq!(board.contentions(), 0);

    // The PIC's frames at its own baud rate, 20 MHz / (16 * 11).
    let out = Command::new("sigrok-cli")
        .args(["-i", vcd.to_str().unwrap(), "-I", "vcd:downsample=1000",
               "-P", "uart:rx=SER_TX:baudrate=113636", "-A", "uart=rx-data"])
        .output().expect("sigrok-cli isn't installed");
    assert!(out.status.success(), "sigrok-cli: {}", String::from_utf8_lossy(&out.stderr));
    let decoded: Vec<u8> = String::from_utf8(out.stdout).unwrap().lines()
        .map(|line| u8::from_str_radix(line.rsplit(' ').next().unwrap(), 16).unwrap()).collect();
    assert_eq!(decoded, expected);
}
