//! recto as an IPP printer for computers and phones: `POST /ipp/print`, with a device password
//! as the password of HTTP Basic, and `POST /device/<secret>/ipp/print`, the personal address
//! for devices that cannot answer a password prompt; `/<job id>` appended to either is the job
//! URI recto hands out. Any other address under `/ipp/`, `/device/` or `/eSCL/` is a bare 404:
//! never the sign-in redirect, which would carry a secret into a `Location` header.
//!
//! These routes sit outside the session and cross-site layers: devices have neither cookies
//! nor `Origin` headers, and authenticate with [`device_auth`]. A personal address is checked
//! before any byte of the body is read; on `/ipp/print`, the 8-byte IPP header is read first,
//! since the operation is in the body, and credentials are required unless it is
//! Get-Printer-Attributes, which clients send before they know they need any.
//!
//! Documents are **held**: written to the held documents' directory and reported completed to
//! the device, they wait on the Jobs page until their owner releases or deletes them, or they
//! expire. Rasters (PWG and Apple) are converted to a PDF first, so everything after sees one
//! format. A user who prints directly has each job released at once to their printer, through
//! the same path as the Jobs page; a refusal leaves it held and the device is told the job
//! completed with errors. Job operations see only the jobs of the device password that sent
//! them. Every URI in an answer is built on the address the request came in on, so a
//! device that switches to the URI it is given keeps its personal address. On the LAN listener
//! the host is the request's `Host`, `ipps://recto-<hostname>.local:8631` for a device that
//! found recto through DNS-SD, since the public URL may not lead there from the LAN.
//!
//! Reading a body is bounded in size, time and number: `RECTO_MAX_UPLOAD_SIZE` (64 KiB without
//! credentials, since Get-Printer-Attributes carries no document), the device read timeout for
//! the header and for each later piece of the body (`408` past it), and two documents per user
//! at once (`server-error-busy` for a third, before its document is read).
//!
//! Log lines name the device password's id, never the path, which may hold the secret.

use std::{future::poll_fn, pin::Pin, sync::Arc, time::Duration};

use ::ipp::{
    attribute::IppAttribute,
    model::{DelimiterTag, StatusCode as IppStatus},
};
use axum::{
    Router,
    body::{Body, Bytes, HttpBody},
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use chrono::Utc;
use recto_app::i18n::Locale;
use recto_app::{
    refusal::Refusal,
    state::{AppState, User},
};
use recto_domain::{
    Error, Problem,
    device_passwords::DeviceUser,
    held::{self, HeldJob, HeldState, NewHeld, Stored},
    users,
};
use recto_printing::{
    Capabilities, ColorMode, DocumentFormat, Sides,
    printer::{
        self, DOCUMENT_FORMATS, GET_PRINTER_ATTRIBUTES, JobView, JobViewState, Operation,
        PrinterUri, Requested, TOO_MANY_JOBS, VirtualPrinter, WhichJobs,
    },
    raster::{RasterError, RasterFormat},
};
use url::Url;

use super::held::release_held;
use super::print::{Choices, FORM_SLACK, Rejected, Submitted, job_name};
use crate::{
    device_auth::{self, DeviceAuth, Listener, Uploads},
    state::AuthState,
};

/// The model devices see; also what recto's own scanner list leaves out, so recto never lists
/// itself.
const MAKE_AND_MODEL: &str = "recto";

/// How many times `RECTO_MAX_UPLOAD_SIZE` a user's waiting documents may take in all.
const HELD_BYTES_FACTOR: i64 = 10;

/// How long the answer to a job printed directly waits for the outcome. The release goes on
/// after that whatever happens to the request; the device is then told it is being released.
const DIRECT_WAIT: Duration = Duration::from_secs(20);

#[derive(Clone)]
struct IppState {
    app: AppState,
    devices: DeviceAuth,
    printer: Arc<VirtualPrinter>,
    /// `ipps://host:port` (or `ipp://` behind an `http` public URL): where devices reach recto
    /// through the proxy. `None` on the LAN listener, where it is the `Host` the device used.
    address: Option<Arc<str>>,
    tls: bool,
    /// The largest body read: the document and its IPP attributes.
    body_limit: usize,
    /// How long the header, then each later piece of a body, may take to arrive.
    read_timeout: Duration,
    /// The documents each user is sending, shared by every route serving devices.
    uploads: Uploads,
}

/// The largest body of a request without credentials: only Get-Printer-Attributes is answered
/// without them, and it carries no document.
const UNAUTHENTICATED_BODY: usize = FORM_SLACK;

/// The operations that carry a document: Print-Job and Send-Document.
const DOCUMENT_OPERATIONS: [u16; 2] = [0x02, 0x06];

/// `ipps://host:port` for an `https` public URL, `ipp://host:port` for an `http` one, always
/// with its port: `ipps` alone means port 631. The bool says whether it is TLS.
pub fn printer_address(public_url: &Url) -> (String, bool) {
    let tls = public_url.scheme() == "https";
    let scheme = if tls { "ipps" } else { "ipp" };
    let host = public_url.host_str().unwrap_or("localhost");
    let port = public_url
        .port_or_known_default()
        .unwrap_or(if tls { 443 } else { 80 });
    (format!("{scheme}://{host}:{port}"), tls)
}

/// The IPP routes, and the catch-alls answering 404 under `/ipp/`, `/device/` and `/eSCL/`.
/// With OS integration off, everything there is a 404.
pub fn router(app: AppState, auth: &AuthState, listener: Listener) -> Router {
    let (address, tls) = match listener {
        Listener::Main => {
            let (address, tls) = printer_address(&auth.public_url);
            (Some(Arc::from(address)), tls)
        }
        Listener::Lan => (None, true),
    };
    let public = |path: &str| auth.public(path).to_string();
    let printer = VirtualPrinter {
        name: app.branding.name.clone(),
        make_and_model: MAKE_AND_MODEL.to_owned(),
        uuid: VirtualPrinter::uuid_for(auth.public_url.as_str()),
        more_info: public("jobs"),
        icons: vec![public("brand/icon-192.png"), public("brand/icon-512.png")],
        default_media: auth.default_media,
        formats: DOCUMENT_FORMATS.to_vec(),
        started_at: Utc::now(),
        // Startup warned already: devices are told before they send.
        accepting_jobs: app.held.dir().is_dir(),
    };
    let state = IppState {
        devices: DeviceAuth::for_listener(auth, &app.branding.name, listener),
        printer: Arc::new(printer),
        address,
        tls,
        body_limit: app.max_upload_size.saturating_add(FORM_SLACK),
        read_timeout: auth.device_read_timeout,
        uploads: auth.device_limits.uploads.clone(),
        app,
    };
    Router::new()
        .route("/ipp/{*rest}", any(basic))
        .route("/device/{*rest}", any(personal))
        .route("/eSCL/{*rest}", any(|| async { StatusCode::NOT_FOUND }))
        .with_state(state)
}

/// `/ipp/print` and `/ipp/print/<job>`.
async fn basic(State(state): State<IppState>, request: Request) -> Response {
    let path = request.uri().path();
    let known = path
        .strip_prefix("/ipp/print")
        .is_some_and(|rest| rest.is_empty() || job_segment(rest));
    if !known || request.method() != Method::POST || !state.app.os_integration {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve(state, request, None).await
}

/// `/device/<secret>/ipp/print` and `/device/<secret>/ipp/print/<job>`.
async fn personal(State(state): State<IppState>, request: Request) -> Response {
    let secret = request
        .uri()
        .path()
        .strip_prefix(device_auth::PERSONAL_PREFIX)
        .and_then(|rest| rest.split_once('/'))
        .filter(|(secret, rest)| {
            !secret.is_empty()
                && rest
                    .strip_prefix("ipp/print")
                    .is_some_and(|rest| rest.is_empty() || job_segment(rest))
        })
        .map(|(secret, _)| secret.to_owned());
    match secret {
        Some(secret) if request.method() == Method::POST && state.app.os_integration => {
            serve(state, request, Some(secret)).await
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `/<digits>`: a job URI's last segment.
fn job_segment(rest: &str) -> bool {
    rest.strip_prefix('/')
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
}

/// `ipps://<Host>` on the LAN listener: devices there use recto's `.local` name or one of its
/// addresses, and switch to the URI they are given. `None` without a usable `Host`, a bare
/// authority without user information.
fn lan_address(headers: &axum::http::HeaderMap) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let authority: axum::http::uri::Authority = host.parse().ok()?;
    (!authority.as_str().contains('@')).then(|| format!("ipps://{authority}"))
}

/// Authenticates, reads and answers one IPP request.
async fn serve(state: IppState, request: Request, secret: Option<String>) -> Response {
    let (parts, mut body) = request.into_parts();
    let address = match &state.address {
        Some(address) => address.to_string(),
        None => match lan_address(&parts.headers) {
            Some(address) => address,
            None => return StatusCode::BAD_REQUEST.into_response(),
        },
    };
    let limit = state.body_limit;
    let announced = parts
        .headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if announced.is_some_and(|length| length > limit as u64) {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    // On a personal address the secret is in the path: checked before any byte is read.
    let mut device = match &secret {
        Some(secret) => match device_auth::authenticate(&state.devices, &parts, Some(secret)).await
        {
            Ok(device) => Some(device),
            Err(refusal) => return state.devices.refuse(refusal),
        },
        None => None,
    };
    let mut buffer = Vec::new();
    let timeout = state.read_timeout;
    // The header as a whole within the timeout, however it is split.
    let header = tokio::time::timeout(timeout, fill(&mut body, &mut buffer, 8, limit, timeout));
    match header.await {
        Ok(Ok(())) => {}
        Ok(Err(unread)) => return unread.into_response(),
        Err(_) => return Unread::TimedOut.into_response(),
    }
    let Some(first) = buffer
        .get(..8)
        .and_then(|bytes| <&[u8; 8]>::try_from(bytes).ok())
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let head = printer::header(first);
    if device.is_none() && head.operation != GET_PRINTER_ATTRIBUTES {
        match device_auth::authenticate(&state.devices, &parts, None).await {
            Ok(user) => device = Some(user),
            Err(refusal) => return state.devices.refuse(refusal),
        }
    }
    let limit = if device.is_some() {
        limit
    } else {
        limit.min(UNAUTHENTICATED_BODY)
    };
    // Kept until the document is held, or refused: it is in memory until then.
    let _upload = match &device {
        Some(device) if DOCUMENT_OPERATIONS.contains(&head.operation) => {
            match state.uploads.begin(device.user_id) {
                Some(upload) => Some(upload),
                None => {
                    tracing::info!(
                        user = device.username,
                        device_password_id = device.device_password_id,
                        "a third document at once refused"
                    );
                    return answer(printer::refusal(&head, BUSY));
                }
            }
        }
        _ => None,
    };
    if let Err(unread) = fill(&mut body, &mut buffer, usize::MAX, limit, timeout).await {
        return unread.into_response();
    }
    let incoming = match printer::decode(Bytes::from(buffer)) {
        Ok(incoming) => incoming,
        Err(status) => return answer(printer::refusal(&head, status as u16)),
    };
    let path = match &secret {
        Some(secret) => format!("{}{secret}/ipp/print", device_auth::PERSONAL_PREFIX),
        None => "/ipp/print".to_owned(),
    };
    let uri = PrinterUri {
        uri: format!("{address}{path}"),
        tls: state.tls,
        basic: secret.is_none(),
    };
    let request = Job {
        state: &state,
        device: device.as_ref(),
        uri: &uri,
        locale: recto_app::i18n::from_headers(&parts.headers),
    };
    let (status, groups) = request.dispatch(incoming.operation.clone()).await;
    answer(printer::response(&incoming, status, groups))
}

/// Why a body could not be read.
enum Unread {
    TooLarge,
    Failed,
    TimedOut,
}

impl IntoResponse for Unread {
    fn into_response(self) -> Response {
        match self {
            Unread::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Unread::Failed => StatusCode::BAD_REQUEST,
            Unread::TimedOut => StatusCode::REQUEST_TIMEOUT,
        }
        .into_response()
    }
}

/// Reads the body into `buffer` until it holds `wanted` bytes or the body ends, refusing more
/// than `limit` in all, whatever the body announced: chunked bodies announce nothing. Each piece
/// must arrive within `idle` of the last.
async fn fill(
    body: &mut Body,
    buffer: &mut Vec<u8>,
    wanted: usize,
    limit: usize,
    idle: Duration,
) -> Result<(), Unread> {
    // What an earlier read took may already be over a limit lowered since.
    if buffer.len() > limit {
        return Err(Unread::TooLarge);
    }
    while buffer.len() < wanted {
        let next = poll_fn(|context| Pin::new(&mut *body).poll_frame(context));
        let frame = tokio::time::timeout(idle, next)
            .await
            .map_err(|_| Unread::TimedOut)?;
        let Some(frame) = frame else {
            return Ok(());
        };
        let Ok(data) = frame.map_err(|_| Unread::Failed)?.into_data() else {
            continue;
        };
        if buffer.len().saturating_add(data.len()) > limit {
            return Err(Unread::TooLarge);
        }
        buffer.extend_from_slice(&data);
    }
    Ok(())
}

/// An IPP answer: `Cache-Control: no-cache` is a must of PWG 5100.14.
fn answer(bytes: Bytes) -> Response {
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/ipp"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
        bytes,
    )
        .into_response()
}

type Groups = Vec<(DelimiterTag, Vec<IppAttribute>)>;

const OK: u16 = IppStatus::SuccessfulOk as u16;
const BAD_REQUEST: u16 = IppStatus::ClientErrorBadRequest as u16;
const NOT_POSSIBLE: u16 = IppStatus::ClientErrorNotPossible as u16;
const NOT_FOUND: u16 = IppStatus::ClientErrorNotFound as u16;
const TOO_LARGE: u16 = IppStatus::ClientErrorRequestEntityTooLong as u16;
const FORMAT_NOT_SUPPORTED: u16 = IppStatus::ClientErrorDocumentFormatNotSupported as u16;
const FORMAT_ERROR: u16 = IppStatus::ClientErrorDocumentFormatError as u16;
const BUSY: u16 = IppStatus::ServerErrorBusy as u16;
const INTERNAL: u16 = IppStatus::ServerErrorInternalError as u16;
const NOT_ACCEPTING: u16 = IppStatus::ServerErrorNotAcceptingJobs as u16;
const OPERATION_NOT_SUPPORTED: u16 = IppStatus::ServerErrorOperationNotSupported as u16;
const ONE_DOCUMENT: u16 = IppStatus::ServerErrorMultipleDocumentJobsNotSupported as u16;

fn status(status: u16) -> (u16, Groups) {
    (status, Vec::new())
}

/// The answer to a raster that was not converted. Content recto cannot read is a format error;
/// one that would cost too much, in pixels, pages, output or time, is too large, like a document
/// over `RECTO_MAX_UPLOAD_SIZE`; only waiting for a place is worth a retry.
fn raster_status(error: RasterError) -> u16 {
    match error {
        RasterError::Truncated | RasterError::Malformed(_) | RasterError::Unsupported(_) => {
            FORMAT_ERROR
        }
        RasterError::PageTooLarge
        | RasterError::TooManyPages
        | RasterError::DocumentTooLarge
        | RasterError::TooLarge
        | RasterError::TimedOut => TOO_LARGE,
        RasterError::Busy => BUSY,
    }
}

/// One authenticated request: what its operations need.
struct Job<'a> {
    state: &'a IppState,
    /// `None` only for Get-Printer-Attributes on `/ipp/print` without credentials.
    device: Option<&'a DeviceUser>,
    uri: &'a PrinterUri,
    locale: Locale,
}

impl Job<'_> {
    async fn dispatch(&self, operation: Operation) -> (u16, Groups) {
        if let Operation::GetPrinterAttributes { requested } = &operation {
            return self.printer_attributes(requested).await;
        }
        let Some(device) = self.device else {
            // Every other operation was authenticated before its body was read.
            return status(INTERNAL);
        };
        let result = match operation {
            Operation::GetPrinterAttributes { .. } => unreachable!("answered above"),
            Operation::PrintJob {
                name,
                format,
                requested,
                document,
            } => {
                let name = job_name(name.as_deref().unwrap_or_default());
                self.store(device, Received::New { name, requested }, format, document)
                    .await
            }
            Operation::ValidateJob { format } => Ok(self.validate(format.as_deref())),
            Operation::CreateJob { name, requested } => {
                let name = job_name(name.as_deref().unwrap_or_default());
                self.create(device, &name, &requested).await
            }
            Operation::SendDocument {
                job_id,
                format,
                document,
                last: _,
            } => self.send_document(device, job_id, format, document).await,
            Operation::CloseJob { job_id } => self.close(device, job_id).await,
            Operation::CancelJob { job_id } => self.cancel(device, job_id).await,
            Operation::CancelMyJobs { job_ids } => self.cancel_mine(device, &job_ids).await,
            Operation::GetJobAttributes { job_id, requested } => {
                self.job_attributes(device, job_id, &requested).await
            }
            Operation::GetJobs {
                which,
                limit,
                requested,
            } => self.jobs(device, which, limit, &requested).await,
            Operation::IdentifyPrinter => Ok(status(OK)),
            Operation::Unsupported(_) => Ok(status(OPERATION_NOT_SUPPORTED)),
        };
        result.unwrap_or_else(|error| {
            tracing::error!(
                %error,
                user = device.username,
                device_password_id = device.device_password_id,
                "an IPP request failed"
            );
            status(INTERNAL)
        })
    }

    async fn printer_attributes(&self, requested: &[String]) -> (u16, Groups) {
        // The device's jobs waiting for their document; nobody's without credentials.
        let queued = match self.device {
            Some(device) => {
                held::list_for_device(&self.state.app.database, device.device_password_id)
                    .await
                    .map(|jobs| {
                        jobs.iter()
                            .filter(|job| job.state == HeldState::Incoming)
                            .count()
                    })
                    .unwrap_or_default()
            }
            None => 0,
        };
        let attributes = self.state.printer.printer_attributes(
            self.uri,
            requested,
            u32::try_from(queued).unwrap_or(u32::MAX),
            Utc::now(),
        );
        (OK, vec![(DelimiterTag::PrinterAttributes, attributes)])
    }

    /// Validate-Job: whether a job of `format` would be taken.
    fn validate(&self, format: Option<&str>) -> (u16, Groups) {
        if !self.state.printer.accepting_jobs {
            return status(NOT_ACCEPTING);
        }
        match format {
            Some(format) if !self.state.printer.accepts(format) => status(FORMAT_NOT_SUPPORTED),
            _ => status(OK),
        }
    }

    async fn create(
        &self,
        device: &DeviceUser,
        name: &str,
        requested: &Requested,
    ) -> recto_domain::Result<(u16, Groups)> {
        if !self.state.printer.accepting_jobs {
            return Ok(status(NOT_ACCEPTING));
        }
        let now = Utc::now();
        let created = held::create_incoming(
            &self.state.app.database,
            device.user_id,
            device.device_password_id,
            name,
            &requested_json(requested),
            now,
            self.state.app.held_lifetime,
            self.max_bytes(),
        )
        .await;
        let id = match created {
            Ok(id) => id,
            Err(Error::Invalid(Problem::TooManyHeldJobs { .. })) => {
                return Ok(status(TOO_MANY_JOBS));
            }
            Err(error) => return Err(error),
        };
        let view = JobView {
            id: ipp_id(id)?,
            name: name.to_owned(),
            device: device.device_name.clone(),
            state: JobViewState::Pending,
            received_at: now,
            pages: None,
        };
        Ok(self.created(&view, &self.message(HeldState::Incoming), now))
    }

    async fn send_document(
        &self,
        device: &DeviceUser,
        job_id: i32,
        format: Option<String>,
        document: Bytes,
    ) -> recto_domain::Result<(u16, Groups)> {
        let database = &self.state.app.database;
        let Some(job) =
            held::get_for_device(database, device.device_password_id, i64::from(job_id)).await?
        else {
            return Ok(status(NOT_FOUND));
        };
        if job.state != HeldState::Incoming {
            return Ok(status(ONE_DOCUMENT));
        }
        self.store(device, Received::Incoming(job), format, document)
            .await
    }

    /// Checks a document and holds it: as a new job, or as the document of an incoming one.
    async fn store(
        &self,
        device: &DeviceUser,
        received: Received,
        declared: Option<String>,
        document: Bytes,
    ) -> recto_domain::Result<(u16, Groups)> {
        let app = &self.state.app;
        if !self.state.printer.accepting_jobs {
            return Ok(status(NOT_ACCEPTING));
        }
        if declared
            .as_deref()
            .is_some_and(|format| !self.state.printer.accepts(format))
        {
            return Ok(status(FORMAT_NOT_SUPPORTED));
        }
        if document.is_empty() {
            return Ok(status(BAD_REQUEST));
        }
        if document.len() > app.max_upload_size {
            return Ok(status(TOO_LARGE));
        }
        // From the content, never the declared type, as for the print form.
        let raster = RasterFormat::detect(&document);
        if DocumentFormat::detect(&document).is_none() && raster.is_none() {
            return Ok(status(FORMAT_NOT_SUPPORTED));
        }
        let size = i64::try_from(document.len()).unwrap_or(i64::MAX);
        let room = match &received {
            Received::New { .. } => {
                held::room(&app.database, device.user_id, size, self.max_bytes()).await?
            }
            Received::Incoming(job) => {
                held::room_for(
                    &app.database,
                    device.user_id,
                    job.id,
                    size,
                    self.max_bytes(),
                )
                .await?
            }
        };
        // Before anything is converted or written; the insert checks again with the PDF's size.
        if !room {
            return Ok(status(TOO_MANY_JOBS));
        }
        // A raster becomes a PDF here, so the file, the count, the preview and CUPS see one
        // format; the PDF stays within the upload size.
        let (document, format, received_mime, converted_pages) = match raster {
            Some(raster) => {
                match app
                    .raster
                    .convert(document, raster, app.max_upload_size)
                    .await
                {
                    Ok(converted) => (
                        Bytes::from(converted.pdf),
                        DocumentFormat::Pdf,
                        raster.mime(),
                        Some(converted.pages),
                    ),
                    Err(error) => {
                        tracing::info!(
                            %error,
                            user = device.username,
                            device_password_id = device.device_password_id,
                            format = raster.mime(),
                            "raster refused"
                        );
                        return Ok(status(raster_status(error)));
                    }
                }
            }
            None => {
                let Some(format) = DocumentFormat::detect(&document) else {
                    return Ok(status(FORMAT_NOT_SUPPORTED));
                };
                (document, format, format.mime(), None)
            }
        };
        let size = i64::try_from(document.len()).unwrap_or(i64::MAX);
        let pages = match (converted_pages, format) {
            (Some(pages), _) => Some(pages),
            // A PDF recto cannot count is still held: CUPS may print it, and the release
            // counts again.
            (None, DocumentFormat::Pdf) => app.page_counter.count(document.clone()).await.ok(),
            (None, DocumentFormat::Jpeg | DocumentFormat::Png) => Some(1),
        }
        .map(i64::from);
        let files = Arc::clone(&app.held);
        let written = tokio::task::spawn_blocking(move || files.write(&document)).await;
        let file = match written {
            Ok(Ok(file)) => file,
            Ok(Err(error)) => {
                tracing::error!(%error, "writing a held document failed");
                return Ok(status(INTERNAL));
            }
            Err(error) => {
                tracing::error!(%error, "writing a held document failed");
                return Ok(status(INTERNAL));
            }
        };
        let stored = Stored {
            file: file.clone(),
            stored_format: format.mime().to_owned(),
            received_format: Some(
                declared
                    .filter(|declared| declared != "application/octet-stream")
                    .unwrap_or_else(|| received_mime.to_owned()),
            ),
            document_pages: pages,
            size,
        };
        let now = Utc::now();
        let (recorded, requested) = match received {
            Received::New { name, requested } => (
                held::receive(
                    &app.database,
                    NewHeld {
                        user_id: device.user_id,
                        device_password_id: Some(device.device_password_id),
                        name: name.clone(),
                        requested: requested_json(&requested),
                        stored,
                        now,
                        lifetime: app.held_lifetime,
                    },
                    self.max_bytes(),
                )
                .await
                .map(|id| Some((id, name, now))),
                requested,
            ),
            Received::Incoming(job) => {
                // Options that no longer read are not worth refusing the document for.
                let requested = serde_json::from_str(&job.requested).unwrap_or_default();
                (
                    held::attach(
                        &app.database,
                        device.device_password_id,
                        job.id,
                        stored,
                        self.max_bytes(),
                    )
                    .await
                    .map(|attached| attached.then_some((job.id, job.name, job.received_at))),
                    requested,
                )
            }
        };
        let (id, name, received_at) = match recorded {
            Ok(Some(recorded)) => recorded,
            Ok(None) => {
                self.forget(file).await;
                return Ok(status(NOT_FOUND));
            }
            Err(Error::Invalid(Problem::TooManyHeldJobs { .. })) => {
                self.forget(file).await;
                return Ok(status(TOO_MANY_JOBS));
            }
            Err(error) => {
                self.forget(file).await;
                return Err(error);
            }
        };
        tracing::info!(
            user = device.username,
            device_password_id = device.device_password_id,
            job = id,
            pages,
            format = received_mime,
            "job held"
        );
        let (state, message) = self.print_directly(device, id, &requested).await;
        let view = JobView {
            id: ipp_id(id)?,
            name,
            device: device.device_name.clone(),
            state,
            received_at,
            pages,
        };
        Ok(self.created(&view, &message, now))
    }

    /// Releases a job just held when its owner prints directly, and says how it went. The
    /// release runs in a task of its own, so a device hanging up cannot cut it between the
    /// ledger and CUPS; the answer waits for it at most [`DIRECT_WAIT`].
    async fn print_directly(
        &self,
        device: &DeviceUser,
        id: i64,
        requested: &Requested,
    ) -> (JobViewState, String) {
        let held = (JobViewState::Completed, self.message(HeldState::Held));
        let app = self.state.app.clone();
        let user = match users::find_by_id(&app.database, device.user_id).await {
            Ok(Some(user)) => user,
            Ok(None) => return held,
            Err(error) => {
                // The job stays held, as if the user held their jobs.
                tracing::error!(%error, job = id, "reading the direct printer failed");
                return held;
            }
        };
        let Some(printer) = user.direct_printer.clone() else {
            return held;
        };
        let user = User {
            id: user.id,
            username: user.username,
            name: user.display_name,
            is_administrator: user.is_administrator,
            has_password: user.password_hash.is_some(),
        };
        let requested = requested.clone();
        let release = tokio::spawn(async move {
            // Fitted to the printer: the device laid the document out for a generic one.
            let capabilities = app
                .print
                .printer(&printer)
                .await
                .ok()
                .map(|printer| printer.capabilities);
            let choices = direct_choices(&printer, &requested, capabilities.as_ref());
            release_held(&app, &user, id, choices).await
        });
        let outcome = match tokio::time::timeout(DIRECT_WAIT, release).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => {
                tracing::error!(%error, job = id, "printing a job directly failed");
                return held;
            }
            Err(_) => {
                return (JobViewState::Completed, self.message(HeldState::Releasing));
            }
        };
        match outcome {
            Ok(Submitted { .. }) | Err(Rejected::Refusal(Refusal::Unconfirmed)) => {
                (JobViewState::Completed, self.message(HeldState::Released))
            }
            Err(rejected) => {
                let refusal = match rejected {
                    Rejected::Quota(_) => Refusal::Quota,
                    Rejected::Refusal(refusal) => refusal,
                };
                tracing::info!(
                    user = device.username,
                    device_password_id = device.device_password_id,
                    job = id,
                    refusal = refusal.code(),
                    "printing directly refused, job held"
                );
                (JobViewState::CompletedWithErrors, self.refused(refusal))
            }
        }
    }

    /// Removes a document written for a job that was not recorded; the orphan sweep catches
    /// what this misses.
    async fn forget(&self, file: String) {
        let files = Arc::clone(&self.state.app.held);
        let removed = tokio::task::spawn_blocking(move || files.remove(&file)).await;
        if !matches!(removed, Ok(Ok(()))) {
            tracing::warn!("removing a held document that was not recorded failed");
        }
    }

    /// Close-Job: a job still without its document is aborted, which deletes it.
    async fn close(&self, device: &DeviceUser, job_id: i32) -> recto_domain::Result<(u16, Groups)> {
        let database = &self.state.app.database;
        let id = i64::from(job_id);
        let Some(job) = held::get_for_device(database, device.device_password_id, id).await? else {
            return Ok(status(NOT_FOUND));
        };
        if job.state == HeldState::Incoming {
            held::delete_for_device(database, device.device_password_id, id).await?;
        }
        Ok(status(OK))
    }

    /// Cancel-Job: only a job still waiting for its document. A held job is reported completed,
    /// and a completed job cannot be canceled (RFC 8011 section 4.3.3): its owner deletes it on
    /// the Jobs page.
    async fn cancel(
        &self,
        device: &DeviceUser,
        job_id: i32,
    ) -> recto_domain::Result<(u16, Groups)> {
        let database = &self.state.app.database;
        let id = i64::from(job_id);
        let Some(job) = held::get_for_device(database, device.device_password_id, id).await? else {
            return Ok(status(NOT_FOUND));
        };
        if job.state != HeldState::Incoming {
            return Ok(status(NOT_POSSIBLE));
        }
        match held::delete_for_device(database, device.device_password_id, id).await? {
            Some(_) => {
                tracing::info!(
                    user = device.username,
                    device_password_id = device.device_password_id,
                    job = id,
                    "held job canceled by its device"
                );
                Ok(status(OK))
            }
            // Its document arrived meanwhile.
            None => Ok(status(NOT_POSSIBLE)),
        }
    }

    /// Cancel-My-Jobs: the device's jobs still waiting for their document, or those of
    /// `job_ids`; held jobs are completed and stay.
    async fn cancel_mine(
        &self,
        device: &DeviceUser,
        job_ids: &[i32],
    ) -> recto_domain::Result<(u16, Groups)> {
        let database = &self.state.app.database;
        let jobs = held::list_for_device(database, device.device_password_id).await?;
        for job in jobs.iter().filter(|job| {
            job.state == HeldState::Incoming
                && (job_ids.is_empty() || job_ids.iter().any(|id| i64::from(*id) == job.id))
        }) {
            held::delete_for_device(database, device.device_password_id, job.id).await?;
        }
        Ok(status(OK))
    }

    async fn job_attributes(
        &self,
        device: &DeviceUser,
        job_id: i32,
        requested: &[String],
    ) -> recto_domain::Result<(u16, Groups)> {
        let database = &self.state.app.database;
        let Some(job) =
            held::get_for_device(database, device.device_password_id, i64::from(job_id)).await?
        else {
            return Ok(status(NOT_FOUND));
        };
        let attributes = self.describe(device, &job, requested)?;
        Ok((OK, vec![(DelimiterTag::JobAttributes, attributes)]))
    }

    async fn jobs(
        &self,
        device: &DeviceUser,
        which: WhichJobs,
        limit: Option<u32>,
        requested: &[String],
    ) -> recto_domain::Result<(u16, Groups)> {
        if which == WhichJobs::Unsupported {
            // The answer names it: the decoder put it with the unsupported attributes.
            return Ok(status(
                IppStatus::ClientErrorAttributesOrValuesNotSupported as u16,
            ));
        }
        let jobs =
            held::list_for_device(&self.state.app.database, device.device_password_id).await?;
        let limit = limit.map_or(usize::MAX, |limit| limit as usize);
        let mut groups = Vec::new();
        for job in jobs
            .iter()
            .filter(|job| {
                let pending = job.state == HeldState::Incoming;
                match which {
                    WhichJobs::NotCompleted => pending,
                    WhichJobs::Completed => !pending,
                    WhichJobs::All => true,
                    WhichJobs::Unsupported => false,
                }
            })
            .take(limit)
        {
            groups.push((
                DelimiterTag::JobAttributes,
                self.describe(device, job, requested)?,
            ));
        }
        Ok((OK, groups))
    }

    /// A job's attributes as its device sees it.
    fn describe(
        &self,
        device: &DeviceUser,
        job: &HeldJob,
        requested: &[String],
    ) -> recto_domain::Result<Vec<IppAttribute>> {
        let refusal = job.refusal.as_deref().and_then(Refusal::from_code);
        let (state, message) = match (job.state, refusal) {
            (HeldState::Incoming, _) => (JobViewState::Pending, self.message(job.state)),
            // Not printed when it was released, directly or from the Jobs page.
            (HeldState::Held, Some(refusal)) => {
                (JobViewState::CompletedWithErrors, self.refused(refusal))
            }
            (state, _) => (JobViewState::Completed, self.message(state)),
        };
        let view = JobView {
            id: ipp_id(job.id)?,
            name: job.name.clone(),
            device: device.device_name.clone(),
            state,
            received_at: job.received_at,
            pages: job.document_pages,
        };
        Ok(self
            .state
            .printer
            .job_attributes(&view, self.uri, &message, requested, Utc::now()))
    }

    /// The job group of a Print-Job, Create-Job or Send-Document answer.
    fn created(&self, view: &JobView, message: &str, now: chrono::DateTime<Utc>) -> (u16, Groups) {
        let attributes = self
            .state
            .printer
            .created_job_attributes(view, self.uri, message, now);
        (OK, vec![(DelimiterTag::JobAttributes, attributes)])
    }

    fn message(&self, state: HeldState) -> String {
        recto_app::messages::held_job(state, &self.state.app.branding.name, self.locale)
    }

    /// The message of a held job whose release was refused.
    fn refused(&self, refusal: Refusal) -> String {
        recto_app::messages::direct_refused(
            &refusal.message(),
            &self.state.app.branding.name,
            self.locale,
        )
    }

    fn max_bytes(&self) -> i64 {
        i64::try_from(self.state.app.max_upload_size)
            .unwrap_or(i64::MAX)
            .saturating_mul(HELD_BYTES_FACTOR)
    }
}

/// What a document is for.
enum Received {
    /// Print-Job: a new job.
    New { name: String, requested: Requested },
    /// Send-Document: the document of a Create-Job.
    Incoming(HeldJob),
}

/// The options a device asked for, fitted to the printer a job goes to directly: what the
/// printer cannot do is left to its default rather than refusing the job, since the device
/// asked a generic printer. Without the printer's capabilities, they go as asked, and the print
/// path checks them.
fn direct_choices(
    printer: &str,
    requested: &Requested,
    capabilities: Option<&Capabilities>,
) -> Choices {
    let sides = requested.sides.filter(|sides| {
        *sides == Sides::OneSided || capabilities.is_none_or(|c| c.sides.contains(sides))
    });
    let color_mode = requested.color_mode.filter(|mode: &ColorMode| {
        capabilities.is_none_or(|c| c.color_modes.is_empty() || c.color_modes.contains(mode))
    });
    let copies = requested
        .copies
        .map(|copies| capabilities.map_or(copies, |c| copies.min(c.max_copies.max(1))));
    Choices {
        printer: printer.to_owned(),
        copies: copies.map(|copies| copies.to_string()),
        sides: sides.map(|sides| sides.keyword().to_owned()),
        color_mode: color_mode.map(|mode| mode.keyword().to_owned()),
        page_ranges: requested.page_ranges.as_ref().map(ToString::to_string),
    }
}

fn requested_json(requested: &Requested) -> String {
    serde_json::to_string(requested).unwrap_or_else(|_| "{}".to_owned())
}

/// A job's IPP id: an IPP `integer` is 32 bits, and ids are never reused, so past that recto
/// has no id to give.
fn ipp_id(id: i64) -> recto_domain::Result<i32> {
    i32::try_from(id).map_err(|_| {
        Error::Database(sea_orm::DbErr::Custom(format!(
            "held job id {id} does not fit an IPP job id"
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_carry_their_port() {
        let address = |url: &str| printer_address(&Url::parse(url).unwrap());
        assert_eq!(
            address("https://print.example.org"),
            ("ipps://print.example.org:443".to_owned(), true)
        );
        assert_eq!(
            address("https://print.example.org:8443/"),
            ("ipps://print.example.org:8443".to_owned(), true)
        );
        assert_eq!(
            address("http://192.168.1.2:8080"),
            ("ipp://192.168.1.2:8080".to_owned(), false)
        );
        assert_eq!(
            address("https://[2001:db8::1]/"),
            ("ipps://[2001:db8::1]:443".to_owned(), true)
        );
    }

    #[test]
    fn raster_refusals_have_their_status() {
        assert_eq!(raster_status(RasterError::Truncated), 0x0411);
        assert_eq!(raster_status(RasterError::Malformed("run")), 0x0411);
        assert_eq!(raster_status(RasterError::Unsupported("16-bit")), 0x0411);
        for large in [
            RasterError::PageTooLarge,
            RasterError::TooManyPages,
            RasterError::DocumentTooLarge,
            RasterError::TooLarge,
            RasterError::TimedOut,
        ] {
            assert_eq!(raster_status(large), 0x0408, "{large:?}");
        }
        assert_eq!(raster_status(RasterError::Busy), 0x0507);
    }

    #[test]
    fn direct_options_fit_the_printer() {
        let requested = Requested {
            copies: Some(50),
            sides: Some(Sides::TwoSidedShortEdge),
            color_mode: Some(ColorMode::Color),
            page_ranges: Some("1-2, 5".parse().unwrap()),
            media: None,
        };
        let mono = Capabilities {
            max_copies: 10,
            sides: vec![Sides::OneSided],
            color_modes: vec![ColorMode::Monochrome],
        };
        let fitted = direct_choices("office", &requested, Some(&mono));
        assert_eq!(fitted.printer, "office");
        assert_eq!(fitted.copies.as_deref(), Some("10"));
        assert_eq!(fitted.sides, None);
        assert_eq!(fitted.color_mode, None);
        assert_eq!(fitted.page_ranges.as_deref(), Some("1-2, 5"));
        // Unknown capabilities: as asked, for the print path to check.
        let asked = direct_choices("office", &requested, None);
        assert_eq!(asked.copies.as_deref(), Some("50"));
        assert_eq!(asked.sides.as_deref(), Some("two-sided-short-edge"));
        assert_eq!(asked.color_mode.as_deref(), Some("color"));
    }

    #[test]
    fn job_segments_are_numbers() {
        assert!(job_segment("/7"));
        assert!(job_segment("/123"));
        for other in ["", "/", "7", "/x", "/7/8", "/-1"] {
            assert!(!job_segment(other), "{other}");
        }
    }
}
