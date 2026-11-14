use std::io::BufWriter;
use std::path::Path;
use std::fs::File;
use std::sync::atomic::{ Ordering, AtomicBool };

static SIGINT: AtomicBool = AtomicBool::new(false);

mod pic;
mod common;
mod hw;
mod board;
mod boardfile;
mod kicad;
mod loader;
mod parts;
mod screen;
mod vnc;

#[cfg(test)]
mod tetris_test;
#[cfg(test)]
mod board_test;
#[cfg(test)]
mod spiram_test;
#[cfg(test)]
mod tinystories_test;
#[cfg(test)]
mod tinystories_fw_test;
#[cfg(test)]
mod uart_test;
#[cfg(test)]
mod weight_loader_test;

use crate::board::Board;
use crate::common::component::Ctx;

const DEFAULT_BOARD: &str = "boards/tetris/tetris.toml";

pub trait Display {
    // Width and height in pixels.
    fn size(&self) -> (usize, usize);
    // If the screen changed since the last call, writes it to pixels as
    // 0xRRGGBB, row-major, and returns true.
    fn render(&mut self, pixels: &mut [u32]) -> bool;
}

pub trait KeyInput {
    fn get_key(&mut self, key: u8, ctx: &mut Ctx);
}

extern "C" fn handler(_sig: libc::c_int) {
    SIGINT.store(true, Ordering::SeqCst);
}

pub unsafe fn install_interrupt_signal() {
    let mut sa: libc::sigaction = std::mem::zeroed();
    sa.sa_sigaction = handler as *const() as usize;
    libc::sigemptyset(&mut sa.sa_mask);
    sa.sa_flags = 0;
    libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

// Usage: emularium [BOARD] [--check] [--vcd FILE [NET_OR_BUS...]]
// BOARD is a board file (.toml), a KiCad schematic (.kicad_sch) or netlist
// (.net). Each display is served over VNC; Ctrl+C stops.
// --check prints how the board was loaded and exits.
// --vcd writes a VCD trace of the named nets and buses, or of all of them.
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut args = args.iter().map(String::as_str);
    let (mut path, mut check, mut vcd) = (None, false, None);
    while let Some(arg) = args.next() {
        match arg {
            "--check" => check = true,
            "--vcd" => {
                let file = args.next().ok_or("--vcd needs a file name.")?;
                vcd = Some((file, args.by_ref().collect::<Vec<&str>>()));
            },
            arg if !arg.starts_with("--") && path.is_none() => path = Some(arg),
            arg => return Err(format!("Unknown argument {arg}.").into())
        }
    }

    let mut board = Board::new();
    let path = path.unwrap_or(DEFAULT_BOARD);
    let report = loader::load(&mut board, Path::new(path))?;
    if check {
        println!("{report}");
        return Ok(());
    }
    if let Some((file, names)) = vcd {
        board.trace(Box::new(BufWriter::new(File::create(file)?)), &names)?;
    }

    board.set_realtime(true);
    board.init();
    for screen in board.serve_screens()? {
        println!("VNC {screen}");
    }
    println!("Running {path}; Ctrl+C stops.");

    unsafe { install_interrupt_signal() };
    board.run();

    board.stop_trace()?;
    Ok(())
}
