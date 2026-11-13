// Loads a board from a file: a TOML board file (.toml, see boardfile.rs) or
// a KiCad schematic or netlist (.kicad_sch, .net, see kicad/). Both give
// the same kind of board: parts, nets, power rails and buses.
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::Path;

use crate::board::Board;
use crate::common::net::Level;
use crate::hw::rail::Rail;

// What a loader made of a board file.
#[derive(Default)]
pub struct Report {
    // (name, model)
    pub emulated: Vec<(String, String)>,
    // Parts that are drawing only (KiCad).
    pub ignored: Vec<String>,
    pub rails: Vec<(String, Level)>,
    pub buses: Vec<String>,
    // Nets with a single emulated pin: nothing to talk to in the emulator.
    pub lone_nets: Vec<String>
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let emulated: Vec<String> = self.emulated.iter().map(|(name, model)| format!("{name} ({model})")).collect();
        let rails: Vec<String> = self.rails.iter().map(|(net, level)| format!("{net} ({})", if *level == Level::High { 1 } else { 0 })).collect();
        writeln!(f, "Emulated: {}", emulated.join(", "))?;
        writeln!(f, "Not emulated: {}", self.ignored.join(", "))?;
        writeln!(f, "Power rails: {}", rails.join(", "))?;
        writeln!(f, "Buses: {}", self.buses.join(", "))?;
        write!(f, "Nets with one emulated pin: {}", self.lone_nets.join(", "))
    }
}

pub fn load(board: &mut Board, path: &Path) -> Result<Report, Box<dyn Error>> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("toml") => crate::boardfile::load(board, path),
        Some("kicad_sch") | Some("net") => crate::kicad::load(board, path),
        _ => Err(format!("{}: expected a .toml board file or a KiCad .kicad_sch/.net", path.display()).into())
    }
}

// Level of a power net, by its name: GND*, VSS and 0V are 0; +*, VCC and
// VDD are 1.
pub fn rail_level(net: &str) -> Option<Level> {
    if net.starts_with("GND") || net == "VSS" || net == "0V" {
        Some(Level::Low)
    } else if net.starts_with('+') || net == "VCC" || net == "VDD" {
        Some(Level::High)
    } else {
        None
    }
}

// Splits "LCD_DB7" into ("LCD_DB", 7).
pub fn bus_member(net: &str) -> Option<(&str, usize)> {
    let prefix = net.trim_end_matches(|c: char| c.is_ascii_digit());
    if prefix.is_empty() || prefix.len() == net.len() {
        return None;
    }
    Some((prefix, net[prefix.len()..].parse().ok()?))
}

// Adds a net of pins ("PART.PIN"), with a constant driver if it's a power
// net; records rails and lone nets in report.
pub fn add_net(board: &mut Board, report: &mut Report, name: &str, mut pins: Vec<String>) {
    if let Some(level) = rail_level(name) {
        board.add_component(name, Box::new(Rail::new(level)));
        pins.push(format!("{name}.OUT"));
        report.rails.push((name.to_string(), level));
    } else if pins.len() == 1 {
        report.lone_nets.push(name.to_string());
    }
    let pins: Vec<&str> = pins.iter().map(String::as_str).collect();
    board.net(name, &pins);
}

// Groups nets NAME0..NAMEn, without gaps, into bus NAME.
pub fn group_buses(board: &mut Board, report: &mut Report, net_names: &[String]) {
    let mut buses: BTreeMap<&str, Vec<(usize, &str)>> = BTreeMap::new();
    for net in net_names {
        if let Some((prefix, idx)) = bus_member(net) {
            buses.entry(prefix).or_default().push((idx, net));
        }
    }
    for (prefix, mut members) in buses {
        members.sort_unstable();
        if members.len() < 2 || members.iter().enumerate().any(|(i, (idx, _))| i != *idx) {
            continue;
        }
        let names: Vec<&str> = members.iter().map(|(_, net)| *net).collect();
        board.group_bus(prefix, &names);
        report.buses.push(prefix.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(bus_member("LCD_DB7"), Some(("LCD_DB", 7)));
        assert_eq!(bus_member("LCD_E"), None);
        assert_eq!(bus_member("42"), None);
        assert_eq!(rail_level("+5V"), Some(Level::High));
        assert_eq!(rail_level("GND"), Some(Level::Low));
        assert_eq!(rail_level("RAM_CS"), None);
    }
}
