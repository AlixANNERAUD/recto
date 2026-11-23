//! The real print service: IPP requests to a CUPS server.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::io::Cursor;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body::{Frame, SizeHint};

use ipp::attribute::{IppAttribute, IppAttributeGroup};
use ipp::model::{DelimiterTag, IppVersion, Operation, StatusCode};
use ipp::operation::builder::IppOperationBuilder;
use ipp::parser::IppParser;
use ipp::payload::IppPayload;
use ipp::prelude::Uri;
use ipp::reader::IppReader;
use ipp::request::IppRequestResponse;
use ipp::value::IppValue;
use percent_encoding::percent_decode_str;
use url::Url;

use crate::{
    Capabilities, ColorMode, Job, JobState, JobStatus, PrintError, PrintRequest, PrintService,
    Printer, PrinterState, Sides, StateReason, supplies,
};

/// Long enough for a busy CUPS, short enough that a page never hangs on a dead one.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A print request carries the document: up to the upload limit (50 MiB by default) to a CUPS
/// that may sit across a slow Wi-Fi link. Reporting a slow upload as "unavailable" could also
/// hide a job CUPS went on to print. Connecting still has [`REQUEST_TIMEOUT`].
const SUBMIT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How many finished jobs the job list shows next to the unfinished ones.
const COMPLETED_JOBS: i32 = 20;

const PRINTER_ATTRIBUTES: [&str; 18] = [
    "printer-name",
    "printer-info",
    "printer-location",
    "printer-make-and-model",
    "printer-state",
    "printer-state-reasons",
    "printer-is-accepting-jobs",
    "marker-names",
    "marker-levels",
    "marker-colors",
    "marker-types",
    "marker-high-levels",
    "marker-low-levels",
    "copies-supported",
    "sides-supported",
    "print-color-mode-supported",
    "printer-type",
    "printer-uri-supported",
];

const JOB_ATTRIBUTES: [&str; 8] = [
    "job-id",
    "job-printer-uri",
    "job-name",
    "job-originating-user-name",
    "job-state",
    "job-media-sheets-completed",
    "job-impressions-completed",
    "time-at-creation",
];

/// Talks to CUPS over IPP. `base` is the HTTP(S) address of the CUPS server.
pub struct Cups {
    base: Url,
    client: reqwest::Client,
}

impl Cups {
    pub fn new(base: Url) -> Self {
        Self::with_timeout(base, REQUEST_TIMEOUT)
    }

    /// `timeout` bounds connecting and every request but a print request, which gets five
    /// minutes.
    pub fn with_timeout(base: Url, timeout: Duration) -> Self {
        // The workspace reqwest (rustls with ring) does the HTTP instead of the client bundled
        // with `ipp`, which would pull in a second TLS stack built on aws-lc.
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .build()
            .expect("the TLS backend initializes");
        Cups { base, client }
    }

    /// `{base}/`, where CUPS answers the operations that are not about one printer.
    fn root_uri(&self) -> Result<Uri, PrintError> {
        let mut url = self.base.clone();
        url.set_path("/");
        url.set_query(None);
        to_uri(&url)
    }

    /// `{base}/printers/{name}` with the name percent-encoded as one path segment.
    fn printer_uri(&self, name: &str) -> Result<Uri, PrintError> {
        let mut url = self.base.clone();
        url.set_query(None);
        url.path_segments_mut()
            .map_err(|()| PrintError::Unavailable("the CUPS address cannot hold a path".into()))?
            .pop_if_empty()
            .push("printers")
            .push(name);
        to_uri(&url)
    }

    /// `ipp://host:port/jobs/{id}`, the address CUPS gives every job.
    fn job_uri(&self, job_id: i32) -> Result<IppValue, PrintError> {
        let host = self
            .base
            .host_str()
            .ok_or_else(|| PrintError::Unavailable("the CUPS address has no host".into()))?;
        let port = self.base.port_or_known_default().unwrap_or(631);
        IppValue::new_uri(format!("ipp://{host}:{port}/jobs/{job_id}")).map_err(refused)
    }

    async fn send(
        &self,
        uri: &Uri,
        request: impl Into<IppRequestResponse>,
    ) -> Result<IppRequestResponse, PrintError> {
        self.exchange(uri, request, None, None)
            .await
            .map_err(|failure| match failure {
                Failure::Unsent(error) => error,
                Failure::Unanswered(reason) => PrintError::Unavailable(reason),
            })
    }

    /// Sends an IPP request, followed by `document` if any, and reads the answer, telling a
    /// request CUPS never got from one it may have acted on. `timeout` replaces the client's.
    async fn exchange(
        &self,
        uri: &Uri,
        request: impl Into<IppRequestResponse>,
        document: Option<Bytes>,
        timeout: Option<Duration>,
    ) -> Result<IppRequestResponse, Failure> {
        let attributes = request.into().to_bytes();
        let body = match document {
            None => reqwest::Body::from(attributes),
            // The document goes out as it is, never copied next to the attributes: it can be
            // as large as the upload limit.
            Some(document) => reqwest::Body::wrap(Chunks(VecDeque::from([attributes, document]))),
        };

        let mut post = self
            .client
            .post(uri.to_string())
            .header("content-type", "application/ipp")
            .body(body);
        if let Some(timeout) = timeout {
            post = post.timeout(timeout);
        }
        let response = post.send().await.map_err(|error| {
            // Only a connection that never opened is sure not to have carried the request: a
            // timeout or a broken connection may come after CUPS read it.
            if error.is_connect() || error.is_builder() {
                Failure::Unsent(transport_error(error))
            } else {
                Failure::Unanswered(error_chain(&error))
            }
        })?;
        let status = response.status();
        if !status.is_success() {
            // A server error may come from a proxy that passed the request on.
            return Err(if status.is_server_error() {
                Failure::Unanswered(format!(
                    "CUPS answered with HTTP status {}",
                    status.as_u16()
                ))
            } else {
                Failure::Unsent(http_status_error(status.as_u16()))
            });
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| Failure::Unanswered(error_chain(&error)))?;
        IppParser::new(IppReader::new(Cursor::new(bytes)))
            .parse()
            .map_err(|error| Failure::Unanswered(format!("unreadable CUPS answer: {error}")))
    }

    /// The name of the default printer, if CUPS has one.
    async fn default_printer(&self) -> Result<Option<String>, PrintError> {
        let mut request = new_request(Operation::CupsGetDefault, None)?;
        add_requested(&mut request, &["printer-name"])?;
        let response = self.send(&self.root_uri()?, request).await?;
        match check(&response, || PrintError::PrinterNotFound(String::new())) {
            Ok(()) => Ok(printer_groups(&response)
                .next()
                .and_then(|group| string(group, "printer-name"))),
            // No default is a normal state of a fresh CUPS.
            Err(PrintError::PrinterNotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn jobs_with(
        &self,
        which: &'static str,
        limit: Option<i32>,
    ) -> Result<Vec<Job>, PrintError> {
        let root = self.root_uri()?;
        let mut request = new_request(Operation::GetJobs, Some(&root))?;
        add_operation(&mut request, attribute("which-jobs", keyword(which)?)?);
        if let Some(limit) = limit {
            add_operation(&mut request, attribute("limit", IppValue::Integer(limit))?);
        }
        add_requested(&mut request, &JOB_ATTRIBUTES)?;
        let response = self.send(&root, request).await?;
        check(&response, || {
            PrintError::Unavailable("CUPS has no job list".into())
        })?;
        Ok(response
            .attributes()
            .groups_of(DelimiterTag::JobAttributes)
            .filter_map(parse_job)
            .collect())
    }
}

#[async_trait::async_trait]
impl PrintService for Cups {
    async fn printers(&self) -> Result<Vec<Printer>, PrintError> {
        let mut request = new_request(Operation::CupsGetPrinters, None)?;
        add_requested(&mut request, &PRINTER_ATTRIBUTES)?;
        let response = self.send(&self.root_uri()?, request).await?;
        // A CUPS without any queue answers client-error-not-found rather than an empty list.
        match check(&response, || PrintError::PrinterNotFound(String::new())) {
            Ok(()) => {}
            Err(PrintError::PrinterNotFound(_)) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        }
        let default = self.default_printer().await?;
        Ok(printer_groups(&response)
            .filter_map(|group| parse_printer(group, default.as_deref()))
            .collect())
    }

    async fn printer(&self, name: &str) -> Result<Printer, PrintError> {
        let uri = self.printer_uri(name)?;
        let mut request = new_request(Operation::GetPrinterAttributes, Some(&uri))?;
        add_requested(&mut request, &PRINTER_ATTRIBUTES)?;
        let response = self.send(&uri, request).await?;
        let not_found = || PrintError::PrinterNotFound(name.to_owned());
        check(&response, not_found)?;
        let default = self.default_printer().await?;
        printer_groups(&response)
            .next()
            .and_then(|group| parse_printer(group, default.as_deref()))
            .ok_or_else(not_found)
    }

    async fn submit(&self, request: PrintRequest) -> Result<i32, PrintError> {
        let uri = self.printer_uri(&request.printer)?;
        // The builder keeps the URI as given, and CUPS expects the ipp scheme in the attribute.
        let operation = IppOperationBuilder::print_job(
            ipp::util::canonicalize_uri(&uri),
            // The document follows the attributes in `exchange`, without a copy.
            IppPayload::empty(),
        )
        .user_name(&request.user)
        .job_title(&request.job_name)
        .document_format(request.format.mime())
        .attributes(request.options.to_attributes(request.format))
        .build()
        .map_err(refused)?;
        // What follows sending may hide a job CUPS queued: the caller must not take it as a
        // refusal.
        let response = self
            .exchange(
                &uri,
                operation,
                Some(request.document),
                Some(SUBMIT_TIMEOUT),
            )
            .await
            .map_err(|failure| match failure {
                Failure::Unsent(error) => error,
                Failure::Unanswered(reason) => PrintError::Unconfirmed(reason),
            })?;
        check(&response, || {
            PrintError::PrinterNotFound(request.printer.clone())
        })?;
        response
            .attributes()
            .groups_of(DelimiterTag::JobAttributes)
            .find_map(|group| integer(group, "job-id"))
            .ok_or_else(|| PrintError::Unconfirmed("CUPS did not return a job id".into()))
    }

    async fn jobs(&self) -> Result<Vec<Job>, PrintError> {
        let mut jobs = self.jobs_with("not-completed", None).await?;
        jobs.extend(self.jobs_with("completed", Some(COMPLETED_JOBS)).await?);
        // CUPS orders the two lists differently, and ids only ever grow.
        jobs.sort_unstable_by_key(|job| std::cmp::Reverse(job.id));
        jobs.dedup_by_key(|job| job.id);
        Ok(jobs)
    }

    async fn job_status(&self, job_id: i32) -> Result<Option<JobStatus>, PrintError> {
        let mut request = new_request(Operation::GetJobAttributes, None)?;
        add_operation(&mut request, attribute("job-uri", self.job_uri(job_id)?)?);
        add_requested(&mut request, &["job-state", "job-impressions-completed"])?;
        let response = self.send(&self.root_uri()?, request).await?;
        match check(&response, || PrintError::JobNotFound(job_id)) {
            Ok(()) => {}
            Err(PrintError::JobNotFound(_)) => return Ok(None),
            Err(error) => return Err(error),
        }
        let group = response
            .attributes()
            .groups_of(DelimiterTag::JobAttributes)
            .next();
        // An answer without a readable state must not look like a finished job.
        let state = group
            .and_then(|group| integer(group, "job-state"))
            .and_then(JobState::from_ipp)
            .ok_or_else(|| PrintError::Unavailable("CUPS did not return the job state".into()))?;
        Ok(Some(JobStatus {
            state,
            impressions_completed: group
                .and_then(|group| integer(group, "job-impressions-completed"))
                .and_then(|impressions| u32::try_from(impressions).ok()),
        }))
    }

    async fn cancel(&self, job_id: i32, requested_by: &str) -> Result<(), PrintError> {
        let root = self.root_uri()?;
        let not_found = || PrintError::JobNotFound(job_id);

        // CUPS only lets the owner (or an admin) cancel a job, and recto's own user is neither,
        // so the request is made in the owner's name.
        let mut lookup = new_request(Operation::GetJobAttributes, None)?;
        add_operation(&mut lookup, attribute("job-uri", self.job_uri(job_id)?)?);
        add_requested(&mut lookup, &["job-originating-user-name"])?;
        let response = self.send(&root, lookup).await?;
        let owner = match check(&response, not_found) {
            Ok(()) => response
                .attributes()
                .groups_of(DelimiterTag::JobAttributes)
                .find_map(|group| string(group, "job-originating-user-name")),
            Err(PrintError::JobNotFound(id)) => return Err(PrintError::JobNotFound(id)),
            // CUPS may hide the owner behind a refusal; the requester is the best guess then.
            Err(PrintError::Refused(_)) => None,
            Err(error) => return Err(error),
        };

        let mut cancel = new_request(Operation::CancelJob, None)?;
        add_operation(&mut cancel, attribute("job-uri", self.job_uri(job_id)?)?);
        add_operation(
            &mut cancel,
            attribute(
                "requesting-user-name",
                IppValue::new_name_without_language(owner.as_deref().unwrap_or(requested_by))
                    .map_err(refused)?,
            )?,
        );
        let response = self.send(&root, cancel).await?;
        check(&response, not_found)
    }
}

/// An IPP request's attributes then its document, each sent as its own chunk. The exact length
/// gives CUPS a `Content-Length`.
struct Chunks(VecDeque<Bytes>);

impl http_body::Body for Chunks {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        Poll::Ready(self.0.pop_front().map(|chunk| Ok(Frame::data(chunk))))
    }

    fn is_end_stream(&self) -> bool {
        self.0.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.0.iter().map(|chunk| chunk.len() as u64).sum())
    }
}

fn to_uri(url: &Url) -> Result<Uri, PrintError> {
    url.as_str()
        .parse()
        .map_err(|error| PrintError::Unavailable(format!("invalid CUPS address: {error}")))
}

/// A request the caller built wrongly (an over-long name, say), not a CUPS failure.
fn refused(error: impl std::fmt::Display) -> PrintError {
    PrintError::Refused(error.to_string())
}

fn new_request(operation: Operation, uri: Option<&Uri>) -> Result<IppRequestResponse, PrintError> {
    IppRequestResponse::new(IppVersion::v1_1(), operation, uri.cloned()).map_err(refused)
}

fn attribute(name: &str, value: IppValue) -> Result<IppAttribute, PrintError> {
    IppAttribute::with_name(name, value).map_err(refused)
}

fn keyword(value: &str) -> Result<IppValue, PrintError> {
    IppValue::new_keyword(value).map_err(refused)
}

fn add_operation(request: &mut IppRequestResponse, attribute: IppAttribute) {
    request
        .attributes_mut()
        .add(DelimiterTag::OperationAttributes, attribute);
}

fn add_requested(request: &mut IppRequestResponse, names: &[&str]) -> Result<(), PrintError> {
    let values = names
        .iter()
        .map(|name| keyword(name))
        .collect::<Result<Vec<_>, _>>()?;
    let value = if values.len() == 1 {
        values.into_iter().next().expect("one value")
    } else {
        IppValue::Array(values)
    };
    add_operation(request, attribute("requested-attributes", value)?);
    Ok(())
}

fn http_status_error(code: u16) -> PrintError {
    match code {
        401 | 403 => PrintError::Refused("CUPS requires authentication".into()),
        // A wrong Host header (missing `ServerAlias *` in cupsd.conf) is answered with a 400.
        code => PrintError::Unavailable(format!("CUPS answered with HTTP status {code}")),
    }
}

/// Why an IPP request failed.
enum Failure {
    /// CUPS never got the request, or answered it with an error.
    Unsent(PrintError),
    /// The request reached CUPS, or may have, and no usable answer came back.
    Unanswered(String),
}

/// Network failures. The reason goes into the message so the banner is actionable.
fn transport_error(error: reqwest::Error) -> PrintError {
    PrintError::Unavailable(error_chain(&error))
}

/// The error and its causes, joined, so the reason is not lost behind "error sending request".
pub(crate) fn error_chain(error: &reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

/// Turns a non-success IPP status into the error the interface can explain.
fn check(
    response: &IppRequestResponse,
    not_found: impl FnOnce() -> PrintError,
) -> Result<(), PrintError> {
    let status = response.header().status_code();
    if status.is_success() {
        return Ok(());
    }
    let message = response
        .attributes()
        .groups_of(DelimiterTag::OperationAttributes)
        .find_map(|group| string(group, "status-message"))
        .unwrap_or_else(|| format!("{status:?}"));
    Err(match status {
        StatusCode::ClientErrorNotFound | StatusCode::ClientErrorGone => not_found(),
        StatusCode::ServerErrorNotAcceptingJobs => {
            PrintError::Refused("the printer is not accepting jobs".into())
        }
        StatusCode::ServerErrorInternalError
        | StatusCode::ServerErrorServiceUnavailable
        | StatusCode::ServerErrorDeviceError
        | StatusCode::ServerErrorTemporaryError
        | StatusCode::ServerErrorBusy => PrintError::Unavailable(message),
        _ => PrintError::Refused(message),
    })
}

pub(crate) fn printer_groups(
    response: &IppRequestResponse,
) -> impl Iterator<Item = &IppAttributeGroup> {
    response
        .attributes()
        .groups_of(DelimiterTag::PrinterAttributes)
}

/// Every value of an attribute, with multi-valued ones flattened.
fn values<'a>(group: &'a IppAttributeGroup, name: &str) -> Vec<&'a IppValue> {
    match group.get(name).map(IppAttribute::value) {
        Some(IppValue::Array(items)) => items.iter().collect(),
        Some(value) => vec![value],
        None => vec![],
    }
}

/// Only real text counts. Out-of-band values (`unknown`, `no-value`, ...) are how CUPS hides an
/// attribute, and they must never be mistaken for the text itself.
fn text_of(value: &IppValue) -> Option<String> {
    let text = match value {
        IppValue::TextWithoutLanguage(text) | IppValue::TextWithLanguage { text, .. } => {
            text.to_string()
        }
        IppValue::NameWithoutLanguage(name) | IppValue::NameWithLanguage { name, .. } => {
            name.to_string()
        }
        IppValue::Keyword(keyword) => keyword.to_string(),
        IppValue::Uri(uri) => uri.to_string(),
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

fn strings(group: &IppAttributeGroup, name: &str) -> Vec<String> {
    values(group, name)
        .into_iter()
        .filter_map(text_of)
        .collect()
}

pub(crate) fn string(group: &IppAttributeGroup, name: &str) -> Option<String> {
    strings(group, name).into_iter().next()
}

fn integers(group: &IppAttributeGroup, name: &str) -> Vec<i32> {
    values(group, name)
        .into_iter()
        .filter_map(|value| match value {
            IppValue::Integer(n) | IppValue::Enum(n) => Some(*n),
            _ => None,
        })
        .collect()
}

fn integer(group: &IppAttributeGroup, name: &str) -> Option<i32> {
    integers(group, name).into_iter().next()
}

fn parse_printer(group: &IppAttributeGroup, default: Option<&str>) -> Option<Printer> {
    let name = string(group, "printer-name")?;
    let sides: Vec<Sides> = strings(group, "sides-supported")
        .iter()
        .filter_map(|keyword| Sides::from_keyword(keyword))
        .collect();
    let color_modes: Vec<ColorMode> = strings(group, "print-color-mode-supported")
        .iter()
        .filter_map(|keyword| ColorMode::from_keyword(keyword))
        .collect();
    let max_copies = values(group, "copies-supported")
        .into_iter()
        .find_map(|value| match value {
            IppValue::RangeOfInteger { max, .. } => Some(*max),
            IppValue::Integer(n) => Some(*n),
            _ => None,
        })
        .and_then(|max| u32::try_from(max).ok())
        .filter(|max| *max >= 1)
        .unwrap_or(1);

    Some(Printer {
        is_default: default == Some(name.as_str()),
        description: string(group, "printer-info"),
        location: string(group, "printer-location"),
        make_and_model: string(group, "printer-make-and-model"),
        // Without a known state nothing says the printer works, so do not claim it is idle.
        state: integer(group, "printer-state")
            .and_then(PrinterState::from_ipp)
            .unwrap_or(PrinterState::Stopped),
        reasons: strings(group, "printer-state-reasons")
            .iter()
            .filter_map(|keyword| StateReason::parse(keyword))
            .collect(),
        // CUPS always sends it; a missing value should not lock the print form.
        accepting_jobs: group
            .get("printer-is-accepting-jobs")
            .and_then(|a| a.value().as_boolean().copied())
            .unwrap_or(true),
        supplies: supplies(
            &strings(group, "marker-names"),
            &integers(group, "marker-levels"),
            &strings(group, "marker-colors"),
            &strings(group, "marker-types"),
        ),
        capabilities: Capabilities {
            max_copies,
            // A printer that does not list them still prints single-sided and in color.
            sides: if sides.is_empty() {
                vec![Sides::OneSided]
            } else {
                sides
            },
            color_modes: if color_modes.is_empty() {
                vec![ColorMode::Color]
            } else {
                color_modes
            },
        },
        name,
    })
}

fn parse_job(group: &IppAttributeGroup) -> Option<Job> {
    let printer = string(group, "job-printer-uri")
        .and_then(|uri| Url::parse(&uri).ok())
        .and_then(|url| {
            url.path_segments()?
                .next_back()
                .map(|segment| percent_decode_str(segment).decode_utf8_lossy().into_owned())
        })
        .unwrap_or_default();
    let pages = integer(group, "job-media-sheets-completed")
        .or_else(|| integer(group, "job-impressions-completed"))
        .and_then(|pages| u32::try_from(pages).ok());
    Some(Job {
        id: integer(group, "job-id")?,
        printer,
        name: string(group, "job-name"),
        owner: string(group, "job-originating-user-name"),
        state: JobState::from_ipp(integer(group, "job-state")?)?,
        pages,
        created: integer(group, "time-at-creation").map(i64::from),
        counted_pages: None,
        cost: None,
    })
}
