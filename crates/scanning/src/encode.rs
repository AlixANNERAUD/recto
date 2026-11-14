use crate::{error::ScanError, model::OutputFormat, pdf};
use image::{
    DynamicImage, ImageEncoder,
    codecs::{jpeg::JpegEncoder, png::PngEncoder},
};

/// One scanned page, compressed as soon as it was read: a feeder batch is held as compressed
/// pages, never as raw pixels, so its memory grows by tens or hundreds of kilobytes a page
/// rather than by the tens of megabytes of the raw image.
#[derive(Debug, Clone)]
pub struct ScannedPage {
    /// The page in the image encoding of `format`: JPEG for PDF and JPEG, PNG for PNG.
    pub data: Vec<u8>,
    /// The output format the page was encoded for.
    pub format: OutputFormat,
    pub width: u32,
    pub height: u32,
    pub gray: bool,
    pub dpi: u32,
}

const JPEG_QUALITY: u8 = 85;

fn encoding(e: impl std::fmt::Display) -> ScanError {
    ScanError::Encoding(e.to_string())
}

/// 8-bit gray and RGB images are written as they are; anything else (16-bit, alpha) is
/// converted first, which JPEG needs. Converting would hold a second copy of the page, so it
/// is kept for the cases that need it.
fn jpeg(image: &DynamicImage, gray: bool) -> Result<Vec<u8>, ScanError> {
    let mut buf = Vec::new();
    let encoder = JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY);
    match image {
        DynamicImage::ImageLuma8(_) | DynamicImage::ImageRgb8(_) => encoder.write_image(
            image.as_bytes(),
            image.width(),
            image.height(),
            image.color().into(),
        ),
        _ if gray => image.to_luma8().write_with_encoder(encoder),
        _ => image.to_rgb8().write_with_encoder(encoder),
    }
    .map_err(encoding)?;
    Ok(buf)
}

/// Lossless, in the image's own sample depth.
fn png(image: &DynamicImage) -> Result<Vec<u8>, ScanError> {
    let mut buf = Vec::new();
    PngEncoder::new(&mut buf)
        .write_image(
            image.as_bytes(),
            image.width(),
            image.height(),
            image.color().into(),
        )
        .map_err(encoding)?;
    Ok(buf)
}

impl ScannedPage {
    /// Compresses a freshly read page for `format`.
    pub fn encode(image: &DynamicImage, dpi: u32, format: OutputFormat) -> Result<Self, ScanError> {
        let gray = !image.color().has_color();
        let data = match format {
            OutputFormat::Png => png(image)?,
            OutputFormat::Jpeg | OutputFormat::Pdf => jpeg(image, gray)?,
        };
        Ok(Self {
            data,
            format,
            width: image.width(),
            height: image.height(),
            gray,
            dpi,
        })
    }
}

/// The file for `pages`, which must have been encoded for `format`. PNG and JPEG hold a single
/// image, so only the first page is written.
pub fn assemble(pages: Vec<ScannedPage>, format: OutputFormat) -> Result<Vec<u8>, ScanError> {
    if pages.iter().any(|page| page.format != format) {
        return Err(encoding("pages encoded for another format"));
    }
    match format {
        OutputFormat::Png | OutputFormat::Jpeg => pages
            .into_iter()
            .next()
            .map(|page| page.data)
            .ok_or_else(|| encoding("no pages to encode")),
        OutputFormat::Pdf => {
            if pages.is_empty() {
                return Err(encoding("no pages to encode"));
            }
            let pages: Vec<_> = pages
                .into_iter()
                .map(|page| pdf::PdfPage {
                    jpeg: page.data,
                    width_px: page.width,
                    height_px: page.height,
                    gray: page.gray,
                    dpi: page.dpi,
                })
                .collect();
            Ok(pdf::write(&pages))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, ImageFormat, Luma, Rgb, RgbImage};

    fn rgb(w: u32, h: u32, dpi: u32, format: OutputFormat) -> ScannedPage {
        ScannedPage::encode(&DynamicImage::ImageRgb8(RgbImage::new(w, h)), dpi, format).unwrap()
    }

    fn count(hay: &[u8], needle: &[u8]) -> usize {
        hay.windows(needle.len()).filter(|w| *w == needle).count()
    }

    #[test]
    fn encodes_png_and_jpeg() {
        for (format, kind) in [
            (OutputFormat::Png, ImageFormat::Png),
            (OutputFormat::Jpeg, ImageFormat::Jpeg),
        ] {
            let pages = vec![rgb(40, 30, 150, format), rgb(10, 10, 150, format)];
            let bytes = assemble(pages, format).unwrap();
            let back = image::load_from_memory_with_format(&bytes, kind).unwrap();
            assert_eq!((back.width(), back.height()), (40, 30));
        }
        assert_eq!(OutputFormat::Jpeg.extension(), "jpg");
        assert_eq!(OutputFormat::Pdf.mime(), "application/pdf");
    }

    #[test]
    fn png_output_is_lossless() {
        let image = RgbImage::from_fn(64, 48, |x, y| Rgb([x as u8 * 4, y as u8 * 5, 77]));
        let gray = GrayImage::from_fn(33, 17, |x, y| Luma([(x * 7 + y * 3) as u8]));
        for image in [
            DynamicImage::ImageRgb8(image),
            DynamicImage::ImageLuma8(gray),
        ] {
            let page = ScannedPage::encode(&image, 300, OutputFormat::Png).unwrap();
            let bytes = assemble(vec![page], OutputFormat::Png).unwrap();
            let back = image::load_from_memory_with_format(&bytes, ImageFormat::Png).unwrap();
            assert_eq!(back, image);
        }
    }

    #[test]
    fn pages_hold_compressed_data() {
        // A blank page compresses to a sliver of its 300 KB of samples.
        let page = rgb(400, 250, 150, OutputFormat::Pdf);
        assert!(page.data.starts_with(&[0xFF, 0xD8]), "a JPEG");
        assert!(page.data.len() < 400 * 250 * 3 / 20, "{}", page.data.len());
        assert_eq!((page.width, page.height, page.gray), (400, 250, false));
    }

    #[test]
    fn encodes_multi_page_pdf() {
        let gray = ScannedPage::encode(
            &DynamicImage::ImageLuma8(GrayImage::new(100, 50)),
            100,
            OutputFormat::Pdf,
        )
        .unwrap();
        let bytes = assemble(
            vec![rgb(300, 600, 150, OutputFormat::Pdf), gray],
            OutputFormat::Pdf,
        )
        .unwrap();
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
    fn empty_or_mismatched_input_is_an_encoding_error() {
        assert!(matches!(
            assemble(vec![], OutputFormat::Png),
            Err(ScanError::Encoding(_))
        ));
        assert!(matches!(
            assemble(vec![], OutputFormat::Pdf),
            Err(ScanError::Encoding(_))
        ));
        assert!(matches!(
            assemble(vec![rgb(4, 4, 75, OutputFormat::Png)], OutputFormat::Pdf),
            Err(ScanError::Encoding(_))
        ));
    }
}
