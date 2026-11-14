use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannerInfo {
    pub id: String,
    pub vendor: String,
    pub model: String,
    pub kind: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanMode {
    Color,
    Gray,
    Lineart,
}

impl ScanMode {
    /// SANE spells modes inconsistently across backends (`Color`, `color`, `COLOR`).
    pub fn from_sane(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "color" => Some(Self::Color),
            "gray" => Some(Self::Gray),
            "lineart" => Some(Self::Lineart),
            _ => None,
        }
    }

    /// The canonical SANE name. Callers setting the option should pick the device's own spelling
    /// from its string list with `matches`, since some backends are case sensitive.
    pub fn sane_name(self) -> &'static str {
        match self {
            Self::Color => "Color",
            Self::Gray => "Gray",
            Self::Lineart => "Lineart",
        }
    }

    pub fn matches(self, device_name: &str) -> bool {
        Self::from_sane(device_name) == Some(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputFormat {
    Pdf,
    Png,
    Jpeg,
}

impl OutputFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Pdf => "application/pdf",
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub name: String,
    pub is_feeder: bool,
}

impl Source {
    pub fn new(name: &str) -> Self {
        let lower = name.to_ascii_lowercase();
        let is_feeder = ["adf", "feeder", "duplex"]
            .iter()
            .any(|k| lower.contains(k));
        Self {
            name: name.to_owned(),
            is_feeder,
        }
    }
}

/// A scan window in millimetres from the top-left corner of the glass.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Area {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Area {
    pub fn width(&self) -> f64 {
        self.right - self.left
    }

    pub fn height(&self) -> f64 {
        self.bottom - self.top
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScannerCapabilities {
    pub resolutions: Vec<u32>,
    pub modes: Vec<ScanMode>,
    pub sources: Vec<Source>,
    pub area: Option<Area>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_match_case_insensitively() {
        assert_eq!(ScanMode::from_sane("COLOR"), Some(ScanMode::Color));
        assert_eq!(ScanMode::from_sane("lineart"), Some(ScanMode::Lineart));
        assert_eq!(ScanMode::from_sane("Halftone"), None);
        assert!(ScanMode::Gray.matches("gray"));
    }

    #[test]
    fn feeders_are_detected_by_name() {
        assert!(Source::new("ADF Duplex").is_feeder);
        assert!(Source::new("Automatic Document Feeder").is_feeder);
        assert!(Source::new("duplex").is_feeder);
        assert!(!Source::new("Flatbed").is_feeder);
    }
}
