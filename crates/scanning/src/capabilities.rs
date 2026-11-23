use crate::model::{Area, ScanMode, ScannerCapabilities, Source};

/// recto's copy of a SANE option descriptor, so the interpretation below is testable without libsane.
#[derive(Debug, Clone, PartialEq)]
pub struct OptionInfo {
    pub name: String,
    pub unit: OptionUnit,
    pub constraint: OptionConstraint,
    pub active: bool,
    pub settable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionUnit {
    None,
    Pixel,
    Mm,
    Dpi,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OptionConstraint {
    None,
    IntRange { min: i32, max: i32, quant: i32 },
    FixedRange { min: f64, max: f64, quant: f64 },
    IntList(Vec<i32>),
    FixedList(Vec<f64>),
    StringList(Vec<String>),
}

const COMMON_RESOLUTIONS: [u32; 8] = [75, 100, 150, 200, 300, 600, 1200, 2400];

pub fn capabilities(options: &[OptionInfo]) -> ScannerCapabilities {
    // Inactive options exist in the descriptor list but the device ignores them right now.
    let find = |name: &str| options.iter().find(|o| o.name == name && o.active);
    ScannerCapabilities {
        resolutions: find("resolution")
            .map(|o| resolutions(&o.constraint))
            .unwrap_or_default(),
        modes: find("mode")
            .map(|o| strings(&o.constraint))
            .unwrap_or_default()
            .iter()
            .filter_map(|s| ScanMode::from_sane(s))
            .collect(),
        sources: find("source")
            .map(|o| strings(&o.constraint))
            .unwrap_or_default()
            .iter()
            .map(|s| Source::new(s))
            .collect(),
        area: area(&find("tl-x"), &find("tl-y"), &find("br-x"), &find("br-y")),
    }
}

fn strings(c: &OptionConstraint) -> Vec<String> {
    match c {
        OptionConstraint::StringList(v) => v.clone(),
        _ => Vec::new(),
    }
}

fn resolutions(c: &OptionConstraint) -> Vec<u32> {
    let positive = |v: i64| u32::try_from(v).ok().filter(|v| *v > 0);
    match c {
        OptionConstraint::IntList(v) => v.iter().filter_map(|v| positive(i64::from(*v))).collect(),
        OptionConstraint::FixedList(v) => v
            .iter()
            .filter_map(|v| positive(v.round() as i64))
            .collect(),
        OptionConstraint::IntRange { min, max, quant } => {
            snap(f64::from(*min), f64::from(*max), f64::from(*quant))
        }
        OptionConstraint::FixedRange { min, max, quant } => snap(*min, *max, *quant),
        _ => Vec::new(),
    }
}

/// A range can hold thousands of values; users only want the familiar steps, of those the range
/// holds: within it and on its steps (`quant`, 0 for any value), else the backend would round
/// the one asked for.
fn snap(min: f64, max: f64, quant: f64) -> Vec<u32> {
    let on_step = |r: f64| {
        if quant <= 0.0 {
            return true;
        }
        let steps = (r - min) / quant;
        (steps - steps.round()).abs() < 1e-6
    };
    let mut v: Vec<u32> = COMMON_RESOLUTIONS
        .iter()
        .copied()
        .filter(|r| f64::from(*r) >= min && f64::from(*r) <= max && on_step(f64::from(*r)))
        .collect();
    if v.is_empty() && min >= 1.0 {
        v.push(min.ceil() as u32);
    }
    v
}

fn area(
    tlx: &Option<&OptionInfo>,
    tly: &Option<&OptionInfo>,
    brx: &Option<&OptionInfo>,
    bry: &Option<&OptionInfo>,
) -> Option<Area> {
    // Pixel-based geometry cannot be mapped to the millimetre preview selection.
    let range = |o: &Option<&OptionInfo>| {
        let o = (*o)?;
        if !o.settable || o.unit != OptionUnit::Mm {
            return None;
        }
        match o.constraint {
            OptionConstraint::FixedRange { min, max, .. } => Some((min, max)),
            OptionConstraint::IntRange { min, max, .. } => Some((f64::from(min), f64::from(max))),
            _ => None,
        }
    };
    Some(Area {
        left: range(tlx)?.0,
        top: range(tly)?.0,
        right: range(brx)?.1,
        bottom: range(bry)?.1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(name: &str, unit: OptionUnit, constraint: OptionConstraint) -> OptionInfo {
        OptionInfo {
            name: name.into(),
            unit,
            constraint,
            active: true,
            settable: true,
        }
    }

    fn list(items: &[&str]) -> OptionConstraint {
        OptionConstraint::StringList(items.iter().map(|s| (*s).to_owned()).collect())
    }

    fn geometry(unit: OptionUnit) -> Vec<OptionInfo> {
        let r = |max| OptionConstraint::FixedRange {
            min: 0.0,
            max,
            quant: 0.0,
        };
        vec![
            opt("tl-x", unit, r(215.9)),
            opt("tl-y", unit, r(297.0)),
            opt("br-x", unit, r(215.9)),
            opt("br-y", unit, r(297.0)),
        ]
    }

    #[test]
    fn capabilities_from_list_and_range() {
        let mut options = vec![
            opt(
                "resolution",
                OptionUnit::Dpi,
                OptionConstraint::IntList(vec![75, 150, 300, 600]),
            ),
            opt(
                "mode",
                OptionUnit::None,
                list(&["Color", "Gray", "Lineart"]),
            ),
            opt("source", OptionUnit::None, list(&["Flatbed", "ADF"])),
        ];
        options.extend(geometry(OptionUnit::Mm));
        let caps = capabilities(&options);
        assert_eq!(caps.resolutions, [75, 150, 300, 600]);
        assert_eq!(
            caps.modes,
            [ScanMode::Color, ScanMode::Gray, ScanMode::Lineart]
        );
        assert_eq!(
            caps.sources,
            [
                Source {
                    name: "Flatbed".into(),
                    is_feeder: false
                },
                Source {
                    name: "ADF".into(),
                    is_feeder: true
                }
            ]
        );
        assert_eq!(
            caps.area,
            Some(Area {
                left: 0.0,
                top: 0.0,
                right: 215.9,
                bottom: 297.0
            })
        );

        let ranged = [opt(
            "resolution",
            OptionUnit::Dpi,
            OptionConstraint::IntRange {
                min: 50,
                max: 1200,
                quant: 1,
            },
        )];
        assert_eq!(
            capabilities(&ranged).resolutions,
            [75, 100, 150, 200, 300, 600, 1200]
        );
        let narrow = [opt(
            "resolution",
            OptionUnit::Dpi,
            OptionConstraint::FixedRange {
                min: 76.0,
                max: 90.0,
                quant: 1.0,
            },
        )];
        assert_eq!(capabilities(&narrow).resolutions, [76]);
        // Steps of 50 from 50: 75 is not one the backend would scan at.
        let stepped = [opt(
            "resolution",
            OptionUnit::Dpi,
            OptionConstraint::IntRange {
                min: 50,
                max: 1200,
                quant: 50,
            },
        )];
        assert_eq!(
            capabilities(&stepped).resolutions,
            [100, 150, 200, 300, 600, 1200]
        );
    }

    #[test]
    fn no_area_without_geometry_or_in_pixels() {
        assert_eq!(capabilities(&[]).area, None);
        assert_eq!(capabilities(&geometry(OptionUnit::Pixel)).area, None);
        let mut partial = geometry(OptionUnit::Mm);
        partial.pop();
        assert_eq!(capabilities(&partial).area, None);
        let mut locked = geometry(OptionUnit::Mm);
        locked[0].settable = false;
        assert_eq!(capabilities(&locked).area, None);
    }

    #[test]
    fn inactive_options_ignored() {
        let mut options = vec![
            opt(
                "resolution",
                OptionUnit::Dpi,
                OptionConstraint::IntList(vec![300]),
            ),
            opt("mode", OptionUnit::None, list(&["Color"])),
            opt("source", OptionUnit::None, list(&["Flatbed"])),
        ];
        options.extend(geometry(OptionUnit::Mm));
        for o in &mut options {
            o.active = false;
        }
        let caps = capabilities(&options);
        assert!(caps.resolutions.is_empty() && caps.modes.is_empty() && caps.sources.is_empty());
        assert_eq!(caps.area, None);
    }

    #[test]
    fn unknown_modes_are_dropped() {
        let options = [opt("mode", OptionUnit::None, list(&["Halftone", "gray"]))];
        assert_eq!(capabilities(&options).modes, [ScanMode::Gray]);
    }
}
