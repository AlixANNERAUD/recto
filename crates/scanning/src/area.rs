use serde::{Deserialize, Serialize};

use crate::{
    error::ScanError,
    model::{Area, ScanMode, ScannerCapabilities},
};

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

/// The most raw image data one page may take, in bytes. A page is held whole while it is
/// encoded, so this bounds the memory of a scan: 300 MB is an A4 page in colour at about
/// 1000 dpi, or in gray at about 1700 dpi, far beyond what documents need.
pub const MAX_PAGE_BYTES: u64 = 300_000_000;

impl ScanSettings {
    /// The raw size of one page at these settings, when the area is known: one byte per sample,
    /// three samples per pixel in colour (line art is expanded to a byte per pixel too).
    pub fn page_bytes(&self) -> Option<u64> {
        let area = self.area?;
        // Areas are at most a few hundred millimetres, far inside u64.
        let pixels = |mm: f64| (mm.max(0.0) / 25.4 * f64::from(self.resolution)).ceil() as u64;
        let samples = if self.mode == ScanMode::Color { 3 } else { 1 };
        Some(
            pixels(area.width())
                .saturating_mul(pixels(area.height()))
                .saturating_mul(samples),
        )
    }
}

/// Refuses a page of `bytes` raw bytes above [`MAX_PAGE_BYTES`].
pub fn check_page_bytes(bytes: u64) -> Result<(), ScanError> {
    if bytes > MAX_PAGE_BYTES {
        Err(ScanError::TooLarge)
    } else {
        Ok(())
    }
}

/// Settings for the quick preview from `source` (the device's current one when `None`): low
/// resolution keeps it fast.
pub fn preview_settings(caps: &ScannerCapabilities, source: Option<String>) -> ScanSettings {
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
        source,
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
        let preview = |resolutions: &[u32]| preview_settings(&caps(resolutions), None);
        assert_eq!(preview(&[50, 75, 300]).resolution, 75);
        assert_eq!(preview(&[300, 600]).resolution, 300);
        assert_eq!(preview(&[50, 60]).resolution, 50);
        let s = preview(&[75]);
        assert_eq!(s.mode, ScanMode::Color);
        assert_eq!(s.source, None);
    }

    #[test]
    fn preview_uses_the_chosen_source() {
        let s = preview_settings(&caps(&[75]), Some("Flatbed".into()));
        assert_eq!(s.source.as_deref(), Some("Flatbed"));
    }

    #[test]
    fn preview_falls_back_to_first_mode() {
        let mut c = caps(&[75]);
        c.modes = vec![ScanMode::Gray, ScanMode::Lineart];
        assert_eq!(preview_settings(&c, None).mode, ScanMode::Gray);
    }

    #[test]
    fn page_size_follows_area_resolution_and_mode() {
        let a4 = ScanSettings {
            resolution: 300,
            mode: ScanMode::Color,
            source: None,
            area: Some(Area {
                left: 0.0,
                top: 0.0,
                right: 210.0,
                bottom: 297.0,
            }),
        };
        // 2481 x 3508 pixels.
        assert_eq!(a4.page_bytes(), Some(2481 * 3508 * 3));
        let gray = ScanSettings {
            mode: ScanMode::Gray,
            ..a4.clone()
        };
        assert_eq!(gray.page_bytes(), Some(2481 * 3508));
        assert_eq!(ScanSettings { area: None, ..a4 }.page_bytes(), None);
        assert!(check_page_bytes(MAX_PAGE_BYTES).is_ok());
        assert_eq!(
            check_page_bytes(MAX_PAGE_BYTES + 1),
            Err(ScanError::TooLarge)
        );
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
