//! recto's scanners for the scan apps of computers and phones, over eSCL (AirScan, Mopria):
//! `/eSCL/<scanner>/...` with a device password as the password of HTTP Basic, and
//! `/device/<secret>/eSCL/<scanner>/...`, the personal address, for apps that cannot answer a
//! password prompt (sane-airscan, macOS). `<scanner>` is [`escl::scanner_key`] of the SANE id, and
//! only the scanners the device password's owner may use exist.
//!
//! On the LAN listener, a client without credentials whose TCP peer address is in
//! `RECTO_ESCL_OPEN_NETWORKS` scans anonymously, on the scanners open to everyone only: scan apps
//! that find scanners over DNS-SD cannot carry a secret. Forwarded headers never count, and the
//! main listener, behind the reverse proxy, never allows it, since every client there comes from
//! the proxy's address.
//!
//! A scan runs in a task of its own through the queued [`recto_scanning::ScannerService`], and
//! its documents wait in [`EsclJobs`] for the app to fetch them; nothing is kept in recto. Every
//! scan is logged in the scan log with the `escl` source, without a user when anonymous.
//!
//! These addresses sit in front of the printer's ([`super::ipp`]), outside the session and
//! cross-site layers, and every other address under `/eSCL/` or `/device/` goes on to its bare
//! 404. Log lines name the device password's id, never the path, which may hold the secret.

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use recto_app::state::AppState;
use recto_domain::{
    access::{self, DeviceKind, Usable},
    device_passwords::DeviceUser,
    scans::{self, NewScan},
};
use recto_scanning::{
    OutputFormat, ScanError, ScannedPage, ScannerCapabilities, ScannerInfo,
    escl::{self, AdfState, EsclJobs, EsclRequest, NextDocument, Owner, SettingsError},
};
use uuid::Uuid;

use super::body;
use crate::{
    config::OpenNetworks,
    device_auth::{self, DeviceAuth, Listener},
    state::AuthState,
};

/// `Retry-After` while another eSCL job holds the scanner.
const BUSY_RETRY: &str = "5";
/// `Retry-After` while the job is still scanning.
const SCANNING_RETRY: &str = "2";

/// How long what a scanner said it can do is kept.
const CAPABILITIES_AGE: Duration = Duration::from_secs(10 * 60);

/// A scanner's capabilities, as SANE gave them.
struct Known {
    capabilities: ScannerCapabilities,
    at: Instant,
    /// The scanner list's [`listing`](recto_scanning::ScannerService::listing) then.
    listing: u64,
}

#[derive(Clone)]
struct EsclState {
    app: AppState,
    devices: DeviceAuth,
    jobs: Arc<EsclJobs>,
    /// Who may scan without credentials; none on the main listener.
    open_networks: OpenNetworks,
    /// recto's public URL, which the scanners' UUIDs derive from.
    public_url: Arc<str>,
    /// The scan page, `AdminURI` in the capabilities.
    admin: Arc<str>,
    /// The brand's 512 px icon, public: apps fetch it without credentials.
    icon: Arc<str>,
    /// What each scanner, by key, said it can do. SANE has a single thread, busy for the
    /// whole of a scan: asking it again would keep apps waiting, so the status, the
    /// capabilities and new jobs read this, until [`CAPABILITIES_AGE`] or a new listing.
    capabilities: Arc<Mutex<HashMap<String, Known>>>,
    /// How long `NextDocument` waits for a scan (`AuthState::escl_document_wait`).
    document_wait: Duration,
    /// How long a client has to send its `ScanSettings`, at most [`escl::MAX_SETTINGS_BYTES`]
    /// (`AuthState::device_read_timeout`).
    read_timeout: Duration,
}

/// The eSCL addresses in front of `devices`, the router of every other device address.
pub fn routes(devices: Router, app: AppState, auth: &AuthState, listener: Listener) -> Router {
    let open_networks = match listener {
        Listener::Main => OpenNetworks::default(),
        Listener::Lan => auth.escl_open_networks.clone(),
    };
    let state = EsclState {
        devices: DeviceAuth::for_listener(auth, &app.branding.name, listener),
        jobs: Arc::clone(&auth.escl_jobs),
        open_networks,
        public_url: Arc::from(auth.public_url.as_str()),
        admin: Arc::from(auth.public("scan").as_str()),
        icon: Arc::from(auth.public("brand/icon-512.png").as_str()),
        capabilities: Arc::default(),
        read_timeout: auth.device_read_timeout,
        document_wait: auth.escl_document_wait,
        app,
    };
    devices.layer(middleware::from_fn_with_state(state, intercept))
}

/// An eSCL address: `/eSCL/<key>/<rest>` or `/device/<secret>/eSCL/<key>/<rest>`.
struct Target {
    secret: Option<String>,
    key: String,
    rest: String,
}

impl Target {
    fn parse(path: &str) -> Option<Self> {
        let (secret, scanner) = match path.strip_prefix("/eSCL/") {
            Some(scanner) => (None, scanner),
            None => {
                let (secret, rest) = path
                    .strip_prefix(device_auth::PERSONAL_PREFIX)?
                    .split_once('/')?;
                (Some(secret), rest.strip_prefix("eSCL/")?)
            }
        };
        let (key, rest) = scanner.split_once('/')?;
        if key.is_empty() || secret.is_some_and(str::is_empty) {
            return None;
        }
        Some(Self {
            secret: secret.map(str::to_owned),
            key: key.to_owned(),
            rest: rest.trim_end_matches('/').to_owned(),
        })
    }

    /// The scanner's root on the address the request came in on: a personal address keeps its
    /// secret, a Basic one never gains it.
    fn root(&self) -> String {
        match &self.secret {
            Some(secret) => device_auth::personal_scanner_path(secret, &self.key),
            None => format!("/eSCL/{}", self.key),
        }
    }
}

/// What an eSCL request asks for.
enum Resource {
    Capabilities,
    Status,
    NewJob,
    NextDocument(Uuid),
    ImageInfo(Uuid),
    DeleteJob(Uuid),
}

impl Resource {
    fn parse(method: &Method, rest: &str) -> Option<Self> {
        let segments: Vec<&str> = rest.split('/').collect();
        let job = |id: &str| Uuid::parse_str(id).ok();
        match (method, segments.as_slice()) {
            (&Method::GET, ["ScannerCapabilities"]) => Some(Self::Capabilities),
            (&Method::GET, ["ScannerStatus"]) => Some(Self::Status),
            (&Method::POST, ["ScanJobs"]) => Some(Self::NewJob),
            (&Method::GET, ["ScanJobs", id, "NextDocument"]) => job(id).map(Self::NextDocument),
            (&Method::GET, ["ScanJobs", id, "ScanImageInfo"]) => job(id).map(Self::ImageInfo),
            (&Method::DELETE, ["ScanJobs", id]) => job(id).map(Self::DeleteJob),
            _ => None,
        }
    }
}

/// Who is scanning.
enum Caller {
    Device(DeviceUser),
    /// A client of an open network, without credentials, and its address.
    Anonymous(IpAddr),
}

impl Caller {
    /// The owner of the caller's jobs.
    fn owner(&self) -> Owner {
        match self {
            Self::Device(user) => Owner::User(user.user_id),
            Self::Anonymous(address) => Owner::Anonymous(*address),
        }
    }

    /// The user the scan log names, none for an anonymous client.
    fn user_id(&self) -> Option<i64> {
        match self {
            Self::Device(user) => Some(user.user_id),
            Self::Anonymous(_) => None,
        }
    }

    /// The name in log lines.
    fn name(&self) -> &str {
        match self {
            Self::Device(user) => &user.username,
            Self::Anonymous(_) => "anonymous",
        }
    }

    fn device_password_id(&self) -> Option<i64> {
        match self {
            Self::Device(user) => Some(user.device_password_id),
            Self::Anonymous(_) => None,
        }
    }
}

/// Answers eSCL addresses, and passes every other request on. Every answer carries
/// `Cache-Control: no-cache`, unless it says otherwise.
async fn intercept(State(state): State<EsclState>, request: Request, next: Next) -> Response {
    let Some(target) = Target::parse(request.uri().path()) else {
        return next.run(request).await;
    };
    let mut response = if state.app.os_integration {
        serve(&state, &target, request).await
    } else {
        StatusCode::NOT_FOUND.into_response()
    };
    response
        .headers_mut()
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-cache"));
    response
}

/// Authenticates the caller, finds the scanner, then answers.
async fn serve(state: &EsclState, target: &Target, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let caller = match caller(state, &parts, target.secret.as_deref()).await {
        Ok(caller) => caller,
        Err(refusal) => return state.devices.refuse(refusal),
    };
    let Some(resource) = Resource::parse(&parts.method, &target.rest) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let scanner = match scanner(state, &caller, &target.key).await {
        Ok(Some(scanner)) => scanner,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(status) => return status.into_response(),
    };
    let request = Scan {
        state,
        caller: &caller,
        scanner: &scanner,
        key: &target.key,
        root: target.root(),
    };
    match resource {
        Resource::Capabilities => request.capabilities().await,
        Resource::Status => request.status(),
        Resource::NewJob => request.new_job(&parts, body).await,
        Resource::NextDocument(id) => request.next_document(id).await,
        Resource::ImageInfo(id) => request.image_info(id),
        Resource::DeleteJob(id) => request.delete(id),
    }
}

/// The device password's owner, or an anonymous client of an open network; else the refusal.
/// A personal address always needs its secret, and credentials sent are always checked.
async fn caller(
    state: &EsclState,
    parts: &Parts,
    secret: Option<&str>,
) -> Result<Caller, device_auth::Refusal> {
    let peer = device_auth::peer(parts);
    let anonymous = secret.is_none()
        && !parts.headers.contains_key(header::AUTHORIZATION)
        && state.open_networks.contains(peer);
    if anonymous {
        return Ok(Caller::Anonymous(peer));
    }
    device_auth::authenticate(&state.devices, parts, secret)
        .await
        .map(Caller::Device)
}

/// The listed scanner of `key`, if the caller may use it. Only listed scanners are opened: SANE's
/// `net` backend would connect to whatever host an id names.
async fn scanner(
    state: &EsclState,
    caller: &Caller,
    key: &str,
) -> Result<Option<ScannerInfo>, StatusCode> {
    let scanners = state.app.scan.scanners(false).await.map_err(|error| {
        tracing::warn!(%error, "listing the scanners for eSCL failed");
        StatusCode::SERVICE_UNAVAILABLE
    })?;
    let Some(scanner) = scanners
        .into_iter()
        .find(|scanner| escl::scanner_key(&scanner.id) == key)
    else {
        return Ok(None);
    };
    let database = &state.app.database;
    let allowed = match caller {
        Caller::Device(user) => Usable::read(
            database,
            DeviceKind::Scanner,
            user.user_id,
            user.is_administrator,
        )
        .await
        .map(|usable| usable.allows(&scanner.id)),
        Caller::Anonymous(_) => access::rules(database, DeviceKind::Scanner)
            .await
            .map(|rules| access::allows(rules.get(&scanner.id), false, &[])),
    };
    match allowed {
        Ok(true) => Ok(Some(scanner)),
        Ok(false) => Ok(None),
        Err(error) => {
            tracing::error!(%error, "reading the scanner access rules failed");
            Err(StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

/// One request on a scanner the caller may use.
struct Scan<'a> {
    state: &'a EsclState,
    caller: &'a Caller,
    scanner: &'a ScannerInfo,
    key: &'a str,
    /// The scanner's root on the caller's address.
    root: String,
}

impl Scan<'_> {
    async fn capabilities(&self) -> Response {
        let capabilities = match self.known().await {
            Ok(capabilities) => capabilities,
            Err(error) => return self.failed(&error),
        };
        let uuid = escl::scanner_uuid(&self.state.public_url, &self.scanner.id);
        xml(escl::capabilities_xml(
            self.scanner,
            &capabilities,
            uuid,
            Some(&self.state.icon),
            &self.state.admin,
        ))
    }

    fn status(&self) -> Response {
        let jobs = &self.state.jobs;
        let adf = jobs.adf_state(self.key);
        let has_feeder = self
            .state
            .capabilities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(self.key)
            .is_some_and(|known| {
                known
                    .capabilities
                    .sources
                    .iter()
                    .any(|source| source.is_feeder)
            });
        // A feeder that was found empty, jammed or open is a feeder, whatever the cache says.
        let adf = (has_feeder || adf != AdfState::Loaded).then_some(adf);
        xml(escl::status_xml(
            jobs.state(self.key),
            adf,
            &self.root,
            &jobs.jobs(self.caller.owner(), self.key),
        ))
    }

    /// `POST ScanJobs`: reads the settings, starts the scan and answers with the job's address.
    async fn new_job(&self, parts: &Parts, body: Body) -> Response {
        if body::announced_over(&parts.headers, escl::MAX_SETTINGS_BYTES) {
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        }
        // All of it within the timeout: the settings are small, so a client that needs longer
        // stalls.
        let read = body::read_all(body, escl::MAX_SETTINGS_BYTES, self.state.read_timeout);
        let settings = match read.await {
            Ok(settings) => settings,
            Err(unread) => return unread.into_response(),
        };
        // Before SANE is asked anything: it would answer only once the running scan is over.
        if self.state.jobs.state(self.key) == escl::EsclState::Processing {
            return busy();
        }
        let capabilities = match self.known().await {
            Ok(capabilities) => capabilities,
            Err(error) => return self.failed(&error),
        };
        let request = match escl::parse_settings(&settings, &capabilities) {
            Ok(request) => request,
            Err(error) => {
                tracing::info!(%error, user = self.caller.name(), "eSCL scan settings refused");
                return match error {
                    SettingsError::Malformed(_) => StatusCode::BAD_REQUEST,
                    SettingsError::Unsupported(_) | SettingsError::TooLarge => StatusCode::CONFLICT,
                }
                .into_response();
            }
        };
        let Some(id) = self
            .state
            .jobs
            .start(self.caller.owner(), self.key, request.format)
        else {
            return busy();
        };
        tracing::info!(
            user = self.caller.name(),
            device_password_id = self.caller.device_password_id(),
            peer = %device_auth::peer(parts),
            scanner = self.key,
            format = request.format.extension(),
            "escl scan started"
        );
        tokio::spawn(run_scan(
            self.state.clone(),
            Running::new(Arc::clone(&self.state.jobs), id),
            ScanLog {
                user_id: self.caller.user_id(),
                username: match self.caller {
                    Caller::Device(user) => user.username.clone(),
                    Caller::Anonymous(_) => String::new(),
                },
                scanner: self.scanner.id.clone(),
                key: self.key.to_owned(),
            },
            request,
        ));
        let location = escl::job_uri(&self.root, id);
        match HeaderValue::from_str(&location) {
            Ok(location) => (StatusCode::CREATED, [(header::LOCATION, location)]).into_response(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }

    async fn next_document(&self, id: Uuid) -> Response {
        let next = self
            .state
            .jobs
            .next_document(self.caller.owner(), id, self.state.document_wait)
            .await;
        match next {
            NextDocument::Document(document) => (
                [
                    (
                        header::CONTENT_TYPE,
                        HeaderValue::from_static(document.mime),
                    ),
                    // A scan is the person's document: nothing on the way keeps it.
                    (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
                ],
                document.bytes,
            )
                .into_response(),
            // A 404 is how eSCL ends a job, and how sane-airscan learns the feeder is empty.
            NextDocument::Done | NextDocument::NoDocuments | NextDocument::NotFound => {
                StatusCode::NOT_FOUND.into_response()
            }
            NextDocument::NotYet => (
                StatusCode::SERVICE_UNAVAILABLE,
                [(
                    header::RETRY_AFTER,
                    HeaderValue::from_static(SCANNING_RETRY),
                )],
            )
                .into_response(),
            // The scanner was taken by something else: over for this job, whose state says
            // Aborted, never 503, which the app would retry forever.
            NextDocument::Failed(ScanError::Busy) => StatusCode::CONFLICT.into_response(),
            NextDocument::Failed(error) => self.failed(&error),
        }
    }

    fn image_info(&self, id: Uuid) -> Response {
        match self.state.jobs.image_info(self.caller.owner(), id) {
            Some(info) => xml(escl::scan_image_info_xml(&self.root, id, &info)),
            None => StatusCode::NOT_FOUND.into_response(),
        }
    }

    fn delete(&self, id: Uuid) -> Response {
        if self.state.jobs.cancel(self.caller.owner(), id) {
            StatusCode::OK.into_response()
        } else {
            StatusCode::NOT_FOUND.into_response()
        }
    }

    /// The scanner's capabilities, from what it said before when that is recent enough.
    async fn known(&self) -> Result<ScannerCapabilities, ScanError> {
        let scan = &self.state.app.scan;
        let listing = scan.listing();
        // While one of its jobs scans, asking SANE would wait behind the whole batch, past the
        // device's timeout: what is known, however old, is better.
        let scanning = self.state.jobs.state(self.key) == escl::EsclState::Processing;
        let cached = self
            .state
            .capabilities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(self.key)
            .filter(|known| {
                scanning || (known.listing == listing && known.at.elapsed() < CAPABILITIES_AGE)
            })
            .map(|known| known.capabilities.clone());
        if let Some(capabilities) = cached {
            return Ok(capabilities);
        }
        let capabilities = scan.capabilities(&self.scanner.id).await?;
        self.state
            .capabilities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                self.key.to_owned(),
                Known {
                    capabilities: capabilities.clone(),
                    at: Instant::now(),
                    listing,
                },
            );
        Ok(capabilities)
    }

    /// The answer to a scanner that failed: 409 for what the person can fix at the scanner, 503
    /// while it is busy, 404 for a scanner that went away or an empty feeder, else 500.
    fn failed(&self, error: &ScanError) -> Response {
        tracing::warn!(%error, user = self.caller.name(), scanner = self.key, "eSCL scanning failed");
        match error {
            ScanError::Jammed
            | ScanError::CoverOpen
            | ScanError::Unsupported(_)
            | ScanError::TooLarge => StatusCode::CONFLICT.into_response(),
            ScanError::Busy => (
                StatusCode::SERVICE_UNAVAILABLE,
                [(
                    header::RETRY_AFTER,
                    HeaderValue::from_static(SCANNING_RETRY),
                )],
            )
                .into_response(),
            ScanError::NotFound(_) | ScanError::NoDocuments => {
                StatusCode::NOT_FOUND.into_response()
            }
            ScanError::Device(_)
            | ScanError::Interrupted
            | ScanError::Encoding(_)
            | ScanError::Storage { .. } => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}

/// The answer while another eSCL job holds the scanner.
fn busy() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::RETRY_AFTER, HeaderValue::from_static(BUSY_RETRY))],
    )
        .into_response()
}

/// An eSCL XML answer.
fn xml(body: String) -> Response {
    (
        [(header::CONTENT_TYPE, HeaderValue::from_static("text/xml"))],
        body,
    )
        .into_response()
}

/// What the scan log needs of a job.
struct ScanLog {
    user_id: Option<i64>,
    /// Empty when anonymous.
    username: String,
    /// The SANE id.
    scanner: String,
    key: String,
}

/// A job whose scan runs: finished with whatever the scan gave, or, if the task stops before
/// (a panic, the runtime shutting down), as interrupted, so the scanner does not stay busy.
struct Running {
    jobs: Arc<EsclJobs>,
    id: Uuid,
    finished: bool,
}

impl Running {
    fn new(jobs: Arc<EsclJobs>, id: Uuid) -> Self {
        Self {
            jobs,
            id,
            finished: false,
        }
    }

    fn finish(mut self, result: Result<Vec<ScannedPage>, ScanError>) {
        self.finished = true;
        self.jobs.finish(self.id, result);
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if !self.finished {
            self.jobs.finish(self.id, Err(ScanError::Interrupted));
        }
    }
}

/// Scans, logs the scan, then hands the pages to the job. The log comes first, so whatever reads
/// it after the app has its documents sees the scan.
async fn run_scan(state: EsclState, running: Running, log: ScanLog, request: EsclRequest) {
    let format = request.format;
    let result = state
        .app
        .scan
        .scan_every_page(&log.scanner, request.scan_request())
        .await;
    let user = if log.user_id.is_some() {
        log.username.as_str()
    } else {
        "anonymous"
    };
    match &result {
        Ok(pages) => {
            tracing::info!(
                user,
                scanner = log.key,
                pages = pages.len(),
                format = format.extension(),
                "escl scan finished"
            );
            record(&state, &log, format, pages.len()).await;
        }
        Err(error) => {
            tracing::warn!(%error, user, scanner = log.key, "escl scan failed");
        }
    }
    // Writing a PDF of a long batch takes a moment.
    let finished = tokio::task::spawn_blocking(move || running.finish(result)).await;
    if let Err(error) = finished {
        tracing::error!(%error, "handing an eSCL scan to its job failed");
    }
}

/// Logs a finished scan for the usage reports, waiting at most [`scans::LOG_WAIT`]: the app still
/// gets its documents when the log cannot be written.
async fn record(state: &EsclState, log: &ScanLog, format: OutputFormat, pages: usize) {
    let scan = NewScan {
        user_id: log.user_id,
        username: log.username.clone(),
        scanner: log.scanner.clone(),
        result_id: None,
        source: scans::Source::Escl,
        format: format.extension().to_owned(),
        pages: u32::try_from(pages).unwrap_or(u32::MAX),
    };
    let recorded = scans::record(&state.app.database, scan, chrono::Utc::now());
    match tokio::time::timeout(scans::LOG_WAIT, recorded).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            tracing::error!(%error, scanner = log.key, "recording the eSCL scan failed")
        }
        Err(_) => tracing::warn!(scanner = log.key, "recording the eSCL scan timed out"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_read_by_hand() {
        let basic = Target::parse("/eSCL/1a2b3c4d/ScanJobs/").unwrap();
        assert_eq!(
            (basic.secret, basic.key.as_str(), basic.rest.as_str()),
            (None, "1a2b3c4d", "ScanJobs")
        );
        let personal = Target::parse("/device/s3cret/eSCL/1a2b3c4d/ScannerStatus").unwrap();
        assert_eq!(personal.secret.as_deref(), Some("s3cret"));
        assert_eq!(personal.root(), "/device/s3cret/eSCL/1a2b3c4d");
        for other in [
            "/device/s3cret/ipp/print",
            "/device//eSCL/1a2b3c4d/ScannerStatus",
            "/eSCL//ScannerStatus",
            "/eSCL/1a2b3c4d",
            "/ipp/print",
        ] {
            assert!(Target::parse(other).is_none(), "{other}");
        }
    }

    #[test]
    fn resources_need_their_method() {
        let id = "6f1c9a52-4e0b-4c55-9a3e-2d1f0b7c8e91";
        assert!(matches!(
            Resource::parse(&Method::GET, &format!("ScanJobs/{id}/NextDocument")),
            Some(Resource::NextDocument(_))
        ));
        assert!(Resource::parse(&Method::POST, "ScannerCapabilities").is_none());
        assert!(Resource::parse(&Method::GET, "ScanJobs").is_none());
        assert!(Resource::parse(&Method::DELETE, "ScanJobs/1").is_none());
    }
}
