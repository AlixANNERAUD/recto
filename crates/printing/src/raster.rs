//! PWG raster and Apple raster (URF) to PDF.
//!
//! Driverless clients send rasters: IPP Everywhere requires PWG raster (`image/pwg-raster`, PWG
//! 5102.4) and AirPrint Apple raster (`image/urf`, whose reference is CUPS's
//! `cups/raster-stream.c`). recto turns them into a PDF with one lossless Flate image per page
//! when they arrive, so counting, previews, page ranges and CUPS only ever see a PDF.
//!
//! Both formats code each line the same way, and a few bytes can stand for a whole page. The
//! bounds are therefore on decoded pixels, read from each page header before its bitmap: the
//! pixels of a page, of the document and the number of pages ([`Limits`]). Decoded lines go
//! straight into a zlib encoder, so the memory used is one line and the compressed output, which
//! [`Limits::output`] caps as it grows. A deadline is checked every [`DEADLINE_LINES`] lines,
//! because a timeout around the blocking thread would stop waiting for it but not stop it.

use std::io::{self, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use flate2::{Compression, write::ZlibEncoder};
use lopdf::{Document, Object, Stream, dictionary};
use thiserror::Error;
use tokio::sync::Semaphore;

/// The most decoded bytes a page may have: Legal at 600 dpi in colour (5100 x 8400 x 3) fits.
pub const MAX_PAGE_BYTES: u64 = 160 * 1024 * 1024;

/// The most decoded bytes a document may have. They cost compression time, which the deadline
/// bounds, not memory.
pub const MAX_DOCUMENT_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// The most pages a document may have, counted as they are decoded: the counts in the headers
/// are not reliable (Ghostscript writes 0).
pub const MAX_PAGES: u32 = 500;

/// How many decoded lines pass between two looks at the deadline.
pub const DEADLINE_LINES: u32 = 64;

/// The resolutions accepted, in dots per inch, each way.
const RESOLUTIONS: std::ops::RangeInclusive<u32> = 75..=600;

/// The two raster formats, told apart by their first bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RasterFormat {
    /// PWG raster, `RaS2`.
    Pwg,
    /// Apple raster, `UNIRAST\0`.
    Apple,
}

impl RasterFormat {
    /// Detects the format from the content. Other CUPS raster versions and byte orders are not
    /// PWG raster and are not detected.
    pub fn detect(bytes: &[u8]) -> Option<RasterFormat> {
        if bytes.starts_with(b"RaS2") {
            Some(RasterFormat::Pwg)
        } else if bytes.starts_with(b"UNIRAST\0") {
            Some(RasterFormat::Apple)
        } else {
            None
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            RasterFormat::Pwg => "image/pwg-raster",
            RasterFormat::Apple => "image/urf",
        }
    }
}

/// What a conversion may cost. The defaults are the constants of this module; tests shrink them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Decoded bytes of one page.
    pub page_bytes: u64,
    /// Decoded bytes of the whole document.
    pub document_bytes: u64,
    /// Pages of the document.
    pub pages: u32,
    /// Bytes of the PDF written.
    pub output: usize,
}

impl Limits {
    /// The module's limits, for a PDF of at most `output` bytes.
    pub fn new(output: usize) -> Self {
        Self {
            page_bytes: MAX_PAGE_BYTES,
            document_bytes: MAX_DOCUMENT_BYTES,
            pages: MAX_PAGES,
            output,
        }
    }
}

/// Why a raster was not converted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum RasterError {
    /// The input ends inside a header, a line or a pixel.
    #[error("the raster ends early")]
    Truncated,
    /// A run or a repeated line goes past the end of its line or page.
    #[error("malformed raster: {0}")]
    Malformed(&'static str),
    /// Not a raster recto reads: another format, depth, colour space or resolution, or no page.
    #[error("unsupported raster: {0}")]
    Unsupported(&'static str),
    /// A page has more pixels than [`Limits::page_bytes`].
    #[error("a page of the raster is too large")]
    PageTooLarge,
    /// The document has more pages than [`Limits::pages`].
    #[error("the raster has too many pages")]
    TooManyPages,
    /// The document has more pixels than [`Limits::document_bytes`].
    #[error("the raster is too large")]
    DocumentTooLarge,
    /// The PDF would be larger than [`Limits::output`].
    #[error("the raster makes a PDF too large")]
    TooLarge,
    /// The deadline passed during the conversion.
    #[error("the raster took too long to convert")]
    TimedOut,
    /// Other rasters held every place of the [`RasterConverter`] the whole time.
    #[error("too many rasters are being converted")]
    Busy,
}

/// A raster converted to PDF.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Converted {
    pub pdf: Vec<u8>,
    pub pages: u32,
}

/// Converts rasters with bounded concurrency and time, as `count::PageCounter` reads PDFs. A
/// conversion runs on a blocking thread, which cannot be cancelled, so it carries its own
/// deadline into [`to_pdf`] and keeps its place until it ends: the threads and the memory they
/// hold stay bounded whatever is sent.
#[derive(Clone, Debug)]
pub struct RasterConverter {
    permits: Arc<Semaphore>,
    timeout: Duration,
    /// The page, document and page-count budgets; each call gives its own output budget.
    limits: Limits,
}

impl RasterConverter {
    /// At most `concurrency` rasters converted at once. A conversion waits at most `timeout`
    /// for a place, then has `timeout` to finish.
    pub fn new(concurrency: usize, timeout: Duration, limits: Limits) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(concurrency.max(1))),
            timeout,
            limits,
        }
    }

    /// [`to_pdf`] on a blocking thread, for a PDF of at most `max_output` bytes. The document
    /// is shared, not copied.
    pub async fn convert(
        &self,
        document: Bytes,
        format: RasterFormat,
        max_output: usize,
    ) -> Result<Converted, RasterError> {
        let permit = tokio::time::timeout(self.timeout, self.permits.clone().acquire_owned())
            .await
            .map_err(|_| RasterError::Busy)?
            .map_err(|_| RasterError::Busy)?;
        let limits = Limits {
            output: max_output,
            ..self.limits
        };
        let deadline = Instant::now() + self.timeout;
        let converted = tokio::task::spawn_blocking(move || {
            let converted = to_pdf(&document, format, &limits, deadline);
            drop(permit);
            converted
        });
        // The decoder stops at the deadline by itself; a thread that panicked converted nothing.
        converted.await.unwrap_or(Err(RasterError::TimedOut))
    }
}

impl Default for RasterConverter {
    /// Two at a time, sixty seconds each, within the module's limits.
    fn default() -> Self {
        Self::new(2, Duration::from_secs(60), Limits::new(0))
    }
}

/// Converts a raster to a PDF of one image per page, or refuses it as soon as it breaks a limit.
/// This blocks for as long as the decoding takes, up to `deadline`: async callers run it on a
/// blocking thread.
pub fn to_pdf(
    document: &[u8],
    format: RasterFormat,
    limits: &Limits,
    deadline: Instant,
) -> Result<Converted, RasterError> {
    let mut input = Input {
        bytes: document,
        position: 0,
    };
    match format {
        RasterFormat::Pwg => {
            if input.take(4)? != b"RaS2" {
                return Err(RasterError::Unsupported("not a PWG raster"));
            }
        }
        RasterFormat::Apple => {
            if input.take(8)? != b"UNIRAST\0" {
                return Err(RasterError::Unsupported("not an Apple raster"));
            }
            // The page count, unreliable: pages are counted as they are decoded.
            input.take(4)?;
        }
    }

    let mut clock = Clock { deadline, lines: 0 };
    let mut output_left = limits.output;
    let mut document_bytes: u64 = 0;
    let mut pages = Vec::new();
    let mut line = Vec::new();
    while !input.is_empty() {
        let header = match format {
            RasterFormat::Pwg => pwg_header(input.take(PWG_HEADER)?)?,
            RasterFormat::Apple => urf_header(input.take(URF_HEADER)?)?,
        };
        if pages.len() >= limits.pages as usize {
            return Err(RasterError::TooManyPages);
        }
        let page_bytes = header.bytes().ok_or(RasterError::PageTooLarge)?;
        if page_bytes > limits.page_bytes {
            return Err(RasterError::PageTooLarge);
        }
        document_bytes = document_bytes
            .checked_add(page_bytes)
            .filter(|bytes| *bytes <= limits.document_bytes)
            .ok_or(RasterError::DocumentTooLarge)?;
        if let Some(bytes_per_line) = header.bytes_per_line
            && bytes_per_line != u64::from(header.width) * header.samples as u64
        {
            return Err(RasterError::Unsupported("bytes per line"));
        }

        let mut image = ZlibEncoder::new(
            Capped {
                bytes: Vec::new(),
                left: output_left,
            },
            Compression::new(6),
        );
        decode_page(&mut input, &header, &mut line, &mut image, &mut clock)?;
        let image = image.finish().map_err(|_| RasterError::TooLarge)?;
        output_left = image.left;
        pages.push((header, image.bytes));
    }
    if pages.is_empty() {
        return Err(RasterError::Unsupported("no pages"));
    }
    let count = pages.len() as u32;
    let pdf = write_pdf(pages, limits.output)?;
    Ok(Converted { pdf, pages: count })
}

const PWG_HEADER: usize = 1796;
const URF_HEADER: usize = 32;

/// What decoding and drawing a page needs from its header.
struct PageHeader {
    width: u32,
    height: u32,
    /// Bytes, and samples, per pixel: 1 for grey, 3 for RGB.
    samples: usize,
    x_dpi: u32,
    y_dpi: u32,
    /// PWG states it, Apple raster does not. Compared once the page size is known to be sane,
    /// so a huge page reports its size rather than this.
    bytes_per_line: Option<u64>,
}

impl PageHeader {
    /// The decoded size of the page, `None` past `u64`.
    fn bytes(&self) -> Option<u64> {
        u64::from(self.width)
            .checked_mul(u64::from(self.height))?
            .checked_mul(self.samples as u64)
    }

    fn checked(self) -> Result<PageHeader, RasterError> {
        if self.width == 0 || self.height == 0 {
            return Err(RasterError::Unsupported("empty page"));
        }
        if !RESOLUTIONS.contains(&self.x_dpi) || !RESOLUTIONS.contains(&self.y_dpi) {
            return Err(RasterError::Unsupported("resolution"));
        }
        Ok(self)
    }
}

fn u32_at(header: &[u8], offset: usize) -> u32 {
    let mut bytes = [0; 4];
    bytes.copy_from_slice(&header[offset..offset + 4]);
    u32::from_be_bytes(bytes)
}

/// A PWG 5102.4 page header (Table 1); only 8-bit sGray and sRGB, chunky, are read.
fn pwg_header(header: &[u8]) -> Result<PageHeader, RasterError> {
    if u32_at(header, 384) != 8 {
        return Err(RasterError::Unsupported("bits per colour"));
    }
    if u32_at(header, 396) != 0 {
        return Err(RasterError::Unsupported("colour order"));
    }
    // Colour space, bits per pixel and number of colours: sgray_8 and srgb_8.
    let samples = match (
        u32_at(header, 400),
        u32_at(header, 388),
        u32_at(header, 420),
    ) {
        (18, 8, 1) => 1,
        (19, 24, 3) => 3,
        _ => return Err(RasterError::Unsupported("colour space")),
    };
    PageHeader {
        width: u32_at(header, 372),
        height: u32_at(header, 376),
        samples,
        x_dpi: u32_at(header, 276),
        y_dpi: u32_at(header, 280),
        bytes_per_line: Some(u64::from(u32_at(header, 392))),
    }
    .checked()
}

/// An Apple raster page header, as CUPS reads it. Ghostscript's `urfgray` and `urfrgb` write
/// the device colour spaces 4 and 5 where CUPS writes sGray 0 and sRGB 1: both are accepted.
fn urf_header(header: &[u8]) -> Result<PageHeader, RasterError> {
    let samples = match (header[0], header[1]) {
        (8, 0 | 4) => 1,
        (24, 1 | 5) => 3,
        _ => return Err(RasterError::Unsupported("colour space")),
    };
    let dpi = u32_at(header, 20);
    PageHeader {
        width: u32_at(header, 12),
        height: u32_at(header, 16),
        samples,
        x_dpi: dpi,
        y_dpi: dpi,
        bytes_per_line: None,
    }
    .checked()
}

/// The raster being read; reading past its end is [`RasterError::Truncated`], never padding.
struct Input<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Input<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], RasterError> {
        let end = self
            .position
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(RasterError::Truncated)?;
        let taken = &self.bytes[self.position..end];
        self.position = end;
        Ok(taken)
    }

    fn byte(&mut self) -> Result<u8, RasterError> {
        Ok(self.take(1)?[0])
    }

    fn is_empty(&self) -> bool {
        self.position == self.bytes.len()
    }
}

/// Counts decoded lines across pages, and looks at the deadline every [`DEADLINE_LINES`].
struct Clock {
    deadline: Instant,
    lines: u32,
}

impl Clock {
    fn line(&mut self) -> Result<(), RasterError> {
        self.lines = self.lines.wrapping_add(1);
        if self.lines.is_multiple_of(DEADLINE_LINES) && Instant::now() >= self.deadline {
            return Err(RasterError::TimedOut);
        }
        Ok(())
    }
}

/// Decodes one page into `image`, line by line (PWG 5102.4 section 4.4, with CUPS's code 128).
/// Each line starts with how many times it is used, less one; then, until the line is full, a
/// control byte `c`: up to 127 repeats the next pixel `c + 1` times, 129 and up copies `257 - c`
/// pixels, and 128 fills the rest of the line with white. CUPS clamps a run past the end of the
/// line or the page; recto refuses it, since no producer writes one.
fn decode_page(
    input: &mut Input,
    header: &PageHeader,
    line: &mut Vec<u8>,
    image: &mut impl Write,
    clock: &mut Clock,
) -> Result<(), RasterError> {
    let pixel = header.samples;
    // Fits: the page's size, of which this is a part, passed the page budget.
    let line_bytes = header.width as usize * pixel;
    let mut y = 0;
    while y < header.height {
        let uses = u32::from(input.byte()?) + 1;
        line.clear();
        while line.len() < line_bytes {
            match input.byte()? {
                128 => line.resize(line_bytes, 0xFF),
                control @ 129.. => {
                    let bytes = (257 - usize::from(control)) * pixel;
                    if line.len() + bytes > line_bytes {
                        return Err(RasterError::Malformed("literal past the end of the line"));
                    }
                    line.extend_from_slice(input.take(bytes)?);
                }
                control => {
                    let times = usize::from(control) + 1;
                    if line.len() + times * pixel > line_bytes {
                        return Err(RasterError::Malformed("run past the end of the line"));
                    }
                    let value = input.take(pixel)?;
                    for _ in 0..times {
                        line.extend_from_slice(value);
                    }
                }
            }
        }
        if uses > header.height - y {
            return Err(RasterError::Malformed("line used past the end of the page"));
        }
        for _ in 0..uses {
            image.write_all(line).map_err(|_| RasterError::TooLarge)?;
            clock.line()?;
        }
        y += uses;
    }
    Ok(())
}

/// A writer that fails once it would hold more than `left` bytes.
struct Capped {
    bytes: Vec<u8>,
    left: usize,
}

impl Write for Capped {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.left {
            return Err(io::Error::other("over the output limit"));
        }
        self.left -= bytes.len();
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A PDF of one page per image, each page the size of its image at its resolution. Raster line
/// 0 is the top of the page, as is the first row of a PDF image: nothing is flipped.
fn write_pdf(pages: Vec<(PageHeader, Vec<u8>)>, output: usize) -> Result<Vec<u8>, RasterError> {
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let mut kids = Vec::with_capacity(pages.len());
    for (header, image) in pages {
        let width = header.width as f32 * 72.0 / header.x_dpi as f32;
        let height = header.height as f32 * 72.0 / header.y_dpi as f32;
        let color_space = if header.samples == 3 {
            "DeviceRGB"
        } else {
            "DeviceGray"
        };
        let image = document.add_object(
            Stream::new(
                dictionary! {
                    "Type" => "XObject",
                    "Subtype" => "Image",
                    "Width" => i64::from(header.width),
                    "Height" => i64::from(header.height),
                    "ColorSpace" => color_space,
                    "BitsPerComponent" => 8,
                    "Filter" => "FlateDecode",
                },
                image,
            )
            .with_compression(false),
        );
        let content = document.add_object(Stream::new(
            dictionary! {},
            format!("q {width} 0 0 {height} 0 0 cm /Im0 Do Q").into_bytes(),
        ));
        kids.push(Object::from(document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), width.into(), height.into()],
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image } },
            "Contents" => content,
        })));
    }
    let count = kids.len() as i64;
    document.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }),
    );
    let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    document.trailer.set("Root", catalog);
    let mut pdf = Capped {
        bytes: Vec::new(),
        left: output,
    };
    // Only the cap can fail a write to memory.
    document
        .save_to(&mut pdf)
        .map_err(|_| RasterError::TooLarge)?;
    Ok(pdf.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: &[u8] = include_bytes!("../tests/fixtures/raster/odd-7x5-srgb24.urf");
    const TWO_PAGES: &[u8] = include_bytes!("../tests/fixtures/raster/two-pages-w8-75dpi.urf");

    #[tokio::test]
    async fn the_converter_bounds_places_and_time() {
        let converter = RasterConverter::new(1, Duration::from_millis(50), Limits::new(0));
        let convert = |document: &'static [u8], output: usize| {
            converter.convert(Bytes::from_static(document), RasterFormat::Apple, output)
        };
        assert_eq!(convert(SMALL, 1 << 20).await.map(|done| done.pages), Ok(1));
        // The output budget is the caller's, not the template's.
        assert_eq!(convert(SMALL, 16).await, Err(RasterError::TooLarge));

        // Another conversion holds the only place until the time is up: busy, which a retry
        // may fix, and the document is not read.
        let place = converter.permits.clone().acquire_owned().await.unwrap();
        assert_eq!(convert(SMALL, 1 << 20).await, Err(RasterError::Busy));
        drop(place);
        assert_eq!(
            convert(TWO_PAGES, 1 << 20).await.map(|done| done.pages),
            Ok(2)
        );

        // No time at all: the decoder stops itself on its thread.
        let hurried = RasterConverter::new(1, Duration::ZERO, Limits::new(0));
        assert_eq!(
            hurried
                .convert(Bytes::from_static(TWO_PAGES), RasterFormat::Apple, 1 << 20)
                .await,
            Err(RasterError::TimedOut)
        );
    }
}
