use std::io::Cursor;

use crate::{error::ScanError, model::OutputFormat, pdf};
use image::{DynamicImage, ImageFormat, codecs::jpeg::JpegEncoder};

#[derive(Debug)]
pub struct ScannedPage {
    pub image: DynamicImage,
    pub dpi: u32,
}

const JPEG_QUALITY: u8 = 85;

fn encoding(e: impl std::fmt::Display) -> ScanError {
    ScanError::Encoding(e.to_string())
}

/// JPEG has no alpha and no 16-bit mode, so normalise to 8-bit gray or RGB first.
fn jpeg(image: &DynamicImage) -> Result<(Vec<u8>, bool), ScanError> {
    let gray = !image.color().has_color();
    let mut buf = Vec::new();
    let encoder = JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY);
    if gray {
        image.to_luma8().write_with_encoder(encoder)
    } else {
        image.to_rgb8().write_with_encoder(encoder)
    }
    .map_err(encoding)?;
    Ok((buf, gray))
}

/// PNG and JPEG hold a single image, so only the first page is written.
pub fn encode(pages: &[ScannedPage], format: OutputFormat) -> Result<Vec<u8>, ScanError> {
    let first = pages
        .first()
        .ok_or_else(|| encoding("no pages to encode"))?;
    match format {
        OutputFormat::Png => {
            let mut out = Cursor::new(Vec::new());
            first
                .image
                .write_to(&mut out, ImageFormat::Png)
                .map_err(encoding)?;
            Ok(out.into_inner())
        }
        OutputFormat::Jpeg => Ok(jpeg(&first.image)?.0),
        OutputFormat::Pdf => {
            let pages = pages
                .iter()
                .map(|p| {
                    let (jpeg, gray) = jpeg(&p.image)?;
                    Ok(pdf::PdfPage {
                        jpeg,
                        width_px: p.image.width(),
                        height_px: p.image.height(),
                        gray,
                        dpi: p.dpi,
                    })
                })
                .collect::<Result<Vec<_>, ScanError>>()?;
            Ok(pdf::write(&pages))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, RgbImage};

    fn rgb(w: u32, h: u32, dpi: u32) -> ScannedPage {
        ScannedPage {
            image: DynamicImage::ImageRgb8(RgbImage::new(w, h)),
            dpi,
        }
    }

    fn count(hay: &[u8], needle: &[u8]) -> usize {
        hay.windows(needle.len()).filter(|w| *w == needle).count()
    }

    #[test]
    fn encodes_png_and_jpeg() {
        let pages = [rgb(40, 30, 150), rgb(10, 10, 150)];
        for (format, kind) in [
            (OutputFormat::Png, ImageFormat::Png),
            (OutputFormat::Jpeg, ImageFormat::Jpeg),
        ] {
            let bytes = encode(&pages, format).unwrap();
            let back = image::load_from_memory_with_format(&bytes, kind).unwrap();
            assert_eq!((back.width(), back.height()), (40, 30));
        }
        assert_eq!(OutputFormat::Jpeg.extension(), "jpg");
        assert_eq!(OutputFormat::Pdf.mime(), "application/pdf");
    }

    #[test]
    fn encodes_multi_page_pdf() {
        let gray = ScannedPage {
            image: DynamicImage::ImageLuma8(GrayImage::new(100, 50)),
            dpi: 100,
        };
        let bytes = encode(&[rgb(300, 600, 150), gray], OutputFormat::Pdf).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        assert!(bytes.trim_ascii_end().ends_with(b"%%EOF"));
        assert_eq!(count(&bytes, b"/Type /Page "), 2);
        assert_eq!(count(&bytes, b"/Type /Pages "), 1);
        assert_eq!(count(&bytes, b"/Count 2"), 1);
        assert_eq!(count(&bytes, b"/MediaBox [0 0 144 288]"), 1);
        assert_eq!(count(&bytes, b"/MediaBox [0 0 72 36]"), 1);
        assert_eq!(count(&bytes, b"/ColorSpace /DeviceRGB"), 1);
        assert_eq!(count(&bytes, b"/ColorSpace /DeviceGray"), 1);
        assert_eq!(count(&bytes, b"/Filter /DCTDecode"), 2);
    }

    #[test]
    fn empty_input_is_an_encoding_error() {
        assert!(matches!(
            encode(&[], OutputFormat::Png),
            Err(ScanError::Encoding(_))
        ));
    }
}
