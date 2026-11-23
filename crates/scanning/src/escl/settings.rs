//! `ScanSettings`, the body of `POST ScanJobs`, read into a recto scan.

use quick_xml::NsReader;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use thiserror::Error;

use super::{PWG, SCAN, Sources, full_area, modes, resolutions, units_to_mm};
use crate::{Area, OutputFormat, ScanMode, ScanRequest, ScanSettings, ScannerCapabilities};

/// The largest `ScanSettings` recto reads. Real ones are under 2 KiB.
pub const MAX_SETTINGS_BYTES: usize = 64 * 1024;

/// Deeper than any eSCL document (`ScanSettings/ScanRegions/ScanRegion/Width` is 4).
const MAX_DEPTH: usize = 16;

/// What the scan is for, as the client says. It picks the defaults of what the client left out,
/// and a `Preview` is worth knowing in the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Document,
    TextAndGraphic,
    Text,
    Photo,
    Preview,
    /// Any other intent, which picks the same defaults as none.
    Other,
}

impl Intent {
    fn from_escl(name: &str) -> Self {
        match name {
            "Document" => Self::Document,
            "TextAndGraphic" => Self::TextAndGraphic,
            "Text" => Self::Text,
            "Photo" => Self::Photo,
            "Preview" => Self::Preview,
            _ => Self::Other,
        }
    }

    fn is_text(self) -> bool {
        matches!(self, Self::Document | Self::TextAndGraphic | Self::Text)
    }
}

/// A `ScanSettings` snapped to what the scanner offers.
#[derive(Debug, Clone, PartialEq)]
pub struct EsclRequest {
    pub settings: ScanSettings,
    pub format: OutputFormat,
    pub intent: Option<Intent>,
    /// Whether the source is a feeder, whose emptiness ends a job rather than failing it.
    pub feeder: bool,
}

impl EsclRequest {
    /// The scan to run, with [`ScannerService::scan_every_page`](crate::ScannerService::scan_every_page):
    /// each PNG or JPEG page of a feeder batch is a document of its own. A preview reads one
    /// sheet only: it must never pull a whole stack through the feeder.
    pub fn scan_request(&self) -> ScanRequest {
        ScanRequest {
            settings: self.settings.clone(),
            format: self.format,
            single_page: self.intent == Some(Intent::Preview),
        }
    }
}

/// Why a `ScanSettings` was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SettingsError {
    /// Not a `ScanSettings` recto reads: too large, not well formed, with a DOCTYPE or an unknown
    /// entity, or a value of the wrong kind. HTTP 400.
    #[error("malformed scan settings: {0}")]
    Malformed(String),
    /// Well formed, but asking for something the scanner does not offer. HTTP 409.
    #[error("the scanner cannot do this: {0}")]
    Unsupported(String),
    /// A page would exceed [`MAX_PAGE_BYTES`](crate::MAX_PAGE_BYTES). HTTP 409.
    #[error("the scan is too large")]
    TooLarge,
}

fn malformed(reason: impl std::fmt::Display) -> SettingsError {
    SettingsError::Malformed(reason.to_string())
}

fn unsupported(reason: impl std::fmt::Display) -> SettingsError {
    SettingsError::Unsupported(reason.to_string())
}

/// The fields recto reads, as the client wrote them.
#[derive(Default)]
struct Fields {
    intent: Option<String>,
    input_source: Option<String>,
    duplex: Option<String>,
    color_mode: Option<String>,
    x_resolution: Option<String>,
    y_resolution: Option<String>,
    document_format: Option<String>,
    document_format_ext: Option<String>,
    units: Option<String>,
    x_offset: Option<String>,
    y_offset: Option<String>,
    width: Option<String>,
    height: Option<String>,
}

/// An open element: whether it is eSCL's (either namespace: clients disagree on which one some
/// elements belong to), its local name and its text so far.
struct Open {
    escl: bool,
    name: String,
    text: String,
}

fn is_escl(namespace: &ResolveResult<'_>) -> bool {
    matches!(namespace, ResolveResult::Bound(Namespace(ns)) if *ns == PWG || *ns == SCAN)
}

/// Reads the document into [`Fields`], refusing anything that is not a plain eSCL document.
/// quick-xml never expands entities beyond the predefined ones; a DOCTYPE, which could declare
/// some, stops the parse.
fn read(xml: &[u8]) -> Result<Fields, SettingsError> {
    if xml.len() > MAX_SETTINGS_BYTES {
        return Err(malformed("larger than 64 KiB"));
    }
    let xml = std::str::from_utf8(xml).map_err(|_| malformed("not UTF-8"))?;
    let mut reader = NsReader::from_str(xml);
    let mut fields = Fields::default();
    let mut open: Vec<Open> = Vec::new();
    let mut root = false;
    let mut regions = 0;
    loop {
        let (namespace, event) = reader.read_resolved_event().map_err(malformed)?;
        let escl = is_escl(&namespace);
        match event {
            Event::Start(element) => start(&mut open, &mut root, escl, &element)?,
            Event::Empty(element) => {
                start(&mut open, &mut root, escl, &element)?;
                end(&mut fields, &mut open, &mut regions)?;
            }
            Event::End(_) => end(&mut fields, &mut open, &mut regions)?,
            Event::DocType(_) => return Err(malformed("DOCTYPE")),
            Event::Text(text) => {
                if let Some(top) = open.last_mut() {
                    top.text.push_str(&text.xml10_content());
                }
            }
            Event::CData(text) => {
                if let Some(top) = open.last_mut() {
                    top.text.push_str(&text.xml10_content());
                }
            }
            Event::GeneralRef(reference) => {
                let resolved = match reference.resolve_char_ref().map_err(malformed)? {
                    Some(c) => c.to_string(),
                    None => resolve_predefined_entity(&reference.xml10_content())
                        .ok_or_else(|| malformed("unknown entity"))?
                        .to_owned(),
                };
                if let Some(top) = open.last_mut() {
                    top.text.push_str(&resolved);
                }
            }
            Event::Eof => break,
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) => {}
        }
    }
    if !root {
        return Err(malformed("no ScanSettings"));
    }
    if !open.is_empty() {
        return Err(malformed("truncated"));
    }
    Ok(fields)
}

fn start(
    open: &mut Vec<Open>,
    root: &mut bool,
    escl: bool,
    element: &BytesStart<'_>,
) -> Result<(), SettingsError> {
    if *root && open.is_empty() {
        return Err(malformed("content after ScanSettings"));
    }
    if open.len() >= MAX_DEPTH {
        return Err(malformed("nested too deep"));
    }
    let local = element.local_name();
    let name: &str = local.as_ref();
    if open.is_empty() {
        if !(escl && name == "ScanSettings") {
            return Err(malformed("not a ScanSettings document"));
        }
        *root = true;
    }
    open.push(Open {
        escl,
        name: name.to_owned(),
        text: String::new(),
    });
    Ok(())
}

fn end(
    fields: &mut Fields,
    open: &mut Vec<Open>,
    regions: &mut usize,
) -> Result<(), SettingsError> {
    let element = open.pop().ok_or_else(|| malformed("unbalanced"))?;
    keep(fields, open, element, regions);
    Ok(())
}

/// Stores the text of a closed eSCL element that recto reads, by its place in the document.
fn keep(fields: &mut Fields, parents: &[Open], element: Open, regions: &mut usize) {
    if !element.escl || parents.iter().any(|p| !p.escl) {
        return;
    }
    let path: Vec<&str> = parents.iter().map(|p| p.name.as_str()).collect();
    let value = Some(element.text.trim().to_owned());
    match (path.as_slice(), element.name.as_str()) {
        (["ScanSettings"], name) => {
            let slot = match name {
                "Intent" => &mut fields.intent,
                "InputSource" => &mut fields.input_source,
                "Duplex" => &mut fields.duplex,
                "ColorMode" => &mut fields.color_mode,
                "XResolution" => &mut fields.x_resolution,
                "YResolution" => &mut fields.y_resolution,
                "DocumentFormat" => &mut fields.document_format,
                "DocumentFormatExt" => &mut fields.document_format_ext,
                _ => return,
            };
            *slot = value;
        }
        (["ScanSettings", "ScanRegions"], "ScanRegion") => *regions += 1,
        // Only the first region: recto offers one.
        (["ScanSettings", "ScanRegions", "ScanRegion"], name) if *regions == 0 => {
            let slot = match name {
                "ContentRegionUnits" => &mut fields.units,
                "XOffset" => &mut fields.x_offset,
                "YOffset" => &mut fields.y_offset,
                "Width" => &mut fields.width,
                "Height" => &mut fields.height,
                _ => return,
            };
            *slot = value;
        }
        _ => {}
    }
}

fn number(name: &str, value: &Option<String>) -> Result<Option<u32>, SettingsError> {
    value
        .as_deref()
        .map(|v| v.parse().map_err(|_| malformed(format!("{name} {v:?}"))))
        .transpose()
}

/// The source to scan from and whether it is a feeder.
fn source(
    fields: &Fields,
    caps: &ScannerCapabilities,
) -> Result<(Option<String>, bool), SettingsError> {
    let duplex = match fields.duplex.as_deref() {
        None | Some("false" | "0") => false,
        Some("true" | "1") => true,
        Some(other) => return Err(malformed(format!("Duplex {other:?}"))),
    };
    let sources = Sources::of(caps);
    let platen = || {
        sources
            .platen
            .map(|s| (s.map(|s| s.name.clone()), false))
            .ok_or_else(|| unsupported("no platen"))
    };
    let feeder = || {
        let source = if duplex {
            sources.duplex.ok_or_else(|| unsupported("no duplex"))?
        } else {
            sources.simplex.ok_or_else(|| unsupported("no feeder"))?
        };
        Ok((Some(source.name.clone()), true))
    };
    match fields.input_source.as_deref() {
        Some("Platen") => platen(),
        Some("Feeder") => feeder(),
        None if sources.platen.is_some() => platen(),
        None => feeder(),
        Some(other) => Err(unsupported(format!("source {other:?}"))),
    }
}

fn mode(
    fields: &Fields,
    intent: Option<Intent>,
    caps: &ScannerCapabilities,
) -> Result<ScanMode, SettingsError> {
    let offered = modes(caps);
    let Some(name) = fields.color_mode.as_deref() else {
        let wanted = if intent == Some(Intent::Photo) {
            ScanMode::Color
        } else {
            ScanMode::Gray
        };
        return Ok(if offered.contains(&wanted) {
            wanted
        } else {
            offered[0]
        });
    };
    // recto scans 8 bits a sample; deeper modes get the nearest it has.
    let mode = match name {
        "RGB24" | "RGB48" => ScanMode::Color,
        "Grayscale8" | "Grayscale16" => ScanMode::Gray,
        "BlackAndWhite1" => ScanMode::Lineart,
        other => return Err(unsupported(format!("colour mode {other:?}"))),
    };
    if offered.contains(&mode) {
        Ok(mode)
    } else {
        Err(unsupported(format!("colour mode {name}")))
    }
}

/// The offered resolution nearest the one asked for, the higher of two equally near.
fn resolution(fields: &Fields, caps: &ScannerCapabilities) -> Result<u32, SettingsError> {
    let x = number("XResolution", &fields.x_resolution)?;
    let y = number("YResolution", &fields.y_resolution)?;
    let wanted = x.or(y).unwrap_or(300);
    let offered = resolutions(caps);
    Ok(offered
        .iter()
        .copied()
        .min_by_key(|&r| (r.abs_diff(wanted), std::cmp::Reverse(r)))
        .unwrap_or(300))
}

/// The region in millimetres on the glass, clamped to it; the whole glass when none is given.
fn area(fields: &Fields, caps: &ScannerCapabilities) -> Result<Option<Area>, SettingsError> {
    let x = number("XOffset", &fields.x_offset)?.unwrap_or(0);
    let y = number("YOffset", &fields.y_offset)?.unwrap_or(0);
    let width = number("Width", &fields.width)?.unwrap_or(0);
    let height = number("Height", &fields.height)?.unwrap_or(0);
    match fields.units.as_deref() {
        None | Some("escl:ThreeHundredthsOfInches" | "ThreeHundredthsOfInches") => {}
        Some(other) => return Err(unsupported(format!("units {other:?}"))),
    }
    // Without geometry in millimetres SANE cannot set a window; the scanner scans what it does.
    let Some(full) = caps.area else {
        return Ok(None);
    };
    if width == 0 || height == 0 {
        return Ok(Some(full));
    }
    let left = (full.left + units_to_mm(x)).min(full.right);
    let top = (full.top + units_to_mm(y)).min(full.bottom);
    let region = Area {
        left,
        top,
        right: (left + units_to_mm(width)).min(full.right),
        bottom: (top + units_to_mm(height)).min(full.bottom),
    };
    if region.width() <= 0.0 || region.height() <= 0.0 {
        return Err(unsupported("the region is off the glass"));
    }
    Ok(Some(region))
}

fn format(fields: &Fields, intent: Option<Intent>) -> Result<OutputFormat, SettingsError> {
    // The newer element wins: clients that send both mean it.
    match fields
        .document_format_ext
        .as_deref()
        .or(fields.document_format.as_deref())
    {
        Some("application/pdf") => Ok(OutputFormat::Pdf),
        Some("image/jpeg") => Ok(OutputFormat::Jpeg),
        Some("image/png") => Ok(OutputFormat::Png),
        // macOS asks for images and previews them; a PDF would break that. Documents get a PDF.
        None | Some("application/octet-stream") => Ok(if intent.is_some_and(Intent::is_text) {
            OutputFormat::Pdf
        } else {
            OutputFormat::Jpeg
        }),
        Some(other) => Err(unsupported(format!("format {other:?}"))),
    }
}

/// Reads a `ScanSettings` and snaps it to what `caps` offer: the nearest resolution, `Platen` and
/// `Feeder` to SANE sources, the region from 1/300 inch to millimetres. Every element may be
/// absent; recto picks what the intent suggests.
pub fn parse_settings(
    xml: &[u8],
    caps: &ScannerCapabilities,
) -> Result<EsclRequest, SettingsError> {
    let fields = read(xml)?;
    let intent = fields.intent.as_deref().map(Intent::from_escl);
    let (source, feeder) = source(&fields, caps)?;
    let settings = ScanSettings {
        resolution: resolution(&fields, caps)?,
        mode: mode(&fields, intent, caps)?,
        source,
        area: area(&fields, caps)?,
    };
    let page = settings.page_bytes().or_else(|| {
        // Without a known area, the default eSCL page stands for it.
        ScanSettings {
            area: Some(full_area(caps)),
            ..settings.clone()
        }
        .page_bytes()
    });
    if page.is_some_and(|bytes| crate::check_page_bytes(bytes).is_err()) {
        return Err(SettingsError::TooLarge);
    }
    Ok(EsclRequest {
        settings,
        format: format(&fields, intent)?,
        intent,
        feeder,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Area, ScanMode, Source};

    fn fixture(name: &str) -> Vec<u8> {
        let path = format!("{}/tests/fixtures/escl/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    fn a4() -> Area {
        Area {
            left: 0.0,
            top: 0.0,
            right: 215.9,
            bottom: 297.0,
        }
    }

    /// The mock scanner of the development server, with a duplex feeder.
    fn caps() -> ScannerCapabilities {
        ScannerCapabilities {
            resolutions: vec![75, 100, 150, 200, 300, 600],
            modes: vec![ScanMode::Color, ScanMode::Gray, ScanMode::Lineart],
            sources: ["Flatbed", "ADF", "ADF Duplex"]
                .iter()
                .map(|s| Source::new(s))
                .collect(),
            area: Some(a4()),
        }
    }

    fn parse(xml: &str) -> Result<EsclRequest, SettingsError> {
        parse_settings(xml.as_bytes(), &caps())
    }

    /// `body` inside a `ScanSettings` root with the usual prefixes.
    fn settings(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><scan:ScanSettings xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm" xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03"><pwg:Version>2.0</pwg:Version>{body}</scan:ScanSettings>"#
        )
    }

    fn one_region(x: u32, y: u32, width: u32, height: u32) -> String {
        format!(
            "<pwg:ScanRegion><pwg:ContentRegionUnits>escl:ThreeHundredthsOfInches</pwg:ContentRegionUnits><pwg:XOffset>{x}</pwg:XOffset><pwg:YOffset>{y}</pwg:YOffset><pwg:Width>{width}</pwg:Width><pwg:Height>{height}</pwg:Height></pwg:ScanRegion>"
        )
    }

    fn region(x: u32, y: u32, width: u32, height: u32) -> String {
        format!(
            "<pwg:ScanRegions>{}</pwg:ScanRegions>",
            one_region(x, y, width, height)
        )
    }

    fn assert_area(actual: Option<Area>, expected: Area) {
        let actual = actual.expect("an area");
        for (a, e) in [
            (actual.left, expected.left),
            (actual.top, expected.top),
            (actual.right, expected.right),
            (actual.bottom, expected.bottom),
        ] {
            assert!((a - e).abs() < 1e-6, "{actual:?} != {expected:?}");
        }
    }

    fn request(
        source: &str,
        mode: ScanMode,
        resolution: u32,
        format: OutputFormat,
        intent: Option<Intent>,
    ) -> EsclRequest {
        EsclRequest {
            settings: ScanSettings {
                resolution,
                mode,
                source: Some(source.into()),
                area: Some(a4()),
            },
            format,
            intent,
            feeder: Source::new(source).is_feeder,
        }
    }

    #[test]
    fn recorded_settings_are_understood() {
        let flatbed = parse_settings(&fixture("sane-airscan-flatbed.xml"), &caps()).unwrap();
        assert_area(
            flatbed.settings.area,
            Area {
                left: 0.0,
                top: 0.0,
                right: 1181.0 / 300.0 * 25.4,
                bottom: 591.0 / 300.0 * 25.4,
            },
        );
        assert_eq!(
            flatbed,
            EsclRequest {
                settings: ScanSettings {
                    area: flatbed.settings.area,
                    ..request("Flatbed", ScanMode::Gray, 150, OutputFormat::Png, None).settings
                },
                ..request("Flatbed", ScanMode::Gray, 150, OutputFormat::Png, None)
            }
        );

        let cases = [
            (
                "sane-airscan-adf-duplex.xml",
                request("ADF Duplex", ScanMode::Color, 300, OutputFormat::Png, None),
            ),
            (
                "sane-backends-escl.xml",
                request("Flatbed", ScanMode::Color, 300, OutputFormat::Pdf, None),
            ),
            (
                "macos-preview.xml",
                request(
                    "Flatbed",
                    ScanMode::Color,
                    75,
                    OutputFormat::Jpeg,
                    Some(Intent::Preview),
                ),
            ),
            // Everything left to recto: the platen, gray, 300 dpi, the whole glass, an image.
            (
                "minimal.xml",
                request("Flatbed", ScanMode::Gray, 300, OutputFormat::Jpeg, None),
            ),
        ];
        for (name, expected) in cases {
            let parsed = parse_settings(&fixture(name), &caps()).unwrap();
            // The full height in 1/300 inch is a little over 297 mm; it is clamped to the glass.
            assert_area(parsed.settings.area, a4());
            assert_eq!(
                EsclRequest {
                    settings: ScanSettings {
                        area: expected.settings.area,
                        ..parsed.settings.clone()
                    },
                    ..parsed
                },
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn requests_scan_without_dropping_pages() {
        let parsed = parse_settings(&fixture("sane-airscan-adf-duplex.xml"), &caps()).unwrap();
        let request = parsed.scan_request();
        assert_eq!(request.settings, parsed.settings);
        assert_eq!(request.format, OutputFormat::Png);
        assert!(!request.single_page);
        // A preview reads one sheet, even from a feeder, as on the scan page.
        let preview = EsclRequest {
            intent: Some(Intent::Preview),
            ..parsed
        };
        assert!(preview.scan_request().single_page);
    }

    #[test]
    fn resolutions_snap_to_the_nearest_offered() {
        let at = |dpi: &str| {
            parse(&settings(&format!(
                "<scan:XResolution>{dpi}</scan:XResolution><scan:YResolution>{dpi}</scan:YResolution>"
            )))
            .unwrap()
            .settings
            .resolution
        };
        assert_eq!(at("290"), 300);
        assert_eq!(at("250"), 300, "ties go up");
        assert_eq!(at("1"), 75);
        assert_eq!(at("4800"), 600);
        assert_eq!(at(" 150 "), 150);
        // X wins when they differ.
        let differ = settings(
            "<scan:XResolution>100</scan:XResolution><scan:YResolution>600</scan:YResolution>",
        );
        assert_eq!(parse(&differ).unwrap().settings.resolution, 100);
        // A scanner that does not list 300 gets the nearest one by default.
        let caps = ScannerCapabilities {
            resolutions: vec![200, 400],
            ..caps()
        };
        let minimal = parse_settings(settings("").as_bytes(), &caps).unwrap();
        assert_eq!(minimal.settings.resolution, 400);
    }

    #[test]
    fn regions_become_millimetres_on_the_glass() {
        let full_letter = parse(&settings(&region(0, 0, 2550, 3300))).unwrap();
        assert_area(
            full_letter.settings.area,
            Area {
                left: 0.0,
                top: 0.0,
                right: 215.9,
                bottom: 279.4,
            },
        );
        let offset = parse(&settings(&region(300, 600, 600, 300))).unwrap();
        assert_area(
            offset.settings.area,
            Area {
                left: 25.4,
                top: 50.8,
                right: 76.2,
                bottom: 76.2,
            },
        );
        // Past the edge: clamped.
        let over = parse(&settings(&region(2400, 0, 600, 300))).unwrap();
        assert_area(
            over.settings.area,
            Area {
                left: 203.2,
                top: 0.0,
                right: 215.9,
                bottom: 25.4,
            },
        );
        // No size: the whole glass.
        let zero = parse(&settings(&region(0, 0, 0, 0))).unwrap();
        assert_area(zero.settings.area, a4());
        // Only the first region counts.
        let two = settings(&format!(
            "<pwg:ScanRegions>{}{}</pwg:ScanRegions>",
            one_region(300, 600, 600, 300),
            one_region(0, 0, 30, 30)
        ));
        assert_area(
            parse(&two).unwrap().settings.area,
            offset.settings.area.unwrap(),
        );
        // Off the glass entirely.
        assert!(matches!(
            parse(&settings(&region(9000, 0, 600, 300))),
            Err(SettingsError::Unsupported(_))
        ));
        // A scanner without geometry scans what it scans.
        let caps = ScannerCapabilities {
            area: None,
            ..caps()
        };
        let none = parse_settings(settings(&region(300, 600, 600, 300)).as_bytes(), &caps);
        assert_eq!(none.unwrap().settings.area, None);
    }

    #[test]
    fn sources_modes_and_formats_map_to_recto() {
        let source = |body: &str| {
            let parsed = parse(&settings(body)).unwrap();
            (parsed.settings.source.unwrap(), parsed.feeder)
        };
        assert_eq!(
            source("<pwg:InputSource>Feeder</pwg:InputSource>"),
            ("ADF".into(), true)
        );
        assert_eq!(
            source("<pwg:InputSource>Feeder</pwg:InputSource><scan:Duplex>1</scan:Duplex>"),
            ("ADF Duplex".into(), true)
        );
        assert_eq!(
            source("<scan:InputSource>Platen</scan:InputSource><scan:Duplex>true</scan:Duplex>"),
            ("Flatbed".into(), false)
        );

        let mode = |name: &str| {
            parse(&settings(&format!(
                "<scan:ColorMode>{name}</scan:ColorMode>"
            )))
            .map(|r| r.settings.mode)
        };
        assert_eq!(mode("RGB48"), Ok(ScanMode::Color));
        assert_eq!(mode("Grayscale16"), Ok(ScanMode::Gray));
        assert_eq!(mode("BlackAndWhite1"), Ok(ScanMode::Lineart));
        assert!(matches!(mode("CMYK32"), Err(SettingsError::Unsupported(_))));
        let photo = parse(&settings("<scan:Intent>Photo</scan:Intent>")).unwrap();
        assert_eq!(photo.settings.mode, ScanMode::Color);
        assert_eq!(photo.format, OutputFormat::Jpeg);
        assert_eq!(photo.intent, Some(Intent::Photo));

        let format = |body: &str| parse(&settings(body)).map(|r| r.format);
        let document = "<scan:Intent>Document</scan:Intent>";
        assert_eq!(format(document), Ok(OutputFormat::Pdf));
        assert_eq!(
            format(&format!(
                "{document}<pwg:DocumentFormat>application/octet-stream</pwg:DocumentFormat>"
            )),
            Ok(OutputFormat::Pdf)
        );
        assert_eq!(
            format(
                "<pwg:DocumentFormat>application/pdf</pwg:DocumentFormat><scan:DocumentFormatExt>image/png</scan:DocumentFormatExt>"
            ),
            Ok(OutputFormat::Png),
            "Ext wins"
        );
        assert!(matches!(
            format("<pwg:DocumentFormat>image/tiff</pwg:DocumentFormat>"),
            Err(SettingsError::Unsupported(_))
        ));
    }

    #[test]
    fn what_the_scanner_lacks_is_refused() {
        let flatbed = ScannerCapabilities {
            modes: vec![ScanMode::Color],
            sources: vec![Source::new("Flatbed")],
            ..caps()
        };
        let simplex = ScannerCapabilities {
            sources: vec![Source::new("ADF")],
            ..caps()
        };
        for (caps, body) in [
            (&flatbed, "<pwg:InputSource>Feeder</pwg:InputSource>"),
            (&flatbed, "<pwg:InputSource>Camera</pwg:InputSource>"),
            (&flatbed, "<scan:ColorMode>Grayscale8</scan:ColorMode>"),
            (&simplex, "<pwg:InputSource>Platen</pwg:InputSource>"),
            (
                &simplex,
                "<pwg:InputSource>Feeder</pwg:InputSource><scan:Duplex>true</scan:Duplex>",
            ),
            (
                &simplex,
                "<pwg:ScanRegions><pwg:ScanRegion><pwg:ContentRegionUnits>escl:Pixels</pwg:ContentRegionUnits></pwg:ScanRegion></pwg:ScanRegions>",
            ),
        ] {
            let result = parse_settings(settings(body).as_bytes(), caps);
            assert!(
                matches!(result, Err(SettingsError::Unsupported(_))),
                "{body}: {result:?}"
            );
        }
        // A feeder-only scanner scans from its feeder by default.
        let only = parse_settings(settings("").as_bytes(), &simplex).unwrap();
        assert_eq!(only.settings.source.as_deref(), Some("ADF"));
        assert!(only.feeder);
        // A scanner without a source option takes no source.
        let bare = ScannerCapabilities {
            sources: vec![],
            ..caps()
        };
        let platen = settings("<pwg:InputSource>Platen</pwg:InputSource>");
        assert_eq!(
            parse_settings(platen.as_bytes(), &bare)
                .unwrap()
                .settings
                .source,
            None
        );
    }

    #[test]
    fn pages_over_the_budget_are_refused() {
        let caps = ScannerCapabilities {
            resolutions: vec![4800],
            ..caps()
        };
        assert_eq!(
            parse_settings(settings("").as_bytes(), &caps),
            Err(SettingsError::TooLarge)
        );
    }

    #[test]
    fn prefixes_do_not_matter() {
        let xml = r#"<s:ScanSettings xmlns:s="http://schemas.hp.com/imaging/escl/2011/05/03" xmlns="http://www.pwg.org/schemas/2010/12/sm"><Version>2.0</Version><InputSource>Feeder</InputSource><s:XResolution>150</s:XResolution><DocumentFormat>image/png</DocumentFormat></s:ScanSettings>"#;
        let parsed = parse(xml).unwrap();
        assert_eq!(parsed.settings.source.as_deref(), Some("ADF"));
        assert_eq!(parsed.settings.resolution, 150);
        assert_eq!(parsed.format, OutputFormat::Png);
        // Elements of other namespaces are not eSCL's.
        let other = settings(
            r#"<x:XResolution xmlns:x="urn:other">75</x:XResolution><scan:XResolution>150</scan:XResolution>"#,
        );
        assert_eq!(parse(&other).unwrap().settings.resolution, 150);
        // References are resolved.
        let refs = settings("<scan:XResolution>&#49;5&#x30;</scan:XResolution>");
        assert_eq!(parse(&refs).unwrap().settings.resolution, 150);
    }

    #[test]
    fn hostile_xml_is_refused() {
        let malformed = |xml: &[u8]| {
            let result = parse_settings(xml, &caps());
            assert!(
                matches!(result, Err(SettingsError::Malformed(_))),
                "{}: {result:?}",
                String::from_utf8_lossy(&xml[..xml.len().min(200)])
            );
        };
        // Entities, external or expanding, never get a chance.
        malformed(
            br#"<?xml version="1.0"?><!DOCTYPE lol [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;&lol;">]><scan:ScanSettings xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03"><scan:Intent>&lol2;</scan:Intent></scan:ScanSettings>"#,
        );
        malformed(settings("<scan:Intent>&unknown;</scan:Intent>").as_bytes());
        // Over 64 KiB, even if well formed.
        let padding = format!("<!--{}-->", "x".repeat(MAX_SETTINGS_BYTES));
        malformed(settings(&padding).as_bytes());
        // Deeper than any eSCL document.
        let deep = format!(
            "{}{}",
            "<scan:Nested>".repeat(100),
            "</scan:Nested>".repeat(100)
        );
        malformed(settings(&deep).as_bytes());
        // Not eSCL, not XML, not finished, not UTF-8, not a number.
        malformed(br#"<ScanSettings><Version>2.0</Version></ScanSettings>"#);
        malformed(br#"<scan:ScanSettings xmlns:scan="urn:other"/>"#);
        malformed(b"");
        malformed(b"hello");
        malformed(settings("<scan:XResolution>150</scan:YResolution>").as_bytes());
        malformed(settings("").replace("</scan:ScanSettings>", "").as_bytes());
        malformed(&[b'<', 0xff, 0xfe, b'>']);
        malformed(settings("<scan:XResolution>-150</scan:XResolution>").as_bytes());
        malformed(settings("<scan:XResolution>lots</scan:XResolution>").as_bytes());
        malformed(settings("<scan:Duplex>maybe</scan:Duplex>").as_bytes());
        malformed(format!("{}<scan:Other/>", settings("")).as_bytes());
    }

    #[test]
    fn no_input_panics() {
        let caps = caps();
        let fixtures: Vec<Vec<u8>> = [
            "sane-airscan-flatbed.xml",
            "sane-airscan-adf-duplex.xml",
            "sane-backends-escl.xml",
            "macos-preview.xml",
        ]
        .iter()
        .map(|name| fixture(name))
        .collect();
        // Cut anywhere.
        for input in &fixtures {
            for end in 0..input.len() {
                let _ = parse_settings(&input[..end], &caps);
            }
        }
        // Cheap deterministic noise: every byte changed in turn.
        for input in &fixtures {
            for i in 0..input.len() {
                let mut noisy = input.clone();
                noisy[i] = noisy[i].wrapping_mul(31).wrapping_add(7);
                let _ = parse_settings(&noisy, &caps);
            }
        }
    }
}
