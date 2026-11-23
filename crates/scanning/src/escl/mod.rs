//! eSCL, the scan protocol of AirScan and Mopria: the XML recto writes for scan apps, the
//! `ScanSettings` it reads from them, and the jobs between a request and its documents.
//!
//! Everything here is pure; the HTTP routes, authentication and DNS-SD live in the server.
//!
//! sane-airscan parses capabilities and status by the literal names `scan:...` and `pwg:...`,
//! without resolving namespaces, so recto always writes these two prefixes. What it reads is
//! resolved by namespace, since other clients may choose other prefixes.

use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::{
    Area, MAX_PAGE_BYTES, ScanMode, ScanSettings, ScannerCapabilities, ScannerInfo, Source,
};

mod jobs;
mod settings;

pub use jobs::{Document, EsclJobs, NextDocument};
pub use settings::{EsclRequest, Intent, MAX_SETTINGS_BYTES, SettingsError, parse_settings};

/// The start of recto's eSCL `MakeAndModel` and DNS-SD `ty`. SANE lists a scanner sane-airscan
/// discovered under this model with `model = hardware`, and recto's sane-airscan blacklist matches
/// `recto*`, so recto never offers its own scanners back to itself.
pub const MODEL: &str = "recto";

/// The eSCL version recto speaks, in `pwg:Version` and the DNS-SD `vers`.
pub const VERSION: &str = "2.63";

const PWG: &str = "http://www.pwg.org/schemas/2010/12/sm";
const SCAN: &str = "http://schemas.hp.com/imaging/escl/2011/05/03";

/// eSCL sizes are in 1/300 inch, whatever the resolution.
const UNITS_PER_INCH: f64 = 300.0;
const MM_PER_INCH: f64 = 25.4;

/// Letter width and A4 height, as HP devices say, for scanners whose geometry is unknown.
const DEFAULT_WIDTH: u32 = 2550;
const DEFAULT_HEIGHT: u32 = 3508;
/// The smallest region, about 1.4 mm.
const MIN_SIZE: u32 = 16;

/// The scanner's name in eSCL paths and DNS-SD: the first 8 hex digits of the SHA-256 of its SANE
/// id, stable and short, and without the id itself, which can name a host or a USB bus.
pub fn scanner_key(id: &str) -> String {
    Sha256::digest(id.as_bytes())
        .iter()
        .take(4)
        .fold(String::with_capacity(8), |mut key, byte| {
            let _ = write!(key, "{byte:02x}");
            key
        })
}

/// The UUID a scanner is advertised under: the same for one recto and one SANE id across
/// restarts, so clients keep recognising it.
pub fn scanner_uuid(public_url: &str, id: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("{public_url}#scanner/{id}").as_bytes(),
    )
}

/// Whether a SANE device is one of recto's own eSCL scanners found again through the network:
/// its model starts with [`MODEL`], or is one of the DNS-SD instance names recto advertises,
/// which sane-airscan uses as the model by default.
pub fn is_own_scanner(model: &str, advertised: &[String]) -> bool {
    model.starts_with(MODEL) || advertised.iter().any(|name| name == model)
}

/// The path of a job under `root`, the scanner's eSCL root the request came in on: a personal
/// address keeps its secret, a Basic one never gains it.
pub fn job_uri(root: &str, id: Uuid) -> String {
    format!("{}/ScanJobs/{id}", root.trim_end_matches('/'))
}

/// The scanner as a whole, in `ScannerStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EsclState {
    Idle,
    /// An eSCL job of this scanner is reading pages.
    Processing,
}

/// What the feeder reports. recto cannot sense paper, so it says loaded until a job finds
/// otherwise; sane-airscan reads this after a failed `NextDocument`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdfState {
    Loaded,
    Empty,
    Jam,
    DoorOpen,
}

impl AdfState {
    fn name(self) -> &'static str {
        match self {
            Self::Loaded => "ScannerAdfLoaded",
            Self::Empty => "ScannerAdfEmpty",
            Self::Jam => "ScannerAdfJam",
            // sane-airscan knows this spelling; go-mfp's HatchOpen is the same state.
            Self::DoorOpen => "ScannerAdfDoorOpen",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EsclJobState {
    /// Scanning, or documents still waiting to be fetched.
    Processing,
    Completed,
    Canceled,
    Aborted,
}

impl EsclJobState {
    fn name(self) -> &'static str {
        match self {
            Self::Processing => "Processing",
            Self::Completed => "Completed",
            Self::Canceled => "Canceled",
            Self::Aborted => "Aborted",
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::Processing => "JobScanning",
            Self::Completed => "JobCompletedSuccessfully",
            Self::Canceled => "JobCanceledByUser",
            Self::Aborted => "AbortedBySystem",
        }
    }
}

/// One job in `ScannerStatus`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobStatusView {
    pub id: Uuid,
    pub state: EsclJobState,
    /// Seconds since the job last changed.
    pub age_seconds: u64,
    /// Documents fetched.
    pub images_completed: u32,
    /// Documents ready and not fetched yet.
    pub images_to_transfer: u32,
}

/// The size of a document, for `ScanImageInfo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    pub gray: bool,
}

impl ImageInfo {
    /// The raw bytes of one line: one sample per pixel in gray, three in colour.
    pub fn bytes_per_line(&self) -> u64 {
        u64::from(self.width) * if self.gray { 1 } else { 3 }
    }
}

/// The XML declaration and the root's opening tag with recto's two prefixes.
fn open(root: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<{root} xmlns:pwg=\"{PWG}\" xmlns:scan=\"{SCAN}\">\n"
    )
}

/// Text for an XML document: escaped, and without the characters XML 1.0 cannot carry even
/// escaped (SANE strings come from devices and network backends).
fn text(value: &str) -> String {
    let allowed = |c: &char| {
        matches!(c, '\t' | '\n' | '\r')
            || (!c.is_control() && !matches!(*c, '\u{FFFE}' | '\u{FFFF}'))
    };
    quick_xml::escape::escape(value.chars().filter(allowed).collect::<String>()).into_owned()
}

/// Appends `<tag>value</tag>` on its own line at `depth`.
fn element(out: &mut String, depth: usize, tag: &str, value: impl std::fmt::Display) {
    let _ = writeln!(
        out,
        "{:indent$}<{tag}>{value}</{tag}>",
        "",
        indent = depth * 2
    );
}

fn line(out: &mut String, depth: usize, tag: &str) {
    let _ = writeln!(out, "{:indent$}{tag}", "", indent = depth * 2);
}

/// The SANE sources an eSCL input source stands for. A scanner without a source option is a
/// platen with whatever it scans from.
struct Sources<'a> {
    platen: Option<Option<&'a Source>>,
    simplex: Option<&'a Source>,
    duplex: Option<&'a Source>,
}

fn is_duplex(source: &Source) -> bool {
    source.is_feeder && source.name.to_ascii_lowercase().contains("duplex")
}

impl<'a> Sources<'a> {
    fn of(caps: &'a ScannerCapabilities) -> Self {
        if caps.sources.is_empty() {
            return Self {
                platen: Some(None),
                simplex: None,
                duplex: None,
            };
        }
        let duplex = caps.sources.iter().find(|s| is_duplex(s));
        Self {
            platen: caps.sources.iter().find(|s| !s.is_feeder).map(Some),
            // A feeder that only has a duplex source scans one side when asked to.
            simplex: caps
                .sources
                .iter()
                .find(|s| s.is_feeder && !is_duplex(s))
                .or(duplex),
            duplex,
        }
    }
}

/// What a scanner offers over eSCL, as its DNS-SD record announces it (`cs`, `is`, `duplex`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offered {
    pub color: bool,
    pub grayscale: bool,
    pub binary: bool,
    pub platen: bool,
    pub adf: bool,
    pub duplex: bool,
}

/// The colour modes and sources [`capabilities_xml`] describes for a scanner.
pub fn offered(caps: &ScannerCapabilities) -> Offered {
    let modes = modes(caps);
    let sources = Sources::of(caps);
    Offered {
        color: modes.contains(&ScanMode::Color),
        grayscale: modes.contains(&ScanMode::Gray),
        binary: modes.contains(&ScanMode::Lineart),
        platen: sources.platen.is_some(),
        adf: sources.simplex.is_some(),
        duplex: sources.duplex.is_some(),
    }
}

/// The resolutions offered over eSCL: those at which a full colour page fits the page budget,
/// since an eSCL profile cannot hold one list per mode. 300 dpi for a scanner that does not say,
/// whose resolution SANE then leaves alone; the lowest one if none fits, refused when scanned.
fn resolutions(caps: &ScannerCapabilities) -> Vec<u32> {
    let mut all = caps.resolutions.clone();
    all.sort_unstable();
    all.dedup();
    if all.is_empty() {
        return vec![300];
    }
    let area = full_area(caps);
    let fits = all
        .iter()
        .copied()
        .filter(|&resolution| {
            let page = ScanSettings {
                resolution,
                mode: ScanMode::Color,
                source: None,
                area: Some(area),
            };
            page.page_bytes()
                .is_some_and(|bytes| bytes <= MAX_PAGE_BYTES)
        })
        .collect::<Vec<_>>();
    if fits.is_empty() {
        all.truncate(1);
        all
    } else {
        fits
    }
}

/// The modes offered over eSCL, colour for a scanner without a mode option.
fn modes(caps: &ScannerCapabilities) -> Vec<ScanMode> {
    if caps.modes.is_empty() {
        vec![ScanMode::Color]
    } else {
        caps.modes.clone()
    }
}

fn color_mode(mode: ScanMode) -> &'static str {
    match mode {
        ScanMode::Color => "RGB24",
        ScanMode::Gray => "Grayscale8",
        ScanMode::Lineart => "BlackAndWhite1",
    }
}

/// The scanner's area, or the default one for a scanner without geometry in millimetres.
fn full_area(caps: &ScannerCapabilities) -> Area {
    caps.area.unwrap_or(Area {
        left: 0.0,
        top: 0.0,
        right: units_to_mm(DEFAULT_WIDTH),
        bottom: units_to_mm(DEFAULT_HEIGHT),
    })
}

fn units_to_mm(units: u32) -> f64 {
    f64::from(units) * MM_PER_INCH / UNITS_PER_INCH
}

fn mm_to_units(mm: f64) -> u32 {
    // Scanner areas are a few hundred millimetres; the cast cannot overflow.
    (mm.max(0.0) / MM_PER_INCH * UNITS_PER_INCH).round() as u32
}

const FORMATS: [&str; 3] = ["application/pdf", "image/jpeg", "image/png"];

/// One `PlatenInputCaps`, `AdfSimplexInputCaps` or `AdfDuplexInputCaps`: every source gets the
/// same profile, since SANE describes the device rather than each source.
fn input_caps(out: &mut String, tag: &str, caps: &ScannerCapabilities, platen: bool) {
    let area = full_area(caps);
    let resolutions = resolutions(caps);
    line(out, 2, &format!("<scan:{tag}>"));
    element(out, 3, "scan:MinWidth", MIN_SIZE);
    element(out, 3, "scan:MaxWidth", mm_to_units(area.width()));
    element(out, 3, "scan:MinHeight", MIN_SIZE);
    element(out, 3, "scan:MaxHeight", mm_to_units(area.height()));
    element(out, 3, "scan:MaxScanRegions", 1);
    line(out, 3, "<scan:SettingProfiles>");
    line(out, 4, "<scan:SettingProfile>");
    line(out, 5, "<scan:ColorModes>");
    for mode in modes(caps) {
        element(out, 6, "scan:ColorMode", color_mode(mode));
    }
    line(out, 5, "</scan:ColorModes>");
    // Both spellings, as HP devices write them: older clients read the first, newer the second.
    line(out, 5, "<scan:DocumentFormats>");
    for format in FORMATS {
        element(out, 6, "pwg:DocumentFormat", format);
    }
    for format in FORMATS {
        element(out, 6, "scan:DocumentFormatExt", format);
    }
    line(out, 5, "</scan:DocumentFormats>");
    // Discrete values only: sane-airscan never reads the Y range of a ResolutionRange.
    line(out, 5, "<scan:SupportedResolutions>");
    line(out, 6, "<scan:DiscreteResolutions>");
    for resolution in &resolutions {
        line(
            out,
            7,
            &format!(
                "<scan:DiscreteResolution><scan:XResolution>{resolution}</scan:XResolution><scan:YResolution>{resolution}</scan:YResolution></scan:DiscreteResolution>"
            ),
        );
    }
    line(out, 6, "</scan:DiscreteResolutions>");
    line(out, 5, "</scan:SupportedResolutions>");
    line(out, 5, "<scan:ColorSpaces>");
    element(out, 6, "scan:ColorSpace", "sRGB");
    line(out, 5, "</scan:ColorSpaces>");
    line(out, 4, "</scan:SettingProfile>");
    line(out, 3, "</scan:SettingProfiles>");
    line(out, 3, "<scan:SupportedIntents>");
    for intent in ["Document", "TextAndGraphic", "Photo"] {
        element(out, 4, "scan:Intent", intent);
    }
    // macOS previews as soon as the scanner is chosen; from a feeder that would pull a sheet.
    if platen {
        element(out, 4, "scan:Intent", "Preview");
    }
    line(out, 3, "</scan:SupportedIntents>");
    let max = resolutions.iter().copied().max().unwrap_or(300);
    element(out, 3, "scan:MaxOpticalXResolution", max);
    element(out, 3, "scan:MaxOpticalYResolution", max);
    line(out, 2, &format!("</scan:{tag}>"));
}

/// `ScannerCapabilities` for a scanner: its SANE description in eSCL terms, with `uuid` from
/// [`scanner_uuid`], `icon` a PNG clients can fetch without credentials, and `admin` the page
/// where people scan from recto itself.
pub fn capabilities_xml(
    info: &ScannerInfo,
    caps: &ScannerCapabilities,
    uuid: Uuid,
    icon: Option<&str>,
    admin: &str,
) -> String {
    let mut out = open("scan:ScannerCapabilities");
    element(&mut out, 1, "pwg:Version", VERSION);
    // Never the bare device name: sane-airscan matches some vendor prefixes against quirk lists.
    let model = [MODEL, info.vendor.trim(), info.model.trim()]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    element(&mut out, 1, "pwg:MakeAndModel", text(&model));
    element(&mut out, 1, "pwg:SerialNumber", scanner_key(&info.id));
    element(&mut out, 1, "scan:UUID", uuid);
    element(&mut out, 1, "scan:AdminURI", text(admin));
    if let Some(icon) = icon {
        element(&mut out, 1, "scan:IconURI", text(icon));
    }
    let sources = Sources::of(caps);
    if sources.platen.is_some() {
        line(&mut out, 1, "<scan:Platen>");
        input_caps(&mut out, "PlatenInputCaps", caps, true);
        line(&mut out, 1, "</scan:Platen>");
    }
    if sources.simplex.is_some() {
        line(&mut out, 1, "<scan:Adf>");
        input_caps(&mut out, "AdfSimplexInputCaps", caps, false);
        if sources.duplex.is_some() {
            input_caps(&mut out, "AdfDuplexInputCaps", caps, false);
            line(&mut out, 2, "<scan:AdfOptions>");
            element(&mut out, 3, "scan:AdfOption", "Duplex");
            line(&mut out, 2, "</scan:AdfOptions>");
        }
        line(&mut out, 1, "</scan:Adf>");
    }
    out.push_str("</scan:ScannerCapabilities>\n");
    out
}

/// `ScannerStatus`: the scanner's state, its feeder's for a scanner with one, and `jobs`, which
/// must be the caller's only, newest first. Their URIs are built on `root`, the caller's own.
pub fn status_xml(
    state: EsclState,
    adf: Option<AdfState>,
    root: &str,
    jobs: &[JobStatusView],
) -> String {
    let mut out = open("scan:ScannerStatus");
    element(&mut out, 1, "pwg:Version", VERSION);
    let state = match state {
        EsclState::Idle => "Idle",
        EsclState::Processing => "Processing",
    };
    element(&mut out, 1, "pwg:State", state);
    if let Some(adf) = adf {
        element(&mut out, 1, "scan:AdfState", adf.name());
    }
    if !jobs.is_empty() {
        line(&mut out, 1, "<scan:Jobs>");
        for job in jobs {
            line(&mut out, 2, "<scan:JobInfo>");
            element(&mut out, 3, "pwg:JobUri", text(&job_uri(root, job.id)));
            element(&mut out, 3, "pwg:JobUuid", job.id);
            element(&mut out, 3, "scan:Age", job.age_seconds);
            element(&mut out, 3, "pwg:ImagesCompleted", job.images_completed);
            element(&mut out, 3, "pwg:ImagesToTransfer", job.images_to_transfer);
            element(&mut out, 3, "pwg:JobState", job.state.name());
            line(&mut out, 3, "<pwg:JobStateReasons>");
            element(&mut out, 4, "pwg:JobStateReason", job.state.reason());
            line(&mut out, 3, "</pwg:JobStateReasons>");
            line(&mut out, 2, "</scan:JobInfo>");
        }
        line(&mut out, 1, "</scan:Jobs>");
    }
    out.push_str("</scan:ScannerStatus>\n");
    out
}

/// `ScanImageInfo` for the last document a job handed out.
pub fn scan_image_info_xml(root: &str, id: Uuid, info: &ImageInfo) -> String {
    let mut out = open("scan:ScanImageInfo");
    element(&mut out, 1, "pwg:JobUri", text(&job_uri(root, id)));
    element(&mut out, 1, "pwg:JobUuid", id);
    element(&mut out, 1, "scan:ActualWidth", info.width);
    element(&mut out, 1, "scan:ActualHeight", info.height);
    element(
        &mut out,
        1,
        "scan:ActualBytesPerLine",
        info.bytes_per_line(),
    );
    out.push_str("</scan:ScanImageInfo>\n");
    out
}

#[cfg(test)]
mod tests;
