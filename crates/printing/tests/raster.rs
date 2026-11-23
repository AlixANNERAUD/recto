//! PWG raster and Apple raster to PDF, on rasters from Ghostscript and CUPS and on hand-built
//! hostile ones.
//!
//! The fixtures come from `gs` (`pwgraster`, `appleraster`, `urfgray`, `urfrgb`) and from CUPS's
//! own filter chain (`cupsfilter -m image/pwg-raster`). The pixel checksums are those of an
//! independent decoder written from PWG 5102.4 and CUPS's `raster-stream.c`.

use std::time::{Duration, Instant};

use lopdf::{Document, Object};
use recto_printing::raster::{Limits, RasterError, RasterFormat, to_pdf};
use sha2::{Digest, Sha256};

macro_rules! fixture {
    ($name:literal) => {
        include_bytes!(concat!("fixtures/raster/", $name)).as_slice()
    };
}

/// One page of a converted PDF, as a reader sees it.
#[derive(Debug)]
struct Page {
    width: i64,
    height: i64,
    color_space: String,
    media_box: (f32, f32),
    pixels: Vec<u8>,
}

fn pages(pdf: &[u8]) -> Vec<Page> {
    let document = Document::load_mem(pdf).unwrap();
    document
        .get_pages()
        .values()
        .map(|&id| {
            let page = document.get_dictionary(id).unwrap();
            let media_box = page.get(b"MediaBox").unwrap().as_array().unwrap();
            assert_eq!(media_box[0].as_float().unwrap(), 0.0);
            assert_eq!(media_box[1].as_float().unwrap(), 0.0);
            let image = page
                .get(b"Resources")
                .and_then(Object::as_dict)
                .and_then(|resources| resources.get(b"XObject"))
                .and_then(Object::as_dict)
                .and_then(|objects| objects.get(b"Im0"))
                .and_then(Object::as_reference)
                .unwrap();
            let image = document.get_object(image).unwrap().as_stream().unwrap();
            let field = |name: &[u8]| image.dict.get(name).unwrap().as_i64().unwrap();
            assert_eq!(field(b"BitsPerComponent"), 8);
            Page {
                width: field(b"Width"),
                height: field(b"Height"),
                color_space: String::from_utf8(
                    image
                        .dict
                        .get(b"ColorSpace")
                        .unwrap()
                        .as_name()
                        .unwrap()
                        .to_vec(),
                )
                .unwrap(),
                media_box: (
                    media_box[2].as_float().unwrap(),
                    media_box[3].as_float().unwrap(),
                ),
                pixels: image
                    .decompressed_content_with_limit(256 * 1024 * 1024)
                    .unwrap(),
            }
        })
        .collect()
}

fn later() -> Instant {
    Instant::now() + Duration::from_secs(3600)
}

fn convert(document: &[u8], format: RasterFormat) -> Result<Vec<Page>, RasterError> {
    convert_within(document, format, &Limits::new(64 * 1024 * 1024))
}

fn convert_within(
    document: &[u8],
    format: RasterFormat,
    limits: &Limits,
) -> Result<Vec<Page>, RasterError> {
    let converted = to_pdf(document, format, limits, later())?;
    let pages = pages(&converted.pdf);
    assert_eq!(converted.pages as usize, pages.len());
    Ok(pages)
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn close(actual: f32, expected: f32) -> bool {
    (actual - expected).abs() < 0.001
}

/// A PWG page header as Ghostscript writes it: `bits` per pixel of 8-bit colours.
fn pwg_header(width: u32, height: u32, color_space: u32, bits: u32, dpi: u32) -> Vec<u8> {
    let mut header = vec![0; 1796];
    header[..9].copy_from_slice(b"PwgRaster");
    set(&mut header, 276, dpi);
    set(&mut header, 280, dpi);
    set(&mut header, 372, width);
    set(&mut header, 376, height);
    set(&mut header, 384, 8);
    set(&mut header, 388, bits);
    set(&mut header, 392, width.wrapping_mul(bits / 8));
    set(&mut header, 400, color_space);
    set(&mut header, 420, if bits == 24 { 3 } else { 1 });
    header
}

fn set(header: &mut [u8], offset: usize, value: u32) {
    header[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

/// A PWG raster of the given pages, each a header and its coded bitmap.
fn pwg(pages: &[(Vec<u8>, &[u8])]) -> Vec<u8> {
    let mut document = b"RaS2".to_vec();
    for (header, bitmap) in pages {
        document.extend_from_slice(header);
        document.extend_from_slice(bitmap);
    }
    document
}

/// An Apple raster page header: 24 bits per pixel is sRGB, 8 is grey.
fn urf_header(width: u32, height: u32, bits: u8, dpi: u32) -> Vec<u8> {
    let mut header = vec![0; 32];
    header[0] = bits;
    header[1] = if bits == 24 { 1 } else { 0 };
    header[2] = 1;
    set(&mut header, 12, width);
    set(&mut header, 16, height);
    set(&mut header, 20, dpi);
    header
}

/// An Apple raster declaring `count` pages, followed by the given pages.
fn urf(count: u32, pages: &[(Vec<u8>, &[u8])]) -> Vec<u8> {
    let mut document = b"UNIRAST\0".to_vec();
    document.extend_from_slice(&count.to_be_bytes());
    for (header, bitmap) in pages {
        document.extend_from_slice(header);
        document.extend_from_slice(bitmap);
    }
    document
}

#[test]
fn formats_are_detected() {
    assert_eq!(
        RasterFormat::detect(fixture!("two-pages-srgb8-75dpi.pwg")),
        Some(RasterFormat::Pwg)
    );
    assert_eq!(
        RasterFormat::detect(fixture!("two-pages-srgb24-75dpi.urf")),
        Some(RasterFormat::Apple)
    );
    assert_eq!(RasterFormat::detect(b"%PDF-1.7\n"), None);
    assert_eq!(RasterFormat::detect(b"2SaR"), None);
    assert_eq!(RasterFormat::detect(b"UNIRAST"), None);
    assert_eq!(RasterFormat::Pwg.mime(), "image/pwg-raster");
    assert_eq!(RasterFormat::Apple.mime(), "image/urf");
}

#[test]
fn every_fixture_becomes_a_pdf_of_its_pages() {
    let cases: [(&[u8], RasterFormat, i64, i64, &str); 9] = [
        (
            fixture!("two-pages-srgb8-75dpi.pwg"),
            RasterFormat::Pwg,
            310,
            437,
            "DeviceRGB",
        ),
        (
            fixture!("two-pages-sgray8-75dpi.pwg"),
            RasterFormat::Pwg,
            310,
            437,
            "DeviceGray",
        ),
        (
            fixture!("two-pages-srgb24-75dpi.urf"),
            RasterFormat::Apple,
            310,
            437,
            "DeviceRGB",
        ),
        (
            fixture!("two-pages-w8-75dpi.urf"),
            RasterFormat::Apple,
            310,
            437,
            "DeviceGray",
        ),
        (
            fixture!("two-pages-urfrgb.urf"),
            RasterFormat::Apple,
            310,
            438,
            "DeviceRGB",
        ),
        (
            fixture!("two-pages-urfgray.urf"),
            RasterFormat::Apple,
            310,
            438,
            "DeviceGray",
        ),
        (
            fixture!("odd-7x5-srgb8.pwg"),
            RasterFormat::Pwg,
            7,
            5,
            "DeviceRGB",
        ),
        (
            fixture!("odd-7x5-srgb24.urf"),
            RasterFormat::Apple,
            7,
            5,
            "DeviceRGB",
        ),
        // The real CUPS chain at 600 dpi: 96 MiB of pixels a page, within the page budget.
        (
            fixture!("cupsfilter-letter-600dpi-srgb8.pwg"),
            RasterFormat::Pwg,
            5100,
            6600,
            "DeviceRGB",
        ),
    ];
    for (document, format, width, height, color_space) in cases {
        let pages = convert(document, format).unwrap();
        let expected = if width == 7 { 1 } else { 2 };
        assert_eq!(pages.len(), expected, "{width}x{height} {color_space}");
        let samples = if color_space == "DeviceRGB" { 3 } else { 1 };
        let dpi = if width == 5100 { 600.0 } else { 75.0 };
        for page in pages {
            assert_eq!((page.width, page.height), (width, height));
            assert_eq!(page.color_space, color_space);
            assert_eq!(page.pixels.len() as i64, width * height * samples);
            let (media_width, media_height) = page.media_box;
            assert!(
                close(media_width, width as f32 * 72.0 / dpi),
                "{media_width}"
            );
            assert!(
                close(media_height, height as f32 * 72.0 / dpi),
                "{media_height}"
            );
        }
    }
}

#[test]
fn pixels_match_the_reference_decoder() {
    let rgb = [
        "9743cb4546382d0d6470c37ae01754043a83a204e9791e89ee222e06ad088b0d",
        "c7e7b02469d7691b5f2ca435ed5aa7f37f259e519a5c129eefe7016501220718",
    ];
    let grey = [
        "c6479305a2640053755d1b86537faf2f058fd0fbf532b159f7147e6b9b672dd1",
        "08399c80a292e8885c4c0feeff33d42d22e7ab2251dd6e3f92d5027470207fbf",
    ];
    let cases: [(&[u8], RasterFormat, [&str; 2]); 6] = [
        (
            fixture!("two-pages-srgb8-75dpi.pwg"),
            RasterFormat::Pwg,
            rgb,
        ),
        (
            fixture!("two-pages-srgb24-75dpi.urf"),
            RasterFormat::Apple,
            rgb,
        ),
        (
            fixture!("two-pages-sgray8-75dpi.pwg"),
            RasterFormat::Pwg,
            grey,
        ),
        (
            fixture!("two-pages-w8-75dpi.urf"),
            RasterFormat::Apple,
            grey,
        ),
        (
            fixture!("two-pages-urfrgb.urf"),
            RasterFormat::Apple,
            [
                "e3b36bc6590b587df391f13f7e147025c231d12c01dc569786b2af916e5dd51b",
                "0cc49841b3819671a109d094aae543e4944e9c5e6ef2b6512a49a7858a13f4c9",
            ],
        ),
        (
            fixture!("two-pages-urfgray.urf"),
            RasterFormat::Apple,
            [
                "303726cb5ecfcd1c2f3286445dd4134581cbdd9971357938fcb502e1833faec8",
                "75490af0b873bac87707dfd7d45e948e315de0035e4891d9677384ec0027f220",
            ],
        ),
    ];
    for (document, format, checksums) in cases {
        let pages = convert(document, format).unwrap();
        let actual: Vec<_> = pages.iter().map(|page| sha256(&page.pixels)).collect();
        assert_eq!(actual, checksums);
    }
}

#[test]
fn odd_widths_need_no_padding() {
    // 7 pixels of 3 bytes: lines 0 and 1 white, lines 2 to 4 four red pixels then three white.
    // Line 0 is the top of the page, as in PDF image space.
    let white = [0xFF; 21];
    let mut red = [0xFF; 21];
    for pixel in red.chunks_mut(3).take(4) {
        pixel.copy_from_slice(&[0xFF, 0, 0]);
    }
    let expected = [white, white, red, red, red].concat();
    assert_eq!(
        sha256(&expected),
        "0df5dcbeaf600f1dc8a65c56af4e16c71b3d6e6627d1c9d8acdae6575cd14b40"
    );
    for (document, format) in [
        (fixture!("odd-7x5-srgb8.pwg"), RasterFormat::Pwg),
        (fixture!("odd-7x5-srgb24.urf"), RasterFormat::Apple),
    ] {
        let pages = convert(document, format).unwrap();
        assert_eq!(pages[0].pixels, expected);
        assert!(close(pages[0].media_box.0, 6.72));
        assert!(close(pages[0].media_box.1, 4.8));
    }
}

#[test]
fn hand_built_rasters_decode_every_code() {
    // Two 8 x 4 sRGB pages at 300 dpi. Page 1: a line used twice (a run of 8 red), then a
    // literal of two pixels followed by the white fill (code 128), used twice. Page 2: the
    // white fill for all four lines at once.
    let page_one: &[u8] = &[
        0x01, 0x07, 0xFF, 0x00, 0x00, // two lines of eight red
        0x01, 0xFF, 0x00, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0x80, // blue, green, white fill
    ];
    let page_two: &[u8] = &[0x03, 0x80];
    let pwg_document = pwg(&[
        (pwg_header(8, 4, 19, 24, 300), page_one),
        (pwg_header(8, 4, 19, 24, 300), page_two),
    ]);
    let urf_document = urf(
        2,
        &[
            (urf_header(8, 4, 24, 300), page_one),
            (urf_header(8, 4, 24, 300), page_two),
        ],
    );
    let red = [0xFF, 0, 0].repeat(8);
    let mut mixed = vec![0, 0, 0xFF, 0, 0xFF, 0];
    mixed.resize(24, 0xFF);
    let first = [red.clone(), red, mixed.clone(), mixed].concat();
    for (document, format) in [
        (pwg_document, RasterFormat::Pwg),
        (urf_document, RasterFormat::Apple),
    ] {
        let pages = convert(&document, format).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].pixels, first);
        assert_eq!(pages[1].pixels, vec![0xFF; 96]);
        for page in &pages {
            assert!(close(page.media_box.0, 1.92) && close(page.media_box.1, 0.96));
        }
    }
}

#[test]
fn truncated_input_is_refused_not_padded() {
    let cases = [
        (fixture!("odd-7x5-srgb8.pwg"), RasterFormat::Pwg, 4),
        (fixture!("odd-7x5-srgb24.urf"), RasterFormat::Apple, 12),
    ];
    for (document, format, file_header) in cases {
        for end in 0..document.len() {
            let result = convert(&document[..end], format).map(|pages| pages.len());
            let expected = if end == file_header {
                RasterError::Unsupported("no pages")
            } else {
                RasterError::Truncated
            };
            assert_eq!(result, Err(expected), "cut at {end}");
        }
    }
    // A second page cut inside its bitmap.
    let two = fixture!("two-pages-srgb8-75dpi.pwg");
    assert_eq!(
        convert(&two[..two.len() - 1], RasterFormat::Pwg).map(|pages| pages.len()),
        Err(RasterError::Truncated)
    );
}

#[test]
fn bad_sync_words_and_headers_are_refused() {
    let white: &[u8] = &[0x00, 0x80];
    let unsupported = |document: &[u8], format| {
        let result = convert(document, format).map(|pages| pages.len());
        assert!(
            matches!(result, Err(RasterError::Unsupported(_))),
            "{result:?}"
        );
    };
    // Other CUPS raster versions and byte orders, and anything else.
    for sync in [b"2SaR", b"RaS3", b"3SaR", b"RaSt", b"tSaR", b"%PDF"] {
        let mut document = pwg(&[(pwg_header(1, 1, 19, 24, 300), white)]);
        document[..4].copy_from_slice(sync);
        unsupported(&document, RasterFormat::Pwg);
    }
    let mut document = urf(1, &[(urf_header(1, 1, 24, 300), white)]);
    document[7] = b'X';
    unsupported(&document, RasterFormat::Apple);
    unsupported(fixture!("two-pages-black1-75dpi.pwg"), RasterFormat::Pwg);

    let edits: [(usize, u32); 12] = [
        (384, 16),   // 16 bits per colour
        (388, 48),   // 48 bits per pixel
        (400, 6),    // CMYK
        (400, 3),    // black
        (420, 1),    // one colour for sRGB
        (396, 1),    // banded colour order
        (392, 4),    // bytes per line not width * 3
        (276, 0),    // no resolution
        (280, 1200), // past 600 dpi
        (276, 74),   // under 75 dpi
        (372, 0),    // no width
        (376, 0),    // no height
    ];
    for (offset, value) in edits {
        let mut header = pwg_header(1, 1, 19, 24, 300);
        set(&mut header, offset, value);
        unsupported(&pwg(&[(header, white)]), RasterFormat::Pwg);
    }
    // The PWG greyscale header with sRGB's sizes.
    unsupported(
        &pwg(&[(pwg_header(1, 1, 18, 24, 300), white)]),
        RasterFormat::Pwg,
    );

    let edits: [(usize, u8); 6] = [
        (0, 32), // CMYK's bits per pixel
        (0, 48), // 16-bit RGB
        (1, 6),  // CMYK
        (1, 2),  // CIELab
        (1, 3),  // Adobe RGB
        (1, 0),  // grey with 24 bits per pixel
    ];
    for (offset, value) in edits {
        let mut header = urf_header(1, 1, 24, 300);
        header[offset] = value;
        unsupported(&urf(1, &[(header, white)]), RasterFormat::Apple);
    }
    for (width, height, dpi) in [(0, 1, 300), (1, 0, 300), (1, 1, 0), (1, 1, 1200)] {
        unsupported(
            &urf(1, &[(urf_header(width, height, 24, dpi), white)]),
            RasterFormat::Apple,
        );
    }
    // No page at all.
    assert_eq!(
        convert(b"RaS2", RasterFormat::Pwg).map(|pages| pages.len()),
        Err(RasterError::Unsupported("no pages"))
    );
}

#[test]
fn run_length_overruns_are_refused() {
    let cases: [(&[u8], &str); 4] = [
        // A run of 8 pixels in a line of 7.
        (&[0x04, 0x07, 0xFF, 0xFF, 0xFF], "run"),
        // Six white pixels, then a literal of two.
        (
            &[0x04, 0x05, 0xFF, 0xFF, 0xFF, 0xFF, 1, 2, 3, 4, 5, 6],
            "literal",
        ),
        // A line used 6 times on a page of 5 lines.
        (&[0x05, 0x80], "repeat"),
        // Four lines, then one used twice.
        (&[0x03, 0x80, 0x01, 0x80], "repeat"),
    ];
    for (bitmap, what) in cases {
        for (document, format) in [
            (
                pwg(&[(pwg_header(7, 5, 19, 24, 75), bitmap)]),
                RasterFormat::Pwg,
            ),
            (
                urf(1, &[(urf_header(7, 5, 24, 75), bitmap)]),
                RasterFormat::Apple,
            ),
        ] {
            let result = convert(&document, format).map(|pages| pages.len());
            assert!(
                matches!(result, Err(RasterError::Malformed(_))),
                "{what}: {result:?}"
            );
        }
    }
}

#[test]
fn hostile_sizes_are_refused_before_decoding() {
    // No bitmap follows the headers: decoding would report the input as truncated.
    let huge = [
        (100_000, 100_000),
        (u32::MAX, u32::MAX),
        (u32::MAX, 1),
        (1, u32::MAX),
    ];
    for (width, height) in huge {
        assert_eq!(
            convert(
                &urf(1, &[(urf_header(width, height, 24, 300), &[])]),
                RasterFormat::Apple
            )
            .map(|pages| pages.len()),
            Err(RasterError::PageTooLarge),
            "{width}x{height}"
        );
        assert_eq!(
            convert(
                &pwg(&[(pwg_header(width, height, 19, 24, 300), &[])]),
                RasterFormat::Pwg
            )
            .map(|pages| pages.len()),
            Err(RasterError::PageTooLarge),
            "{width}x{height}"
        );
    }
    // The page budget is decoded bytes: a grey page is three times smaller than an RGB one.
    let limits = Limits {
        page_bytes: 300,
        ..Limits::new(1024 * 1024)
    };
    let rgb = urf(
        1,
        &[(urf_header(10, 10, 24, 300), &[0x09, 0x09, 0xFF, 0xFF, 0xFF])],
    );
    assert_eq!(
        convert_within(&rgb, RasterFormat::Apple, &limits).map(|pages| pages.len()),
        Ok(1)
    );
    let rgb = urf(1, &[(urf_header(10, 11, 24, 300), &[])]);
    assert_eq!(
        convert_within(&rgb, RasterFormat::Apple, &limits).map(|pages| pages.len()),
        Err(RasterError::PageTooLarge)
    );
}

#[test]
fn a_run_length_bomb_stays_in_its_budgets() {
    // 1 MiB of white per page in 8 bytes: four lines of `FF 80`, each used 256 times.
    let bomb: &[u8] = &[0xFF, 0x80, 0xFF, 0x80, 0xFF, 0x80, 0xFF, 0x80];
    let limits = Limits {
        page_bytes: 1024 * 1024,
        document_bytes: 4 * 1024 * 1024,
        pages: 10,
        output: 1024 * 1024,
    };
    let bombs = |count: usize| {
        let pages: Vec<_> = (0..count)
            .map(|_| (urf_header(1024, 1024, 8, 300), bomb))
            .collect();
        urf(0, &pages)
    };
    assert_eq!(
        convert_within(&bombs(4), RasterFormat::Apple, &limits).map(|pages| pages.len()),
        Ok(4)
    );
    assert_eq!(
        convert_within(&bombs(400), RasterFormat::Apple, &limits).map(|pages| pages.len()),
        Err(RasterError::DocumentTooLarge)
    );

    // Pages are counted as they are decoded, whatever the header says.
    let tiny = |count: usize, declared: u32| {
        let pages: Vec<_> = (0..count)
            .map(|_| (urf_header(1, 1, 8, 300), &[0x00, 0x80][..]))
            .collect();
        urf(declared, &pages)
    };
    assert_eq!(
        convert_within(&tiny(10, 10_000), RasterFormat::Apple, &limits).map(|pages| pages.len()),
        Ok(10)
    );
    assert_eq!(
        convert_within(&tiny(11, 0), RasterFormat::Apple, &limits).map(|pages| pages.len()),
        Err(RasterError::TooManyPages)
    );
    // The real limit, with real page headers.
    let pages = recto_printing::raster::MAX_PAGES as usize;
    assert_eq!(
        convert(&tiny(pages + 1, 0), RasterFormat::Apple).map(|pages| pages.len()),
        Err(RasterError::TooManyPages)
    );

    // Pixels that do not compress: the output budget stops them as they are written.
    let mut noise = Vec::new();
    let mut seed: u32 = 1;
    for _ in 0..256 {
        // Each line used once, made of one literal of 128 pixels.
        noise.extend_from_slice(&[0x00, 0x81]);
        for _ in 0..128 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            noise.push((seed >> 16) as u8);
        }
    }
    let noisy = urf(1, &[(urf_header(128, 256, 8, 300), &noise)]);
    assert_eq!(
        convert_within(&noisy, RasterFormat::Apple, &limits).map(|pages| pages.len()),
        Ok(1)
    );
    let small = Limits {
        output: 16 * 1024,
        ..limits
    };
    assert_eq!(
        convert_within(&noisy, RasterFormat::Apple, &small).map(|pages| pages.len()),
        Err(RasterError::TooLarge)
    );
}

#[test]
fn the_deadline_stops_the_decoder() {
    // 1000 lines declared, 64 coded: within the time, the input ends early; past it, the
    // decoder stops before it notices.
    let document = urf(1, &[(urf_header(16, 1000, 8, 300), &[0x3F, 0x80])]);
    let limits = Limits::new(1024 * 1024);
    assert_eq!(
        to_pdf(&document, RasterFormat::Apple, &limits, later()).map(|done| done.pages),
        Err(RasterError::Truncated)
    );
    assert_eq!(
        to_pdf(&document, RasterFormat::Apple, &limits, Instant::now()).map(|done| done.pages),
        Err(RasterError::TimedOut)
    );
    // A whole fixture with no time left.
    assert_eq!(
        to_pdf(
            fixture!("two-pages-srgb8-75dpi.pwg"),
            RasterFormat::Pwg,
            &limits,
            Instant::now()
        )
        .map(|done| done.pages),
        Err(RasterError::TimedOut)
    );
}
