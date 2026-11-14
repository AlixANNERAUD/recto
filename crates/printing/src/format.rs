//! Document formats recto accepts.

use serde::{Deserialize, Serialize};

/// The formats CUPS can print without a filter recto would have to ship.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocumentFormat {
    Pdf,
    Jpeg,
    Png,
}

impl DocumentFormat {
    /// Detects the format from the content, never from the file name or the declared type, so a
    /// renamed document is refused before it reaches CUPS.
    pub fn detect(bytes: &[u8]) -> Option<DocumentFormat> {
        if bytes.starts_with(b"%PDF-") {
            Some(DocumentFormat::Pdf)
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(DocumentFormat::Jpeg)
        } else if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
            Some(DocumentFormat::Png)
        } else {
            None
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            DocumentFormat::Pdf => "application/pdf",
            DocumentFormat::Jpeg => "image/jpeg",
            DocumentFormat::Png => "image/png",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_formats() {
        assert_eq!(
            DocumentFormat::detect(b"%PDF-1.7\n"),
            Some(DocumentFormat::Pdf)
        );
        assert_eq!(
            DocumentFormat::detect(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(DocumentFormat::Jpeg)
        );
        assert_eq!(
            DocumentFormat::detect(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0]),
            Some(DocumentFormat::Png)
        );
        assert_eq!(DocumentFormat::detect(b"PK\x03\x04docx"), None);
        assert_eq!(DocumentFormat::detect(b""), None);
        assert_eq!(DocumentFormat::Pdf.mime(), "application/pdf");
    }
}
