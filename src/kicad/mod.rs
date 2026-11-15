// Builds a board from a KiCad schematic or netlist. Parts with an Emu field
// are emulated, everything else (resistors, capacitors, crystals, ...) is
// left out. Nets keep their KiCad names; power nets (GND, +5V, ...) get a
// constant driver; nets called NAME0..NAMEn are also traced as bus NAME.
pub mod sexpr;
pub mod netlist;

use std::collections::{ HashMap, HashSet };
use std::error::Error;
use std::path::Path;

use crate::board::Board;
use crate::kicad::netlist::Netlist;
use crate::loader::{ Report, add_net, group_buses };
use crate::parts::PartSpec;

// Matches a symbol pin name such as "INT/RB0" or "~{MCLR}/Vpp" against the
// model's pins.
fn match_pin(function: &str, pins: &[&'static str]) -> Option<&'static str> {
    function.split('/').map(|name| name.replace("~{", "").replace('}', ""))
        .find_map(|name| pins.iter().find(|pin| **pin == name).copied())
}

pub fn load(board: &mut Board, path: &Path) -> Result<Report, Box<dyn Error>> {
    let netlist = netlist::read(path)?;
    build(board, &netlist, path.parent().unwrap_or(Path::new(".")))
}

// Paths in Emu fields are relative to dir.
pub fn build(board: &mut Board, netlist: &Netlist, dir: &Path) -> Result<Report, Box<dyn Error>> {
    let mut report = Report::default();

    // reference -> (pinout, component pins)
    let mut parts = HashMap::new();
    for part in &netlist.parts {
        let Some(field) = part.field("Emu").filter(|field| !field.trim().is_empty()) else {
            report.ignored.push(part.reference.clone());
            continue;
        };
        let spec = PartSpec::parse(field).map_err(|err| format!("{}: {err}", part.reference))?;
        let model = spec.build(dir).map_err(|err| format!("{}: {err}", part.reference))?;
        parts.insert(part.reference.as_str(), (model.pinout, model.component.pins().to_vec()));
        let bc_idx = board.add_component(&part.reference, model.component);
        if let Some(screen) = model.screen {
            board.add_screen(bc_idx, screen);
        }
        report.emulated.push((part.reference.clone(), spec.model));
    }

    let mut connected = HashSet::new();
    let mut net_names = vec![];
    for net in &netlist.nets {
        // KiCad gives each pin with a no-connect flag a net of its own.
        if net.name.starts_with("unconnected-") {
            continue;
        }
        let name = net.name.strip_prefix('/').unwrap_or(&net.name);
        let mut pins = vec![];
        for node in &net.nodes {
            let Some((pinout, model_pins)) = parts.get(node.reference.as_str()) else {
                continue;
            };
            let pin = match pinout {
                Some(pinout) => pinout.get(&node.pin).map(String::as_str),
                None => node.function.as_deref().and_then(|function| match_pin(function, model_pins))
            };
            let Some(pin) = pin else {
                continue;
            };
            let pin = format!("{}.{pin}", node.reference);
            if !connected.insert(pin.clone()) {
                return Err(format!("{pin} is on more than one net (last: {name})").into());
            }
            pins.push(pin);
        }
        if pins.is_empty() {
            continue;
        }

        add_net(board, &mut report, name, pins);
        net_names.push(name.to_string());
    }
    group_buses(board, &mut report, &net_names);

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board_test::SharedBuf;
    use crate::common::net::Level;

    // U1 is matched by pin name, J2 through its pins= map.
    fn netlist(j2_pins: &str) -> String {
        format!(r#"(export (version "E")
  (components
    (comp (ref "R1") (fields (field (name "Footprint"))))
    (comp (ref "J2") (fields (field (name "Emu") "74C922 pins=\"{j2_pins}\"")))
    (comp (ref "U1") (fields (field (name "Emu") "PIC16F876 rom=boards/tetris/tetris.hex"))))
  (nets
    (net (code "1") (name "+5V")
      (node (ref "J2") (pin "5") (pinfunction "Pin_5"))
      (node (ref "R1") (pin "1")))
    (net (code "2") (name "GND")
      (node (ref "U1") (pin "8") (pinfunction "VSS")))
    (net (code "3") (name "/D0")
      (node (ref "J2") (pin "1") (pinfunction "Pin_1"))
      (node (ref "U1") (pin "21") (pinfunction "INT/RB0")))
    (net (code "4") (name "/D1")
      (node (ref "J2") (pin "2") (pinfunction "Pin_2"))
      (node (ref "U1") (pin "22") (pinfunction "RB1")))
    (net (code "5") (name "/X0")
      (node (ref "U1") (pin "11") (pinfunction "RC0/T1OSO/T1CKI")))
    (net (code "6") (name "/X2")
      (node (ref "U1") (pin "13") (pinfunction "RC2/CCP1")))
    (net (code "7") (name "/MCLR")
      (node (ref "U1") (pin "1") (pinfunction "~{{MCLR}}/Vpp")))
    (net (code "8") (name "unconnected-(U1-RC1-Pad12)")
      (node (ref "U1") (pin "12") (pinfunction "RC1/T1OSI/CCP2")))))
"#)
    }

    fn load_text(board: &mut Board, text: &str) -> Result<Report, Box<dyn Error>> {
        build(board, &netlist::parse(text).unwrap(), Path::new("."))
    }

    #[test]
    fn loads_parts_nets_rails_and_buses() {
        let mut board = Board::new();
        let report = load_text(&mut board, &netlist("1:D0 2:D1 5:DA")).unwrap();
        assert_eq!(report.to_string(), "\
Emulated: J2 (74C922), U1 (PIC16F876)
Not emulated: R1
Power rails: +5V (1)
Buses: D
Nets with one emulated pin: X0, X2");
        // VSS isn't an emulated pin, so GND has nothing to drive.
        assert_eq!(board.net_level("GND"), None);
        assert_eq!(board.net_level("MCLR"), None);
        assert_eq!(board.find_component("+5V").is_some(), true);

        let buf = SharedBuf::default();
        board.trace(Box::new(buf.clone()), &["D", "+5V"]).unwrap();
        assert!(board.trace(Box::new(SharedBuf::default()), &["X"]).is_err(), "X0, X2 have a gap");
        board.init();
        board.step();
        assert_eq!(board.net_level("+5V"), Some(Level::High));
        assert!(buf.text().contains("$var wire 2 ! D [1:0] $end"), "{}", buf.text());
    }

    #[test]
    fn reports_bad_parts() {
        let mut board = Board::new();
        let err = load_text(&mut board, &netlist("1:D0 2:D0")).err().unwrap();
        assert_eq!(err.to_string(), "J2.D0 is on more than one net (last: D1)");

        let mut board = Board::new();
        let err = load_text(&mut board, &netlist("1:Q")).err().unwrap();
        assert_eq!(err.to_string(), "J2: 74C922 has no pin Q");

        let mut board = Board::new();
        let text = netlist("1:D0").replace("PIC16F876 rom=boards/tetris/tetris.hex", "PIC16F876 rom=missing.hex");
        let err = load_text(&mut board, &text).err().unwrap().to_string();
    assert!(err.starts_with("U1: ./missing.hex: "), "{err}");
    }

    #[test]
    fn matches_pins_by_name() {
        assert_eq!(match_pin("INT/RB0", &["RB0", "RB1"]), Some("RB0"));
        assert_eq!(match_pin("~{SS}/AN4/RA5", &["RA5"]), Some("RA5"));
        assert_eq!(match_pin("VSS", &["RA5"]), None);
    }
}
