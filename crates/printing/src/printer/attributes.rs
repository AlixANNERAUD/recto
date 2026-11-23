//! What the printer says about itself and about its jobs.
//!
//! Values follow PWG 5100.14-2020 (IPP Everywhere), which is what driverless clients read, and
//! what CUPS's `ipptool` checks with `ipp-everywhere.test`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ipp::attribute::IppAttribute;
use ipp::value::{IppDateTime, IppName, IppValue};
use uuid::Uuid;

use super::Media;

/// The formats the queue takes today. `application/octet-stream` asks recto to detect the
/// format from the content, which it does for every document anyway.
pub const DOCUMENT_FORMATS: &[&str] = &[
    "application/pdf",
    "image/jpeg",
    "image/png",
    "application/octet-stream",
];

/// The Printer Description and Status attributes PWG 5100.14-2020 requires, all answered by
/// [`VirtualPrinter::printer_attributes`]. `media-col-database` only when asked for by name.
pub const REQUIRED_PRINTER_ATTRIBUTES: &[&str] = &[
    "charset-configured",
    "charset-supported",
    "color-supported",
    "compression-supported",
    "copies-default",
    "copies-supported",
    "document-format-default",
    "document-format-supported",
    "finishings-default",
    "finishings-supported",
    "generated-natural-language-supported",
    "identify-actions-default",
    "identify-actions-supported",
    "ipp-features-supported",
    "ipp-versions-supported",
    "job-creation-attributes-supported",
    "job-ids-supported",
    "media-bottom-margin-supported",
    "media-col-database",
    "media-col-default",
    "media-col-ready",
    "media-col-supported",
    "media-default",
    "media-left-margin-supported",
    "media-ready",
    "media-right-margin-supported",
    "media-size-supported",
    "media-source-supported",
    "media-supported",
    "media-top-margin-supported",
    "media-type-supported",
    "multiple-document-jobs-supported",
    "multiple-operation-time-out",
    "multiple-operation-time-out-action",
    "natural-language-configured",
    "operations-supported",
    "orientation-requested-default",
    "orientation-requested-supported",
    "output-bin-default",
    "output-bin-supported",
    "overrides-supported",
    "page-ranges-supported",
    "pages-per-minute",
    "pages-per-minute-color",
    "pdl-override-supported",
    "preferred-attributes-supported",
    "print-color-mode-default",
    "print-color-mode-supported",
    "print-content-optimize-default",
    "print-content-optimize-supported",
    "print-quality-default",
    "print-quality-supported",
    "print-rendering-intent-default",
    "print-rendering-intent-supported",
    "printer-config-change-date-time",
    "printer-config-change-time",
    "printer-current-time",
    "printer-device-id",
    "printer-geo-location",
    "printer-get-attributes-supported",
    "printer-icons",
    "printer-info",
    "printer-is-accepting-jobs",
    "printer-location",
    "printer-make-and-model",
    "printer-more-info",
    "printer-name",
    "printer-organization",
    "printer-organizational-unit",
    "printer-resolution-default",
    "printer-resolution-supported",
    "printer-state",
    "printer-state-change-date-time",
    "printer-state-change-time",
    "printer-state-message",
    "printer-state-reasons",
    "printer-up-time",
    "printer-uri-supported",
    "printer-uuid",
    "queued-job-count",
    "sides-default",
    "sides-supported",
    "uri-authentication-supported",
    "uri-security-supported",
    "which-jobs-supported",
];

/// The operations of PWG 5100.14 Table 4, in code order.
const OPERATIONS: [i32; 11] = [
    0x02, // Print-Job
    0x04, // Validate-Job
    0x05, // Create-Job
    0x06, // Send-Document
    0x08, // Cancel-Job
    0x09, // Get-Job-Attributes
    0x0A, // Get-Jobs
    0x0B, // Get-Printer-Attributes
    0x39, // Cancel-My-Jobs
    0x3B, // Close-Job
    0x3C, // Identify-Printer
];

/// Printable-area margins in hundredths of a millimetre: 1/6 inch, what most office printers
/// keep, so the layout a device makes survives any printer it is released to.
const MARGIN: i32 = 423;

/// The attributes of one job. Asked for by `requested-attributes` with these group names, every
/// job attribute is answered.
const JOB_GROUPS: [&str; 4] = ["all", "job-description", "job-status", "job-template"];

/// What a job creation answers with (RFC 8011 section 4.2.1.2).
const CREATED: [&str; 5] = [
    "job-id",
    "job-uri",
    "job-state",
    "job-state-reasons",
    "job-state-message",
];

/// The queue as devices see it: one generic printer, whose documents wait in recto until the
/// person releases them to a real one.
#[derive(Clone, Debug)]
pub struct VirtualPrinter {
    pub name: String,
    pub make_and_model: String,
    pub uuid: Uuid,
    /// The page where the person releases their jobs.
    pub more_info: String,
    /// Absolute URLs of PNG icons; none leaves `printer-icons` out.
    pub icons: Vec<String>,
    pub default_media: Media,
    /// MIME types, [`DOCUMENT_FORMATS`] today.
    pub formats: Vec<&'static str>,
    /// When recto started: the printer's configuration and state date from then.
    pub started_at: DateTime<Utc>,
}

/// The address a request came in on, echoed in every URI recto returns: clients switch to the
/// URI the printer reports, and on a personal address would otherwise lose the secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrinterUri {
    /// `ipps://host:port/ipp/print` or `ipps://host:port/device/<secret>/ipp/print`.
    pub uri: String,
    pub tls: bool,
    /// Whether this address asks for Basic credentials; a personal address carries its own.
    pub basic: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobViewState {
    /// A Create-Job still waiting for its document.
    Pending,
    /// Held in recto: the device's part is done.
    Completed,
}

/// A job as a device sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobView {
    pub id: i32,
    pub name: String,
    /// The device password that sent it, as `job-originating-user-name`.
    pub device: String,
    pub state: JobViewState,
    pub received_at: DateTime<Utc>,
    pub pages: Option<i64>,
}

impl VirtualPrinter {
    /// A stable `printer-uuid` for the public address, so devices keep recognising the queue
    /// across restarts.
    pub fn uuid_for(public_url: &str) -> Uuid {
        Uuid::new_v5(&Uuid::NAMESPACE_URL, public_url.as_bytes())
    }

    /// Whether a declared `document-format` is one the queue takes.
    pub fn accepts(&self, format: &str) -> bool {
        self.formats.contains(&format)
    }

    /// The printer's attributes, filtered by `requested-attributes`: `all`,
    /// `printer-description` and `job-template` (or no names) mean every attribute except
    /// `media-col-database`, which is sent only when named. `queued` is the number of jobs of
    /// the asking device still pending.
    pub fn printer_attributes(
        &self,
        uri: &PrinterUri,
        requested: &[String],
        queued: u32,
        now: DateTime<Utc>,
    ) -> Vec<IppAttribute> {
        let everything = requested.is_empty()
            || requested.iter().any(|name| {
                matches!(
                    name.as_str(),
                    "all" | "printer-description" | "job-template"
                )
            });
        let wanted = |name: &str| {
            requested.iter().any(|asked| asked == name)
                || (everything && name != "media-col-database")
        };

        let mut out = Attributes::default();
        let mut put = |name: &'static str, value: Option<IppValue>| {
            if let Some(value) = value
                && wanted(name)
            {
                out.push(name, value);
            }
        };

        put("charset-configured", charset("utf-8"));
        put("charset-supported", set(["utf-8", "us-ascii"].map(charset)));
        put("color-supported", Some(IppValue::Boolean(true)));
        put("compression-supported", keyword("none"));
        put("copies-default", Some(IppValue::Integer(1)));
        put(
            "copies-supported",
            Some(IppValue::RangeOfInteger { min: 1, max: 99 }),
        );
        put("document-format-default", mime("application/octet-stream"));
        put(
            "document-format-supported",
            set(self.formats.iter().map(|f| mime(f))),
        );
        put("finishings-default", Some(IppValue::Enum(3)));
        put("finishings-supported", Some(IppValue::Enum(3)));
        put("generated-natural-language-supported", language("en"));
        put("identify-actions-default", keyword("display"));
        put("identify-actions-supported", keyword("display"));
        put("ipp-features-supported", keyword("ipp-everywhere"));
        put("ipp-versions-supported", keywords(&["1.1", "2.0"]));
        put(
            "job-creation-attributes-supported",
            keywords(&[
                "copies",
                "ipp-attribute-fidelity",
                "job-name",
                "media",
                "media-col",
                "orientation-requested",
                "page-ranges",
                "print-color-mode",
                "print-quality",
                "printer-resolution",
                "sides",
            ]),
        );
        put("job-ids-supported", Some(IppValue::Boolean(true)));
        put(
            "media-bottom-margin-supported",
            Some(IppValue::Integer(MARGIN)),
        );
        put(
            "media-col-database",
            set(Media::ALL.map(|m| Some(media_col(m)))),
        );
        put("media-col-default", Some(media_col(self.default_media)));
        put(
            "media-col-ready",
            set(Media::ALL.map(|m| Some(media_col(m)))),
        );
        put(
            "media-col-supported",
            keywords(&[
                "media-bottom-margin",
                "media-left-margin",
                "media-right-margin",
                "media-size",
                "media-source",
                "media-top-margin",
                "media-type",
            ]),
        );
        put("media-default", keyword(self.default_media.keyword()));
        put(
            "media-left-margin-supported",
            Some(IppValue::Integer(MARGIN)),
        );
        put("media-ready", set(Media::ALL.map(|m| keyword(m.keyword()))));
        put(
            "media-right-margin-supported",
            Some(IppValue::Integer(MARGIN)),
        );
        put(
            "media-size-supported",
            set(Media::ALL.map(|m| Some(media_size(m)))),
        );
        put("media-source-supported", keyword("auto"));
        put(
            "media-supported",
            set(Media::ALL.map(|m| keyword(m.keyword()))),
        );
        put(
            "media-top-margin-supported",
            Some(IppValue::Integer(MARGIN)),
        );
        put("media-type-supported", keyword("stationery"));
        put(
            "multiple-document-jobs-supported",
            Some(IppValue::Boolean(false)),
        );
        // The held-jobs sweep drops a Create-Job still without its document after 10 minutes.
        put("multiple-operation-time-out", Some(IppValue::Integer(600)));
        put("multiple-operation-time-out-action", keyword("abort-job"));
        put("natural-language-configured", language("en"));
        put(
            "operations-supported",
            set(OPERATIONS.map(|op| Some(IppValue::Enum(op)))),
        );
        put("orientation-requested-default", Some(IppValue::Enum(3)));
        put(
            "orientation-requested-supported",
            set([3, 4, 5, 6].map(|o| Some(IppValue::Enum(o)))),
        );
        put("output-bin-default", keyword("face-down"));
        put("output-bin-supported", keyword("face-down"));
        put(
            "overrides-supported",
            keywords(&["document-numbers", "pages"]),
        );
        put("page-ranges-supported", Some(IppValue::Boolean(true)));
        put("pages-per-minute", Some(IppValue::Integer(1)));
        put("pages-per-minute-color", Some(IppValue::Integer(1)));
        put("pdl-override-supported", keyword("attempted"));
        put(
            "preferred-attributes-supported",
            Some(IppValue::Boolean(false)),
        );
        put("print-color-mode-default", keyword("auto"));
        put(
            "print-color-mode-supported",
            keywords(&["auto", "color", "monochrome"]),
        );
        put("print-content-optimize-default", keyword("auto"));
        put("print-content-optimize-supported", keyword("auto"));
        put("print-quality-default", Some(IppValue::Enum(4)));
        put("print-quality-supported", Some(IppValue::Enum(4)));
        put("print-rendering-intent-default", keyword("auto"));
        put("print-rendering-intent-supported", keyword("auto"));
        put(
            "printer-config-change-date-time",
            Some(date_time(self.started_at)),
        );
        put("printer-config-change-time", Some(seconds(self.started_at)));
        put("printer-current-time", Some(date_time(now)));
        put("printer-device-id", text(&self.device_id()));
        put("printer-geo-location", Some(unknown()));
        put(
            "printer-get-attributes-supported",
            keyword("document-format"),
        );
        put(
            "printer-icons",
            set(self.icons.iter().map(|icon| uri_value(icon))),
        );
        put("printer-info", text(&self.name));
        put("printer-is-accepting-jobs", Some(IppValue::Boolean(true)));
        put("printer-location", text(""));
        put(
            "printer-make-and-model",
            text(&truncate(&self.make_and_model, 127)),
        );
        put("printer-more-info", uri_value(&self.more_info));
        put("printer-name", name(&truncate(&self.name, 127)));
        put("printer-organization", text(""));
        put("printer-organizational-unit", text(""));
        put("printer-resolution-default", Some(dpi(300)));
        put("printer-resolution-supported", Some(dpi(300)));
        put("printer-state", Some(IppValue::Enum(3)));
        put(
            "printer-state-change-date-time",
            Some(date_time(self.started_at)),
        );
        put("printer-state-change-time", Some(seconds(self.started_at)));
        put("printer-state-message", text("Ready"));
        put("printer-state-reasons", keyword("none"));
        // Unix time, as CUPS does: every `*-time` attribute shares its clock.
        put("printer-up-time", Some(seconds(now)));
        put("printer-uri-supported", uri_value(&uri.uri));
        put(
            "printer-uuid",
            uri_value(&format!("urn:uuid:{}", self.uuid)),
        );
        put("queued-job-count", Some(IppValue::Integer(clamp(queued))));
        put("sides-default", keyword("one-sided"));
        put(
            "sides-supported",
            keywords(&["one-sided", "two-sided-long-edge", "two-sided-short-edge"]),
        );
        // Android's default print service skips an address asking for anything but `none`.
        put(
            "uri-authentication-supported",
            keyword(if uri.basic { "basic" } else { "none" }),
        );
        put(
            "uri-security-supported",
            keyword(if uri.tls { "tls" } else { "none" }),
        );
        put(
            "which-jobs-supported",
            keywords(&["completed", "not-completed", "all"]),
        );
        out.0
    }

    /// A job's attributes, filtered by `requested-attributes`; `message` is its
    /// `job-state-message`. URIs are built on `uri`, the address the request came in on.
    pub fn job_attributes(
        &self,
        job: &JobView,
        uri: &PrinterUri,
        message: &str,
        requested: &[String],
        now: DateTime<Utc>,
    ) -> Vec<IppAttribute> {
        let everything = requested.is_empty()
            || requested
                .iter()
                .any(|name| JOB_GROUPS.contains(&name.as_str()));
        let wanted = |name: &str| everything || requested.iter().any(|asked| asked == name);
        let completed = job.state == JobViewState::Completed;
        let when_completed = |value: IppValue| if completed { value } else { IppValue::NoValue };

        let mut out = Attributes::default();
        let mut put = |name: &'static str, value: Option<IppValue>| {
            if let Some(value) = value
                && wanted(name)
            {
                out.push(name, value);
            }
        };

        put("job-id", Some(IppValue::Integer(job.id)));
        put("job-uri", uri_value(&format!("{}/{}", uri.uri, job.id)));
        put("job-printer-uri", uri_value(&uri.uri));
        put(
            "job-uuid",
            uri_value(&format!("urn:uuid:{}", self.job_uuid(job.id))),
        );
        put("job-name", name(&job.name));
        put("job-originating-user-name", name(&job.device));
        let (state, reason) = if completed {
            (9, "job-completed-successfully")
        } else {
            (3, "job-incoming")
        };
        put("job-state", Some(IppValue::Enum(state)));
        put("job-state-reasons", keyword(reason));
        put("job-state-message", text(message));
        put("job-printer-up-time", Some(seconds(now)));
        put("time-at-creation", Some(seconds(job.received_at)));
        put(
            "time-at-processing",
            Some(when_completed(seconds(job.received_at))),
        );
        put(
            "time-at-completed",
            Some(when_completed(seconds(job.received_at))),
        );
        put("date-time-at-creation", Some(date_time(job.received_at)));
        put(
            "date-time-at-processing",
            Some(when_completed(date_time(job.received_at))),
        );
        put(
            "date-time-at-completed",
            Some(when_completed(date_time(job.received_at))),
        );
        put(
            "job-impressions",
            Some(
                job.pages
                    .and_then(|pages| i32::try_from(pages).ok())
                    .map_or(IppValue::NoValue, IppValue::Integer),
            ),
        );
        // Nothing has printed yet: held jobs print only once released.
        put("job-impressions-completed", Some(IppValue::Integer(0)));
        put(
            "number-of-documents",
            Some(IppValue::Integer(i32::from(completed))),
        );
        out.0
    }

    /// The job group of a Print-Job, Create-Job or Send-Document answer.
    pub fn created_job_attributes(
        &self,
        job: &JobView,
        uri: &PrinterUri,
        message: &str,
        now: DateTime<Utc>,
    ) -> Vec<IppAttribute> {
        let requested: Vec<String> = CREATED.iter().map(|name| (*name).to_owned()).collect();
        self.job_attributes(job, uri, message, &requested, now)
    }

    fn device_id(&self) -> String {
        let commands: Vec<&str> = self
            .formats
            .iter()
            .filter_map(|format| match *format {
                "application/pdf" => Some("PDF"),
                "image/pwg-raster" => Some("PWG"),
                "image/urf" => Some("URF"),
                "image/jpeg" => Some("JPEG"),
                "image/png" => Some("PNG"),
                _ => None,
            })
            .collect();
        // IEEE 1284 keys end with `;`, so the model must not carry one.
        let model = self.make_and_model.replace([';', ':'], " ");
        format!("MFG:recto;MDL:{model};CMD:{};", commands.join(","))
    }

    /// Stable per job and per printer, as 5100.14 asks; job ids are never reused.
    fn job_uuid(&self, id: i32) -> Uuid {
        Uuid::new_v5(&self.uuid, &id.to_be_bytes())
    }
}

/// Attributes in the order they are put, so answers read like the table above.
#[derive(Default)]
struct Attributes(Vec<IppAttribute>);

impl Attributes {
    fn push(&mut self, name: &'static str, value: IppValue) {
        self.0.push(IppAttribute::new(
            IppName::new(name).expect("attribute names are short"),
            value,
        ));
    }
}

/// A set of values, one bare value when there is one, and none at all when empty: an empty
/// `IppValue::Array` encodes without a value length and desyncs the client's parser.
fn set(values: impl IntoIterator<Item = Option<IppValue>>) -> Option<IppValue> {
    let mut values: Vec<IppValue> = values.into_iter().flatten().collect();
    match values.len() {
        0 => None,
        1 => values.pop(),
        _ => Some(IppValue::Array(values)),
    }
}

// Values below that come from the operator's settings (names, URLs) can be too long for IPP;
// such an attribute is left out rather than sent truncated or failing the whole answer.

fn keyword(value: &str) -> Option<IppValue> {
    IppValue::new_keyword(value).ok()
}

fn keywords(values: &[&str]) -> Option<IppValue> {
    set(values.iter().map(|value| keyword(value)))
}

fn charset(value: &str) -> Option<IppValue> {
    IppValue::new_charset(value).ok()
}

fn language(value: &str) -> Option<IppValue> {
    IppValue::new_natural_language(value).ok()
}

fn mime(value: &str) -> Option<IppValue> {
    IppValue::new_mime_media_type(value).ok()
}

fn text(value: &str) -> Option<IppValue> {
    IppValue::new_text_without_language(value).ok()
}

fn name(value: &str) -> Option<IppValue> {
    IppValue::new_name_without_language(value).ok()
}

fn uri_value(value: &str) -> Option<IppValue> {
    IppValue::new_uri(value).ok()
}

fn unknown() -> IppValue {
    IppValue::Other {
        tag: 0x12,
        data: bytes::Bytes::new(),
    }
}

fn dpi(resolution: i32) -> IppValue {
    IppValue::Resolution {
        cross_feed: resolution,
        feed: resolution,
        units: 3,
    }
}

fn date_time(at: DateTime<Utc>) -> IppValue {
    IppValue::DateTime(IppDateTime::from(at))
}

fn seconds(at: DateTime<Utc>) -> IppValue {
    IppValue::Integer(i32::try_from(at.timestamp()).unwrap_or(i32::MAX))
}

fn clamp(count: u32) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// At most `limit` bytes, cut on a character boundary.
fn truncate(value: &str, limit: usize) -> String {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn member(name: &'static str, value: IppValue) -> (IppName, IppValue) {
    (IppName::new(name).expect("member names are short"), value)
}

fn media_size(media: Media) -> IppValue {
    let (x, y) = media.size();
    IppValue::Collection(BTreeMap::from([
        member("x-dimension", IppValue::Integer(x)),
        member("y-dimension", IppValue::Integer(y)),
    ]))
}

fn media_col(media: Media) -> IppValue {
    let keyword = |value: &str| IppValue::new_keyword(value).expect("short keyword");
    IppValue::Collection(BTreeMap::from([
        member("media-bottom-margin", IppValue::Integer(MARGIN)),
        member("media-left-margin", IppValue::Integer(MARGIN)),
        member("media-right-margin", IppValue::Integer(MARGIN)),
        member("media-size", media_size(media)),
        member("media-size-name", keyword(media.keyword())),
        member("media-source", keyword("auto")),
        member("media-top-margin", IppValue::Integer(MARGIN)),
        member("media-type", keyword("stationery")),
    ]))
}
