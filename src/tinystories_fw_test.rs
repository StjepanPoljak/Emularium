// Runs tinystories.asm on the TinyStories board with the real weight image
// and checks every intermediate value against the integer reference's
// trace (tinystories/vectors.py), position by position, at the firmware's
// CK_ labels. Needs tinystories/out/ (export.py, vectors.py) and gpasm;
// skipped when the weight image or the trace is missing.
//
// cargo test --release tinystories_fw -- --nocapture
// TS_POSITIONS=n checks n positions (default 2; 1 in a debug build).
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use crate::Board;
use crate::common::time::MS;

const DIR: &str = "boards/tinystories";
const IMAGE: &str = "tinystories/out/weights_n1_k0.bin";
const VECTORS: &str = "tinystories/out/vectors_seed1.txt";

const O_AREA: usize = 0x18000;
const HB_AREA: usize = 0x18100;
const LG_AREA: usize = 0x18400;

const CHECKS: [&str; 11] = ["CK_EMB", "CK_ATTNORM", "CK_QKV", "CK_KV", "CK_ATTOUT", "CK_XATT",
                            "CK_FFNNORM", "CK_HB", "CK_XFFN", "CK_LOGITS", "CK_TOKEN"];

type Vectors = HashMap<(usize, Option<usize>, String), Vec<i64>>;

fn vectors() -> Vectors {
    std::fs::read_to_string(VECTORS).unwrap().lines().filter(|line| !line.starts_with('#')).map(|line| {
        let mut words = line.split_whitespace();
        let pos = words.next().unwrap().parse().unwrap();
        let layer = words.next().unwrap().parse().ok();
        let name = words.next().unwrap().to_string();
        ((pos, layer, name), words.map(|v| v.parse().unwrap()).collect())
    }).collect()
}

// Label addresses from a gpasm listing's symbol table.
fn symbols(lst: &str) -> HashMap<String, usize> {
    lst.lines().filter_map(|line| {
        let words: Vec<&str> = line.split_whitespace().collect();
        (words.len() >= 3 && words[1] == "ADDRESS")
            .then(|| (words[0].to_string(), usize::from_str_radix(words[2], 16).unwrap()))
    }).collect()
}

fn s24(bytes: &[u8]) -> Vec<i64> {
    bytes.chunks(3).map(|b| (((b[2] as i32) << 24 | (b[1] as i32) << 16 | (b[0] as i32) << 8) >> 8) as i64).collect()
}

pub struct Fw {
    pub board: Board,
    pub pic: usize,
    sram: usize,
    syms: HashMap<String, usize>,
    _dir: tempfile::TempDir
}

impl Fw {
    // Assembles the firmware (with -D SEED=seed) and loads the board.
    pub fn new(seed: u32) -> Option<Self> {
        if !Path::new(IMAGE).exists() {
            eprintln!("{IMAGE} is missing (cd tinystories && python3 export.py): skipped");
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        let hex = dir.path().join("fw.hex");
        let out = Command::new("gpasm")
            .args(["-p", "p16f876", "-I", DIR, "-D", &format!("SEED={seed}"), "-o"]).arg(&hex)
            .arg(format!("{DIR}/tinystories.asm"))
            .output().expect("gpasm isn't installed");
        assert!(out.status.success(), "gpasm: {}", String::from_utf8_lossy(&out.stdout));
        let syms = symbols(&std::fs::read_to_string(dir.path().join("fw.lst")).unwrap());
        let text = std::fs::read_to_string(format!("{DIR}/tinystories.toml")).unwrap()
            .replace("../../tinystories/", &format!("{}/tinystories/", std::env::current_dir().unwrap().display()))
            .replace("\"tinystories.hex\"", &format!("\"{}\"", hex.display()));
        let mut board = Board::new();
        crate::boardfile::build(&mut board, &text, Path::new(DIR)).unwrap();
        let pic = board.find_component("U1").unwrap();
        let sram = board.find_component("U5").unwrap();
        Some(Self { board, pic, sram, syms, _dir: dir })
    }

    pub fn sym(&self, name: &str) -> usize {
        self.syms[name]
    }

    pub fn ram(&mut self, addr: usize) -> u8 {
        let mcu = self.board.get_component(self.pic).unwrap().as_mcu().unwrap();
        mcu.dump_mem(addr, 0).unwrap().values[0] as u8
    }

    fn ram_n(&mut self, addr: usize, n: usize) -> Vec<u8> {
        (0..n).map(|i| self.ram(addr + i)).collect()
    }

    pub fn pc(&mut self) -> usize {
        self.board.get_component(self.pic).unwrap().as_mcu().unwrap().pc()
    }

    fn sram(&mut self, addr: usize, n: usize) -> Vec<u8> {
        self.board.get_component(self.sram).unwrap().memory().unwrap()[addr..addr + n].to_vec()
    }

    // x[0..64]: 0..15 at 0x20, 16..63 at 0x190, int16.
    fn x(&mut self) -> Vec<i64> {
        (0..64).map(|i| {
            let addr = if i < 16 { 0x20 + 2 * i } else { 0x190 + 2 * (i - 16) };
            i16::from_le_bytes([self.ram(addr), self.ram(addr + 1)]) as i64
        }).collect()
    }

    // The n activations, offset binary: 0..95 at 0x110, 96.. at 0xA0.
    fn act(&mut self, n: usize) -> Vec<i64> {
        (0..n).map(|i| self.ram(if i < 96 { 0x110 + i } else { 0xA0 + i - 96 }) as i64 - 128).collect()
    }

    // Runs until a CK_ label; returns its name.
    pub fn run_to_check(&mut self) -> String {
        let start = self.board.now();
        loop {
            if self.board.step() {
                let pc = self.pc() - 1;
                if let Some(name) = CHECKS.iter().find(|name| self.syms[**name] == pc) {
                    return name.to_string();
                }
            }
            assert!(self.board.now() - start < 60_000 * MS, "no checkpoint within a minute of simulated time");
        }
    }

    pub fn break_at_checks(&mut self) {
        for name in CHECKS {
            let addr = self.syms[name];
            self.board.add_breakpoint(self.pic, addr);
        }
    }
}

fn compare(what: &str, got: &[i64], want: &[i64]) {
    assert_eq!(got.len(), want.len(), "{what}: length");
    if let Some(i) = (0..got.len()).find(|i| got[*i] != want[*i]) {
        panic!("{what}: first difference at [{i}]: got {}, want {}\n got  {:?}\n want {:?}",
               got[i], want[i], &got[i.saturating_sub(4)..(i + 8).min(got.len())],
               &want[i.saturating_sub(4)..(i + 8).min(want.len())]);
    }
}

#[test]
fn tinystories_fw_matches_reference() {
    let Some(mut fw) = Fw::new(1) else { return };
    if !Path::new(VECTORS).exists() {
        eprintln!("{VECTORS} is missing (cd tinystories && python3 vectors.py): skipped");
        return;
    }
    let vec = vectors();
    // A position takes about 6 s of simulated time: ~5 s in a release
    // build, ~50 s in a debug one.
    let default = if cfg!(debug_assertions) { 1 } else { 2 };
    let positions: usize = std::env::var("TS_POSITIONS").ok().and_then(|n| n.parse().ok()).unwrap_or(default);
    fw.break_at_checks();
    fw.board.init();
    let (layer_at, grp_at, pos_at, ev_at) = (0x58, 0x59, 0x65, 0x49);
    let mut pos = 0;
    while pos < positions {
        let check = fw.run_to_check();
        let p = fw.ram(pos_at) as usize | (fw.ram(pos_at + 1) as usize) << 8;
        assert_eq!(p, pos, "position");
        let l = fw.ram(layer_at) as usize;
        let g = fw.ram(grp_at) as usize;
        let v = |name: &str, layer: Option<usize>| -> Vec<i64> {
            vec.get(&(pos, layer, name.to_string())).unwrap_or_else(|| panic!("no vector {pos} {layer:?} {name}")).clone()
        };
        let at = |what: &str| format!("pos {pos} layer {l} {what}");
        match check.as_str() {
            "CK_EMB" => compare(&at("x_in"), &fw.x(), &v("x_in", None)),
            "CK_ATTNORM" | "CK_FFNNORM" => {
                let name = if check == "CK_ATTNORM" { "att_norm" } else { "ffn_norm" };
                compare(&at(&format!("{name}_a")), &fw.act(64), &v(&format!("{name}_a"), Some(l)));
                compare(&at(&format!("{name}_E")), &[fw.ram(ev_at) as i8 as i64], &v(&format!("{name}_E"), Some(l)));
            },
            "CK_QKV" => {
                let q = s24(&fw.ram_n(0xA0, 48));
                let k = s24(&fw.ram_n(0xD0, 24));
                let vv = s24(&fw.ram_n(0x150, 24));
                compare(&at(&format!("q group {g}")), &q, &v("q", Some(l))[g * 16..g * 16 + 16]);
                compare(&at(&format!("k group {g}")), &k, &v("k", Some(l))[g * 8..g * 8 + 8]);
                compare(&at(&format!("v group {g}")), &vv, &v("v", Some(l))[g * 8..g * 8 + 8]);
            },
            "CK_KV" => {
                let addr = ((l * 4 + g) * 256 + pos % 256) * 18;
                let e: Vec<i64> = fw.sram(addr, 18).iter().map(|b| *b as i8 as i64).collect();
                compare(&at(&format!("k_a group {g}")), &e[0..8], &v("k_a", Some(l))[g * 8..g * 8 + 8]);
                compare(&at(&format!("k_E group {g}")), &e[8..9], &v("k_E", Some(l))[g..g + 1]);
                compare(&at(&format!("v_a group {g}")), &e[9..17], &v("v_a", Some(l))[g * 8..g * 8 + 8]);
                compare(&at(&format!("v_E group {g}")), &e[17..18], &v("v_E", Some(l))[g..g + 1]);
            },
            "CK_ATTOUT" => compare(&at("att_out"), &s24(&fw.sram(O_AREA, 192)), &v("att_out", Some(l))),
            "CK_XATT" => compare(&at("x_att"), &fw.x(), &v("x_att", Some(l))),
            "CK_HB" => compare(&at("hb"), &s24(&fw.sram(HB_AREA, 516)), &v("hb", Some(l))),
            "CK_XFFN" => compare(&at("x_ffn"), &fw.x(), &v("x_ffn", Some(l))),
            "CK_LOGITS" => compare(&format!("pos {pos} logits"), &s24(&fw.sram(LG_AREA, 1536)), &v("logits", None)),
            "CK_TOKEN" => {
                let next = fw.ram(0x69) as i64 | (fw.ram(0x6A) as i64) << 8;
                compare(&format!("pos {pos} sampled"), &[next], &v("sampled", None));
                eprintln!("pos {pos}: token {next} at {} ms", fw.board.now() / MS);
                pos += 1;
            },
            _ => unreachable!()
        }
    }
}

// Runs the firmware for TS_TOKENS tokens (default 40) with seed TS_SEED
// (default 1), prints the tokens and saves the LCD as a PPM image to
// TS_SCREEN (default screen.ppm).
// cargo test --release tinystories_story -- --ignored --nocapture
#[test]
#[ignore]
fn tinystories_story() {
    let env = |name: &str, default: u32| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
    let Some(mut fw) = Fw::new(env("TS_SEED", 1)) else { return };
    let tokens = env("TS_TOKENS", 40);
    let check = fw.sym("CK_TOKEN");
    fw.board.add_breakpoint(fw.pic, check);
    fw.board.init();
    let mut got = vec![];
    while got.len() < tokens as usize {
        if fw.board.step() && fw.pc() - 1 == check {
            let next = fw.ram(0x69) as u32 | (fw.ram(0x6A) as u32) << 8;
            got.push(next);
            eprintln!("{} ms: token {next}", fw.board.now() / MS);
        }
    }
    // Let the last token's text reach the screen.
    let end = fw.board.now() + 400 * MS;
    while fw.board.now() < end {
        fw.board.step();
    }
    println!("tokens: {got:?}");
    let lcd = fw.board.find_component("J1").unwrap();
    let display = fw.board.get_component(lcd).unwrap().as_display_mut().unwrap();
    let (w, h) = display.size();
    let mut pixels = vec![0u32; w * h];
    display.render(&mut pixels);
    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    ppm.extend(pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]));
    let path = std::env::var("TS_SCREEN").unwrap_or("screen.ppm".to_string());
    std::fs::write(&path, ppm).unwrap();
    println!("screen: {path}");
}
