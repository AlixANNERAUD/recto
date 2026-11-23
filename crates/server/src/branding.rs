//! The operator's branding: the name, the accent color, the logo and the icons. Everything is
//! read and checked once at startup, so a mistake stops recto with a message naming the setting
//! rather than showing a broken page, and requests never touch the files.

use std::{
    fs::File,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

use axum::body::Bytes;
use image::{
    DynamicImage, ImageFormat, ImageReader, Rgba, RgbaImage,
    imageops::{self, FilterType},
};
pub use recto_app::branding::{BrandFile, BrandIcons, Branding, DEFAULT_COLOR, DEFAULT_NAME};

const MEBIBYTE: u64 = 1024 * 1024;
const LOGO_LIMIT: u64 = MEBIBYTE;
const ICON_LIMIT: u64 = 5 * MEBIBYTE;
/// The largest icon a browser asks for; anything smaller would be blown up.
const ICON_MINIMUM: u32 = 512;
/// Checked before decoding: a small, highly compressed file could otherwise unpack into
/// hundreds of megabytes.
const ICON_MAXIMUM: u32 = 4096;
/// What WCAG 2 asks of normal text (level AA).
const MINIMUM_CONTRAST: f64 = 4.5;
const WHITE: [u8; 3] = [255; 3];
/// The light theme's page background; its cards are white.
const LIGHT_BACKGROUND: [u8; 3] = [0xf5; 3];
const BLACK: [u8; 3] = [0; 3];
/// The dark theme's cards, where most accent text sits.
const DARK_CARD: [u8; 3] = [0x29; 3];
/// The dark theme's page background.
const DARK_BACKGROUND: [u8; 3] = [0x1f; 3];

/// The branding settings of `recto serve`.
#[derive(Clone, Debug, getargs_derive::GetArgs)]
pub struct BrandOptions {
    /// The name in the header, the page titles and the installed app; 1 to 64 characters.
    #[arg(long, env = "RECTO_BRAND_NAME", default_value = "recto")]
    pub brand_name: String,
    /// The installed app's label where space is short; 1 to 32 characters, the name by default.
    #[arg(long, env = "RECTO_BRAND_SHORT_NAME")]
    pub brand_short_name: Option<String>,
    /// The accent color, `#rrggbb`, dark enough for white text on it (4.5:1).
    #[arg(long, env = "RECTO_BRAND_COLOR", default_value = "#0f6cbd")]
    pub brand_color: String,
    /// A PNG, JPEG or SVG file of at most 1 MiB shown in the header instead of the glyph.
    #[arg(long, env = "RECTO_BRAND_LOGO")]
    pub brand_logo: Option<PathBuf>,
    /// A square PNG or JPEG file of 512 to 4096 pixels a side and at most 5 MiB for the app
    /// icons, which are otherwise drawn in the accent color.
    #[arg(long, env = "RECTO_BRAND_ICON")]
    pub brand_icon: Option<PathBuf>,
}

impl Default for BrandOptions {
    fn default() -> Self {
        Self {
            brand_name: DEFAULT_NAME.to_owned(),
            brand_short_name: None,
            brand_color: DEFAULT_COLOR.to_owned(),
            brand_logo: None,
            brand_icon: None,
        }
    }
}

/// Checks the settings and reads the files. The error names the variable and its value or path.
pub fn load(options: &BrandOptions) -> Result<Branding, String> {
    let name = bounded("RECTO_BRAND_NAME", &options.brand_name, 64)?;
    let short_name = match &options.brand_short_name {
        Some(text) => bounded("RECTO_BRAND_SHORT_NAME", text, 32)?,
        None => name.clone(),
    };
    let rgb = parse_color(&options.brand_color).ok_or_else(|| {
        format!(
            "RECTO_BRAND_COLOR must be a color written #rrggbb, such as {DEFAULT_COLOR}, not {:?}",
            options.brand_color
        )
    })?;
    let ratio = contrast(WHITE, rgb);
    if ratio < MINIMUM_CONTRAST {
        // Rounded down, so a ratio just short of the minimum never reads as reaching it.
        let shown = (ratio * 100.0).floor() / 100.0;
        return Err(format!(
            "RECTO_BRAND_COLOR {} gives white button text a contrast of {shown:.2}:1; it needs \
             4.5:1. Choose a darker color.",
            options.brand_color
        ));
    }
    let color = hex(rgb);
    // The stylesheet's own values for the default were tuned by hand; derived ones would differ.
    let style = (color != DEFAULT_COLOR).then(|| accent_style(rgb));
    let logo = options.brand_logo.as_deref().map(read_logo).transpose()?;
    let icons = match options.brand_icon.as_deref() {
        Some(path) => custom_icons(&read_icon(path)?, rgb),
        None => default_icons(rgb),
    };
    Ok(Branding {
        name,
        short_name,
        color,
        style,
        logo,
        icons,
    })
}

/// The branding without any setting: recto's name, color and icon.
pub fn default_branding() -> Branding {
    load(&BrandOptions::default()).expect("the default branding is valid")
}

/// `text` trimmed, refused when empty or longer than `limit` characters.
fn bounded(variable: &str, text: &str, limit: usize) -> Result<String, String> {
    let trimmed = text.trim();
    let length = trimmed.chars().count();
    if (1..=limit).contains(&length) {
        Ok(trimmed.to_owned())
    } else {
        Err(format!(
            "{variable} must be 1 to {limit} characters long, not {length}: {text:?}"
        ))
    }
}

/// `#rrggbb`, in either case; nothing else, so the value can go into CSS and HTML as is.
fn parse_color(text: &str) -> Option<[u8; 3]> {
    let digits = text.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

fn hex(color: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
}

/// The relative luminance of WCAG 2.
fn luminance(color: [u8; 3]) -> f64 {
    let linear = |channel: u8| {
        let value = f64::from(channel) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color[0]) + 0.7152 * linear(color[1]) + 0.0722 * linear(color[2])
}

/// The contrast ratio of WCAG 2, from 1 to 21, whichever color is lighter.
fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `a` moved towards `b` by `t`, channel by channel.
fn mix(a: [u8; 3], b: [u8; 3], t: f64) -> [u8; 3] {
    let channel = |index: usize| {
        let value = f64::from(a[index]) * (1.0 - t) + f64::from(b[index]) * t;
        // Between two bytes, so it fits.
        value.round() as u8
    };
    [channel(0), channel(1), channel(2)]
}

/// The accent tokens of `style/main.css` for another color, under the same selectors so that,
/// coming after the stylesheet, they win. Buttons keep the color in both themes (white text on
/// it was checked at startup). Accent text (`--brand`) sits on the cards, the page and its own
/// subtle background: it is darkened in the light theme and lightened in the dark one, by the
/// least step that reads on all of them.
fn accent_style(color: [u8; 3]) -> String {
    let (light, dark) = (light_accent(color).css(), dark_accent(color).css());
    format!(
        ":root{{{light}}}\
         @media (prefers-color-scheme: dark){{:root:not([data-theme=\"light\"]){{{dark}}}}}\
         :root[data-theme=\"dark\"]{{{dark}}}"
    )
}

/// The accent tokens of one theme.
struct Accent {
    primary: [u8; 3],
    primary_hover: [u8; 3],
    /// Text, on the cards, the page and `brand_subtle`.
    brand: [u8; 3],
    brand_subtle: [u8; 3],
    /// The focus ring, as visible as the accent text.
    ring: [u8; 3],
}

impl Accent {
    /// The tokens of a theme whose accent text is `color` moved towards `toward` until it
    /// reaches the minimum contrast on every surface, the theme's own subtle background
    /// included.
    fn new(color: [u8; 3], brand_subtle: [u8; 3], toward: [u8; 3], surfaces: [[u8; 3]; 2]) -> Self {
        let brand = (0..=20)
            .map(|step| mix(color, toward, f64::from(step) / 20.0))
            .find(|brand| {
                [brand_subtle, surfaces[0], surfaces[1]]
                    .iter()
                    .all(|surface| contrast(*brand, *surface) >= MINIMUM_CONTRAST)
            })
            // Black or white always reads on these surfaces; this is never reached.
            .unwrap_or(toward);
        Self {
            primary: color,
            primary_hover: mix(color, BLACK, 0.12),
            brand,
            brand_subtle,
            ring: brand,
        }
    }

    fn css(&self) -> String {
        format!(
            "--primary:{};--primary-hover:{};--brand:{};--brand-subtle:{};--ring:{};",
            hex(self.primary),
            hex(self.primary_hover),
            hex(self.brand),
            hex(self.brand_subtle),
            hex(self.ring)
        )
    }
}

fn light_accent(color: [u8; 3]) -> Accent {
    let subtle = mix(color, WHITE, 0.92);
    Accent::new(color, subtle, BLACK, [WHITE, LIGHT_BACKGROUND])
}

fn dark_accent(color: [u8; 3]) -> Accent {
    let subtle = mix(color, DARK_BACKGROUND, 0.8);
    Accent::new(color, subtle, WHITE, [DARK_CARD, DARK_BACKGROUND])
}

/// The file at `path`, refused beyond `limit` bytes without reading further.
fn read_limited(variable: &str, path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let unreadable =
        |error: std::io::Error| format!("{variable} {} cannot be read: {error}", path.display());
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(limit + 1).read_to_end(&mut bytes))
        .map_err(unreadable)?;
    if bytes.len() as u64 > limit {
        return Err(format!(
            "{variable} {} is larger than {} MiB",
            path.display(),
            limit / MEBIBYTE
        ));
    }
    Ok(bytes)
}

/// The logo, typed by its content: a misnamed file still gets the right type, and anything
/// else is refused rather than served under an image type.
fn read_logo(path: &Path) -> Result<BrandFile, String> {
    let bytes = read_limited("RECTO_BRAND_LOGO", path, LOGO_LIMIT)?;
    let content_type = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if std::str::from_utf8(&bytes).is_ok_and(|text| text.contains("<svg")) {
        "image/svg+xml"
    } else {
        return Err(format!(
            "RECTO_BRAND_LOGO {} is not a PNG, JPEG or SVG image",
            path.display()
        ));
    };
    Ok(BrandFile {
        bytes: Bytes::from(bytes),
        content_type,
    })
}

fn read_icon(path: &Path) -> Result<DynamicImage, String> {
    let bytes = read_limited("RECTO_BRAND_ICON", path, ICON_LIMIT)?;
    let format = image::guess_format(&bytes)
        .ok()
        .filter(|format| matches!(format, ImageFormat::Png | ImageFormat::Jpeg))
        .ok_or_else(|| {
            format!(
                "RECTO_BRAND_ICON {} is not a PNG or JPEG image",
                path.display()
            )
        })?;
    let undecodable = |error: image::ImageError| {
        format!(
            "RECTO_BRAND_ICON {} cannot be decoded: {error}",
            path.display()
        )
    };
    // The size, from the header alone, is checked before the pixels are decoded.
    let (width, height) = ImageReader::with_format(Cursor::new(&bytes), format)
        .into_dimensions()
        .map_err(undecodable)?;
    if width > ICON_MAXIMUM || height > ICON_MAXIMUM {
        return Err(format!(
            "RECTO_BRAND_ICON {} is {width} by {height} pixels; it must be at most \
             {ICON_MAXIMUM} by {ICON_MAXIMUM}",
            path.display()
        ));
    }
    if width != height {
        return Err(format!(
            "RECTO_BRAND_ICON {} is {width} by {height} pixels; it must be square",
            path.display()
        ));
    }
    if width < ICON_MINIMUM {
        return Err(format!(
            "RECTO_BRAND_ICON {} is {width} by {height} pixels; it must be at least \
             {ICON_MINIMUM} by {ICON_MINIMUM}",
            path.display()
        ));
    }
    image::load_from_memory_with_format(&bytes, format).map_err(undecodable)
}

/// The operator's image as the icons. The maskable one keeps it to the middle 80 %, the part
/// no device crops, on the accent color.
fn custom_icons(image: &DynamicImage, color: [u8; 3]) -> BrandIcons {
    let mut maskable = RgbaImage::from_pixel(512, 512, opaque(color));
    imageops::overlay(&mut maskable, &resize(image, 410), 51, 51);
    icons(&resize(image, 192), &resize(image, 512), maskable)
}

/// recto's own icon in the accent color: a white square on a rounded square.
fn default_icons(color: [u8; 3]) -> BrandIcons {
    icons(
        &draw(192, color, false),
        &draw(512, color, false),
        draw(512, color, true),
    )
}

fn icons(any_192: &RgbaImage, any_512: &RgbaImage, maskable_512: RgbaImage) -> BrandIcons {
    let maskable_512 = DynamicImage::ImageRgba8(maskable_512);
    BrandIcons {
        any_192: png(any_192),
        any_512: png(any_512),
        apple_180: png(&resize(&maskable_512, 180)),
        maskable_512: png(&maskable_512.into_rgba8()),
    }
}

/// `image` as a `size` square. Through `DynamicImage`, whose methods are compiled with the
/// `image` crate's optimizations even in debug builds (see the workspace's `Cargo.toml`).
fn resize(image: &DynamicImage, size: u32) -> RgbaImage {
    image
        .resize_exact(size, size, FilterType::Lanczos3)
        .into_rgba8()
}

fn opaque(color: [u8; 3]) -> Rgba<u8> {
    Rgba([color[0], color[1], color[2], 255])
}

fn png(image: &RgbaImage) -> Bytes {
    let mut out = Cursor::new(Vec::new());
    image
        .write_to(&mut out, ImageFormat::Png)
        .expect("encoding a PNG in memory cannot fail");
    Bytes::from(out.into_inner())
}

/// A square from `start` to `end` on both axes with corners of `radius`.
struct RoundedSquare {
    start: f64,
    end: f64,
    radius: f64,
}

impl RoundedSquare {
    fn contains(&self, x: f64, y: f64) -> bool {
        let (low, high) = (self.start + self.radius, self.end - self.radius);
        let (dx, dy) = (x - x.clamp(low, high), y - y.clamp(low, high));
        x >= self.start
            && x <= self.end
            && y >= self.start
            && y <= self.end
            && dx * dx + dy * dy <= self.radius * self.radius
    }
}

/// The icon drawn on a `size` canvas, in units of 1/18 of it as the original 18-pixel design:
/// a rounded square of the color (or, `full`, the whole canvas, for the maskable icon, which
/// the device crops to its own shape) around a white rounded square. Pixels on an edge average
/// 4 by 4 samples, so the curves are smooth.
fn draw(size: u32, color: [u8; 3], full: bool) -> RgbaImage {
    const SAMPLES: u32 = 4;
    let unit = f64::from(size) / 18.0;
    let outer = RoundedSquare {
        start: 0.0,
        end: f64::from(size),
        radius: 5.0 * unit,
    };
    let inner = RoundedSquare {
        start: 5.0 * unit,
        end: 13.0 * unit,
        radius: 1.5 * unit,
    };
    let paint = |x: f64, y: f64| {
        if inner.contains(x, y) {
            Some(WHITE)
        } else if full || outer.contains(x, y) {
            Some(color)
        } else {
            None
        }
    };
    RgbaImage::from_fn(size, size, |x, y| {
        let (x, y) = (f64::from(x), f64::from(y));
        // Both shapes are axis-aligned rounded rectangles much larger than a pixel, so a pixel
        // whose four corners get the same paint lies inside one region and has that paint.
        let corner = paint(x, y);
        if [(1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
            .into_iter()
            .all(|(dx, dy)| paint(x + dx, y + dy) == corner)
        {
            return corner.map_or(Rgba([0; 4]), opaque);
        }
        let mut sums = [0.0; 3];
        let mut covered = 0;
        for i in 0..SAMPLES {
            for j in 0..SAMPLES {
                let offset = |index: u32| (f64::from(index) + 0.5) / f64::from(SAMPLES);
                let Some(paint) = paint(x + offset(i), y + offset(j)) else {
                    continue;
                };
                for (sum, channel) in sums.iter_mut().zip(paint) {
                    *sum += f64::from(channel);
                }
                covered += 1;
            }
        }
        if covered == 0 {
            return Rgba([0; 4]);
        }
        let average = |sum: f64| (sum / f64::from(covered)).round() as u8;
        let alpha = (f64::from(covered) * 255.0 / f64::from(SAMPLES * SAMPLES)).round() as u8;
        Rgba([average(sums[0]), average(sums[1]), average(sums[2]), alpha])
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use image::{ImageFormat, RgbaImage};
    use recto_domain::testing::temporary_directory;

    use super::*;

    fn options(change: impl FnOnce(&mut BrandOptions)) -> BrandOptions {
        let mut options = BrandOptions::default();
        change(&mut options);
        options
    }

    fn decode(png: &[u8]) -> RgbaImage {
        image::load_from_memory_with_format(png, ImageFormat::Png)
            .unwrap()
            .to_rgba8()
    }

    fn write_png(path: &Path, width: u32, height: u32) {
        RgbaImage::from_pixel(width, height, image::Rgba([200, 30, 30, 255]))
            .save_with_format(path, ImageFormat::Png)
            .unwrap();
    }

    #[test]
    fn contrast_follows_wcag() {
        assert!((contrast([255; 3], [0; 3]) - 21.0).abs() < 0.01);
        assert!(contrast([255; 3], parse_color("#0f6cbd").unwrap()) >= 4.5);
    }

    #[test]
    fn colors_are_six_hex_digits() {
        assert_eq!(parse_color("#0F6CBD"), Some([15, 108, 189]));
        for bad in [
            "0f6cbd", "#fff", "#0f6cbg", "blue", "", "#0f6cbd0", "#+f6cbd",
        ] {
            assert_eq!(parse_color(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_light_accent_is_refused() {
        let error = load(&options(|o| o.brand_color = "#ffff00".into())).unwrap_err();
        assert!(error.contains("RECTO_BRAND_COLOR"), "{error}");
        assert!(error.contains("#ffff00"), "{error}");
        assert!(error.contains("1.07:1"), "{error}");
    }

    #[test]
    fn the_default_color_keeps_the_stylesheet() {
        let branding = default_branding();
        assert_eq!(branding.style, None);
        assert_eq!(branding.color, DEFAULT_COLOR);
        assert_eq!(branding.name, "recto");
        assert_eq!(branding.short_name, "recto");
        // Written in capitals, it is still the default.
        let upper = load(&options(|o| o.brand_color = "#0F6CBD".into())).unwrap();
        assert_eq!(upper.style, None);
        assert_eq!(upper.color, DEFAULT_COLOR);
    }

    #[test]
    fn an_accent_covers_both_themes() {
        let style = accent_style(parse_color("#8a2be2").unwrap());
        let light = style.find(":root{").unwrap();
        let media = style.find(":root:not([data-theme=\"light\"])").unwrap();
        let dark = style.find(":root[data-theme=\"dark\"]").unwrap();
        assert!(light < media && media < dark, "{style}");
        assert!(
            style.contains("@media (prefers-color-scheme: dark)"),
            "{style}"
        );
        assert!(style[..media].contains("--primary:#8a2be2;"), "{style}");
        assert!(
            style[..media].contains("--brand-subtle:#f6eefd;"),
            "{style}"
        );
        // The dark --brand must stay readable on the dark card.
        let brand = style[dark..]
            .split("--brand:#")
            .nth(1)
            .map(|rest| &rest[..6])
            .unwrap();
        let brand = parse_color(&format!("#{brand}")).unwrap();
        assert!(contrast(brand, [0x29; 3]) >= 4.5, "{style}");
        assert!(style[dark..].contains(&format!(
            "--ring:#{:02x}{:02x}{:02x}",
            brand[0], brand[1], brand[2]
        )));
        // Both dark blocks hold the same tokens.
        let media_block = &style[media..dark];
        let dark_block = &style[dark..];
        let tokens = |block: &str| block[block.find('{').unwrap()..].replace('}', "");
        assert_eq!(tokens(media_block), tokens(dark_block));
        assert_eq!(
            load(&options(|o| o.brand_color = "#8a2be2".into()))
                .unwrap()
                .style
                .as_deref(),
            Some(style.as_str())
        );
    }

    #[test]
    fn accent_text_reads_on_every_surface() {
        let mut checked = 0;
        for red in (0..=255).step_by(17) {
            for green in (0..=255).step_by(17) {
                for blue in (0..=255).step_by(17) {
                    let color = [red, green, blue];
                    // Only colors the startup check accepts.
                    if contrast(WHITE, color) < MINIMUM_CONTRAST {
                        continue;
                    }
                    checked += 1;
                    let light = light_accent(color);
                    let dark = dark_accent(color);
                    for (theme, accent, surfaces) in [
                        (
                            "light",
                            &light,
                            [WHITE, LIGHT_BACKGROUND, light.brand_subtle],
                        ),
                        (
                            "dark",
                            &dark,
                            [DARK_CARD, DARK_BACKGROUND, dark.brand_subtle],
                        ),
                    ] {
                        for surface in surfaces {
                            let ratio = contrast(accent.brand, surface);
                            assert!(
                                ratio >= MINIMUM_CONTRAST,
                                "{theme} {} on {}: {ratio:.2}",
                                hex(accent.brand),
                                hex(surface)
                            );
                        }
                        assert_eq!(accent.ring, accent.brand);
                        assert_eq!(accent.primary, color);
                    }
                }
            }
        }
        assert!(checked > 1000, "{checked}");
        // A color that already reads is kept as it is in the light theme.
        assert_eq!(light_accent([0x0f, 0x6c, 0xbd]).brand, [0x0f, 0x6c, 0xbd]);
    }

    #[test]
    fn names_are_bounded() {
        for bad in ["", "   ", &"n".repeat(65)] {
            let error = load(&options(|o| o.brand_name = bad.to_owned())).unwrap_err();
            assert!(error.contains("RECTO_BRAND_NAME"), "{error}");
        }
        let error = load(&options(|o| o.brand_short_name = Some("s".repeat(33)))).unwrap_err();
        assert!(error.contains("RECTO_BRAND_SHORT_NAME"), "{error}");
        let branding = load(&options(|o| o.brand_name = "  Maison Dupont ".into())).unwrap();
        assert_eq!(branding.name, "Maison Dupont");
        assert_eq!(branding.short_name, "Maison Dupont");
        let branding = load(&options(|o| {
            o.brand_name = "é".repeat(64);
            o.brand_short_name = Some(" Maison ".into());
        }))
        .unwrap();
        assert_eq!(branding.short_name, "Maison");
    }

    #[test]
    fn default_icons_have_their_sizes() {
        let icons = default_branding().icons;
        for (png, size) in [
            (&icons.any_192, 192),
            (&icons.any_512, 512),
            (&icons.maskable_512, 512),
            (&icons.apple_180, 180),
        ] {
            let image = decode(png);
            assert_eq!(image.dimensions(), (size, size));
        }
        let any = decode(&icons.any_512);
        assert_eq!(any.get_pixel(0, 0)[3], 0);
        assert_eq!(any.get_pixel(40, 256).0, [0x0f, 0x6c, 0xbd, 255]);
        assert_eq!(any.get_pixel(256, 256).0, [255; 4]);
        let maskable = decode(&icons.maskable_512);
        assert_eq!(maskable.get_pixel(0, 0).0, [0x0f, 0x6c, 0xbd, 255]);
        assert_eq!(maskable.get_pixel(256, 256).0, [255; 4]);
    }

    #[test]
    fn a_custom_icon_must_be_square_and_large() {
        let dir = temporary_directory();
        for (name, width, height) in [("wide.png", 512, 400), ("small.png", 256, 256)] {
            let path = dir.path().join(name);
            write_png(&path, width, height);
            let error = load(&options(|o| o.brand_icon = Some(path.clone()))).unwrap_err();
            assert!(error.contains("RECTO_BRAND_ICON"), "{error}");
            assert!(error.contains(name), "{error}");
        }
        let text = dir.path().join("icon.png");
        std::fs::write(&text, "not an image").unwrap();
        let error = load(&options(|o| o.brand_icon = Some(text))).unwrap_err();
        assert!(error.contains("RECTO_BRAND_ICON"), "{error}");
        let missing = dir.path().join("missing.png");
        let error = load(&options(|o| o.brand_icon = Some(missing))).unwrap_err();
        assert!(error.contains("missing.png"), "{error}");

        let path = dir.path().join("square.png");
        write_png(&path, 600, 600);
        let icons = load(&options(|o| {
            o.brand_icon = Some(path);
            o.brand_color = "#8a2be2".into();
        }))
        .unwrap()
        .icons;
        let any = decode(&icons.any_192);
        assert_eq!(any.dimensions(), (192, 192));
        assert_eq!(any.get_pixel(0, 0).0, [200, 30, 30, 255]);
        let maskable = decode(&icons.maskable_512);
        // The image keeps to the middle 80 %, on the accent.
        assert_eq!(maskable.get_pixel(0, 0).0, [0x8a, 0x2b, 0xe2, 255]);
        assert_eq!(maskable.get_pixel(256, 256).0, [200, 30, 30, 255]);
        assert_eq!(decode(&icons.apple_180).dimensions(), (180, 180));
    }

    /// A PNG with only a header claiming `size` by `size` pixels: what a tiny file that
    /// decompresses into a huge image looks like before decoding.
    fn png_header(size: u32) -> Vec<u8> {
        fn crc(bytes: &[u8]) -> u32 {
            let mut crc = !0u32;
            for byte in bytes {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = if crc & 1 == 1 {
                        (crc >> 1) ^ 0xedb8_8320
                    } else {
                        crc >> 1
                    };
                }
            }
            !crc
        }
        fn chunk(png: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
            png.extend((data.len() as u32).to_be_bytes());
            let body = [kind, data].concat();
            png.extend(&body);
            png.extend(crc(&body).to_be_bytes());
        }
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut header = [size.to_be_bytes(), size.to_be_bytes()].concat();
        header.extend([8, 6, 0, 0, 0]);
        chunk(&mut png, b"IHDR", &header);
        chunk(&mut png, b"IDAT", &[]);
        chunk(&mut png, b"IEND", &[]);
        png
    }

    #[test]
    fn a_huge_icon_is_refused_before_decoding() {
        let dir = temporary_directory();
        let path = dir.path().join("huge.png");
        std::fs::write(&path, png_header(11_000)).unwrap();
        let error = load(&options(|o| o.brand_icon = Some(path))).unwrap_err();
        assert!(error.contains("RECTO_BRAND_ICON"), "{error}");
        assert!(error.contains("huge.png"), "{error}");
        assert!(error.contains("at most 4096 by 4096"), "{error}");
    }

    #[test]
    fn a_logo_is_typed_by_its_content() {
        let dir = temporary_directory();
        let svg = dir.path().join("logo.png");
        std::fs::write(&svg, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#).unwrap();
        let logo = load(&options(|o| o.brand_logo = Some(svg)))
            .unwrap()
            .logo
            .unwrap();
        assert_eq!(logo.content_type, "image/svg+xml");

        let png = dir.path().join("logo.svg");
        write_png(&png, 4, 4);
        let logo = load(&options(|o| o.brand_logo = Some(png)))
            .unwrap()
            .logo
            .unwrap();
        assert_eq!(logo.content_type, "image/png");

        let jpeg = dir.path().join("logo");
        image::RgbImage::new(4, 4)
            .save_with_format(&jpeg, ImageFormat::Jpeg)
            .unwrap();
        let logo = load(&options(|o| o.brand_logo = Some(jpeg)))
            .unwrap()
            .logo
            .unwrap();
        assert_eq!(logo.content_type, "image/jpeg");

        let large = dir.path().join("large.svg");
        let mut content = b"<svg>".to_vec();
        content.resize(1024 * 1024 + 1, b' ');
        std::fs::write(&large, content).unwrap();
        let error = load(&options(|o| o.brand_logo = Some(large))).unwrap_err();
        assert!(error.contains("RECTO_BRAND_LOGO"), "{error}");
        assert!(error.contains("large.svg"), "{error}");

        let text = dir.path().join("logo.txt");
        std::fs::write(&text, "hello").unwrap();
        let error = load(&options(|o| o.brand_logo = Some(text))).unwrap_err();
        assert!(error.contains("RECTO_BRAND_LOGO"), "{error}");
        assert!(error.contains("logo.txt"), "{error}");
    }
}
