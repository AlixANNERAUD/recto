use serde::{Deserialize, Serialize};

use crate::model::{Area, ScanMode, ScannerCapabilities};

/// A rectangle on the preview as fractions of its width and height, from the top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Selection {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Below this a drag is a stray click, not a selection.
const MIN_FRACTION: f64 = 0.01;
/// Above this on both sides the selection is the whole page, so the device default is better.
const FULL_FRACTION: f64 = 0.99;

impl Selection {
    /// `None` means "scan the full area".
    pub fn to_area(&self, full: &Area) -> Option<Area> {
        let x0 = self.x.clamp(0.0, 1.0);
        let y0 = self.y.clamp(0.0, 1.0);
        let x1 = (self.x + self.width).clamp(0.0, 1.0);
        let y1 = (self.y + self.height).clamp(0.0, 1.0);
        let (w, h) = (x1 - x0, y1 - y0);
        if w.is_nan() || h.is_nan() || w < MIN_FRACTION || h < MIN_FRACTION {
            return None;
        }
        if w >= FULL_FRACTION && h >= FULL_FRACTION {
            return None;
        }
        Some(Area {
            left: full.left + x0 * full.width(),
            top: full.top + y0 * full.height(),
            right: full.left + x1 * full.width(),
            bottom: full.top + y1 * full.height(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanSettings {
    pub resolution: u32,
    pub mode: ScanMode,
    pub source: Option<String>,
    pub area: Option<Area>,
}

/// Settings for the quick preview: low resolution keeps it fast, and the source is left alone so
/// a preview does not flip the feeder/flatbed choice made on the device.
pub fn preview_settings(caps: &ScannerCapabilities) -> ScanSettings {
    let resolution = caps
        .resolutions
        .iter()
        .copied()
        .filter(|r| *r >= 75)
        .min()
        .or_else(|| caps.resolutions.iter().copied().min())
        .unwrap_or(75);
    let mode = if caps.modes.contains(&ScanMode::Color) {
        ScanMode::Color
    } else {
        caps.modes.first().copied().unwrap_or(ScanMode::Color)
    };
    ScanSettings {
        resolution,
        mode,
        source: None,
        area: caps.area,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(resolutions: &[u32]) -> ScannerCapabilities {
        ScannerCapabilities {
            resolutions: resolutions.to_vec(),
            modes: vec![ScanMode::Gray, ScanMode::Color],
            sources: vec![],
            area: None,
        }
    }

    fn page() -> Area {
        Area {
            left: 0.0,
            top: 0.0,
            right: 200.0,
            bottom: 300.0,
        }
    }

    #[test]
    fn preview_picks_low_resolution() {
        assert_eq!(preview_settings(&caps(&[50, 75, 300])).resolution, 75);
        assert_eq!(preview_settings(&caps(&[300, 600])).resolution, 300);
        assert_eq!(preview_settings(&caps(&[50, 60])).resolution, 50);
        let s = preview_settings(&caps(&[75]));
        assert_eq!(s.mode, ScanMode::Color);
        assert_eq!(s.source, None);
    }

    #[test]
    fn preview_falls_back_to_first_mode() {
        let mut c = caps(&[75]);
        c.modes = vec![ScanMode::Gray, ScanMode::Lineart];
        assert_eq!(preview_settings(&c).mode, ScanMode::Gray);
    }

    #[test]
    fn selection_maps_to_area() {
        let s = Selection {
            x: 0.5,
            y: 0.25,
            width: 0.5,
            height: 0.5,
        };
        assert_eq!(
            s.to_area(&page()),
            Some(Area {
                left: 100.0,
                top: 75.0,
                right: 200.0,
                bottom: 225.0
            })
        );
    }

    #[test]
    fn tiny_or_full_selection_is_none() {
        let tiny = Selection {
            x: 0.2,
            y: 0.2,
            width: 0.005,
            height: 0.5,
        };
        assert_eq!(tiny.to_area(&page()), None);
        let full = Selection {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 0.995,
        };
        assert_eq!(full.to_area(&page()), None);
        // Full on one side only is a real strip.
        let strip = Selection {
            x: 0.0,
            y: 0.2,
            width: 1.0,
            height: 0.3,
        };
        assert!(strip.to_area(&page()).is_some());
    }

    #[test]
    fn selection_is_clamped() {
        let s = Selection {
            x: -0.5,
            y: 0.5,
            width: 1.0,
            height: 2.0,
        };
        assert_eq!(
            s.to_area(&page()),
            Some(Area {
                left: 0.0,
                top: 150.0,
                right: 100.0,
                bottom: 300.0
            })
        );
        let outside = Selection {
            x: 1.5,
            y: 0.0,
            width: 0.5,
            height: 0.5,
        };
        assert_eq!(outside.to_area(&page()), None);
    }
}
