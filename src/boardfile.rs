// Board files: a board as TOML, loaded at run time.
//
//     name = "Tetris"                  # optional, shown by VNC viewers
//
//     [parts]
//     U1 = { model = "PIC16F876", rom = "../../TETRIS.hex", fosc = "4MHz" }
//     J1 = { model = "ST7920", rotate = 270, vnc = 5900 }
//     J2 = { model = "74C922", hold = "130ms", keys = { a = 3, space = 5 } }
//
//     [nets]
//     LCD_E = ["U1.RA2", "J1.E"]
//     "LCD_DB0..7" = ["U1.RC0..7", "J1.DB0..7"]
//
// A part's keys other than model are the model's parameters (see
// parts.rs); paths are relative to the board file. Pins are PART.PIN with
// the model's pin names. A range A..B makes one net per number: LCD_DB0
// connects U1.RC0 and J1.DB0, and so on. As with KiCad schematics, nets
// called GND*, VSS, 0V, +*, VCC or VDD are power rails, and nets NAME0..n
// are traced as bus NAME.
use std::collections::HashSet;
use std::error::Error;
use std::path::Path;

use toml::{ Table, Value };

use crate::board::Board;
use crate::loader::{ Report, add_net, group_buses };
use crate::parts::PartSpec;

pub fn load(board: &mut Board, path: &Path) -> Result<Report, Box<dyn Error>> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    build(board, &text, path.parent().unwrap_or(Path::new(".")))
        .map_err(|err| format!("{}: {err}", path.display()).into())
}

// A parameter value as the part models take it: a table { a = 3 } becomes
// "a:3", an array its items separated by spaces.
fn param_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Table(table) => table.iter().map(|(key, value)| format!("{key}:{}", param_text(value)))
            .collect::<Vec<_>>().join(" "),
        Value::Array(items) => items.iter().map(param_text).collect::<Vec<_>>().join(" "),
        value => value.to_string()
    }
}

// "U1.RC0..7" -> ["U1.RC0", ..., "U1.RC7"]; "U1.RA0" -> ["U1.RA0"].
// Ranges may count down: "D7..0".
fn expand(text: &str) -> Result<Vec<String>, String> {
    let Some((head, last)) = text.split_once("..") else {
        return Ok(vec![text.to_string()]);
    };
    let prefix = head.trim_end_matches(|c: char| c.is_ascii_digit());
    let first: usize = head[prefix.len()..].parse().map_err(|_| format!("{text}: expected NAME<first>..<last>"))?;
    let last: usize = last.parse().map_err(|_| format!("{text}: expected NAME<first>..<last>"))?;
    let numbers: Vec<usize> = if first <= last { (first..=last).collect() } else { (last..=first).rev().collect() };
    Ok(numbers.iter().map(|i| format!("{prefix}{i}")).collect())
}

pub fn build(board: &mut Board, text: &str, dir: &Path) -> Result<Report, Box<dyn Error>> {
    let doc: Table = text.parse()?;
    let mut report = Report::default();
    if let Some(key) = doc.keys().find(|key| !["name", "parts", "nets"].contains(&key.as_str())) {
        return Err(format!("unknown key {key} (expected name, parts, nets)").into());
    }
    if let Some(name) = doc.get("name") {
        board.set_name(name.as_str().ok_or("name must be a string")?);
    }

    let parts = doc.get("parts").and_then(Value::as_table).ok_or("no [parts] table")?;
    for (name, part) in parts {
        let part = part.as_table().ok_or_else(|| format!("part {name} must be a table"))?;
        let model = part.get("model").and_then(Value::as_str).ok_or_else(|| format!("part {name} needs model = \"...\""))?;
        let params = part.iter().filter(|(key, _)| *key != "model")
            .map(|(key, value)| (key.clone(), param_text(value))).collect();
        let spec = PartSpec::new(model, params);
        let built = spec.build(dir).map_err(|err| format!("{name}: {err}"))?;
        let bc_idx = board.add_component(name, built.component);
        if let Some(screen) = built.screen {
            board.add_screen(bc_idx, screen);
        }
        report.emulated.push((name.clone(), model.to_string()));
    }

    let empty = Table::new();
    let nets = doc.get("nets").map(|nets| nets.as_table().ok_or("nets must be a table")).transpose()?.unwrap_or(&empty);
    let mut connected = HashSet::new();
    let mut net_names = vec![];
    for (net, pins) in nets {
        let names = expand(net)?;
        let pins = pins.as_array().ok_or_else(|| format!("net {net}: expected a list of pins"))?;
        // columns[j] is the pin of entry j on each of the nets.
        let mut columns = vec![];
        for pin in pins {
            let pin = pin.as_str().ok_or_else(|| format!("net {net}: pins are strings like \"U1.RA0\""))?;
            let expanded = expand(pin)?;
            if expanded.len() != names.len() {
                return Err(format!("net {net}: {pin} has {} pins for {} nets", expanded.len(), names.len()).into());
            }
            for pin in &expanded {
                let (part, pin_name) = pin.split_once('.').ok_or_else(|| format!("net {net}: {pin} is not PART.PIN"))?;
                let bc_idx = board.find_component(part).ok_or_else(|| format!("net {net}: no part {part}"))?;
                let model_pins = board.get_component(bc_idx)?.pins();
                if !model_pins.contains(&pin_name) {
                    return Err(format!("net {net}: {part} has no pin {pin_name} (pins: {})", model_pins.join(" ")).into());
                }
                if !connected.insert(pin.clone()) {
                    return Err(format!("{pin} is on more than one net (last: {net})").into());
                }
            }
            columns.push(expanded);
        }
        for (i, name) in names.iter().enumerate() {
            add_net(board, &mut report, name, columns.iter().map(|column| column[i].clone()).collect());
            net_names.push(name.clone());
        }
    }
    group_buses(board, &mut report, &net_names);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::net::Level;

    const BOARD: &str = r#"
        name = "Test"
        [parts]
        U1 = { model = "PIC16F876", rom = "TETRIS.hex", fosc = "4MHz" }
        J2 = { model = "74C922", keys = { a = 3, space = 5 } }
        [nets]
        "+5V" = ["J2.DA"]
        "D0..1" = ["U1.RB0..1", "J2.D0..1"]
        X0 = ["U1.RC0"]
    "#;

    fn load_text(text: &str) -> Result<(Board, Report), Box<dyn Error>> {
        let mut board = Board::new();
        let report = build(&mut board, text, Path::new("."))?;
        Ok((board, report))
    }

    #[test]
    fn expands_ranges() {
        assert_eq!(expand("U1.RC0..2").unwrap(), ["U1.RC0", "U1.RC1", "U1.RC2"]);
        assert_eq!(expand("D3..1").unwrap(), ["D3", "D2", "D1"]);
        assert_eq!(expand("U1.RA0").unwrap(), ["U1.RA0"]);
        assert!(expand("D..3").is_err());
    }

    #[test]
    fn converts_params() {
        let table: Table = "keys = { a = 3, space = 5 }\nfosc = 4000000\nlist = [1, \"b\"]".parse().unwrap();
        assert_eq!(param_text(&table["keys"]), "a:3 space:5");
        assert_eq!(param_text(&table["fosc"]), "4000000");
        assert_eq!(param_text(&table["list"]), "1 b");
    }

    #[test]
    fn loads_parts_nets_rails_and_buses() {
        let (mut board, report) = load_text(BOARD).unwrap();
        assert_eq!(report.to_string(), "\
Emulated: J2 (74C922), U1 (PIC16F876)
Not emulated: \nPower rails: +5V (1)
Buses: D
Nets with one emulated pin: X0");
        assert_eq!(board.pins_of("D1"), Some(vec!["U1.RB1".to_string(), "J2.D1".to_string()]));
        board.init();
        board.step();
        assert_eq!(board.net_level("+5V"), Some(Level::High));
    }

    // The board files in boards/ wire the same pins as their schematics
    // (needs kicad-cli).
    #[test]
    fn matches_kicad_schematics() {
        for name in ["tetris", "spiram"] {
            let nets = |file: &str| {
                let mut board = Board::new();
                crate::loader::load(&mut board, Path::new(&format!("boards/{name}/{name}.{file}"))).unwrap();
                let mut nets: Vec<(String, Vec<String>)> = board.net_names().into_iter()
                    .map(|net| { let mut pins = board.pins_of(&net).unwrap(); pins.sort(); (net, pins) })
                    .filter(|(_, pins)| pins.len() > 1).collect();
                nets.sort();
                nets
            };
            assert_eq!(nets("toml"), nets("kicad_sch"), "{name}");
        }
    }

    #[test]
    fn reports_mistakes() {
        let err = |text: &str| load_text(text).err().unwrap().to_string();
        assert_eq!(err(&BOARD.replace("X0 = [\"U1.RC0\"]", "X0 = [\"U1.RB0\"]")), "U1.RB0 is on more than one net (last: X0)");
        assert!(err(&BOARD.replace("U1.RC0", "U1.RC9")).starts_with("net X0: U1 has no pin RC9 (pins: "));
        assert_eq!(err(&BOARD.replace("U1.RC0", "U3.RC0")), "net X0: no part U3");
        assert_eq!(err(&BOARD.replace("J2.D0..1", "J2.D0..2")), "net D0..1: J2.D0..2 has 3 pins for 2 nets");
        assert_eq!(err(&BOARD.replace("model = \"74C922\"", "model = \"Z80\"")), "J2: unknown model Z80");
        assert_eq!(err(&BOARD.replace("[nets]", "[wires]")), "unknown key wires (expected name, parts, nets)");
    }
}
