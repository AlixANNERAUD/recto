//! The routes of the server, driven through the router with mock printing and scanning services
//! and a fresh test database. Users are created with the provider callback's code and signed in
//! by writing their session straight into the router's session store; no route of the server can
//! do that.

use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Request, Response, StatusCode, header},
};
use leptos::config::LeptosOptions;
use recto::session::SignInMethod;
use recto::session_store::DatabaseSessionStore;
use recto::{
    branding::{BrandFile, Branding, default_branding},
    serve::{AuthState, router},
};
use recto_app::state::{AppState, JobVisibility};
use recto_authentication::{OpenId, OpenIdSettings};
use recto_domain::{
    access::{self, Access, DeviceKind},
    jobs,
    passwords::{self, PasswordPolicy},
    testing::{TempDir, temporary_directory},
    users::{self, Identity},
};
use recto_entity::{groups, print_jobs, sessions};
use recto_printing::{
    Capabilities, ColorMode, DocumentFormat, Job, JobState, PageRanges, PrintOptions, Printer,
    PrinterState, Sides,
    testing::{AdminCall, MockPrintService, MockPrinterAdmin},
};
use recto_scanning::{MockScannerService, OutputFormat, ResultStore};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
use tower::ServiceExt;
use tower_sessions::{Session, SessionStore, cookie::time::OffsetDateTime, session::Id};
use url::Url;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

const PUBLIC_URL: &str = "http://recto.test/";
const ORIGIN: &str = "http://recto.test";
/// Nothing listens there, so the identity provider is down.
const UNREACHABLE_ISSUER: &str = "http://127.0.0.1:9/";
const PDF: &[u8] = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\n%%EOF\n";
const PNG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
/// A file of the site root, which the fallback serves to anyone.
const STYLESHEET: &str = "body{}";

fn office() -> Printer {
    Printer {
        name: "office".into(),
        description: None,
        location: None,
        make_and_model: None,
        is_default: true,
        state: PrinterState::Idle,
        reasons: vec![],
        accepting_jobs: true,
        supplies: vec![],
        capabilities: Capabilities {
            max_copies: 10,
            sides: vec![Sides::OneSided, Sides::TwoSidedLongEdge],
            color_modes: vec![ColorMode::Color, ColorMode::Monochrome],
        },
    }
}

struct Setup {
    max_upload_size: usize,
    output_dir: Option<PathBuf>,
    result_ttl: Duration,
    /// `None` for no OpenID Connect provider.
    issuer: Option<String>,
    public_url: &'static str,
    /// The database of another harness, to restart on; a fresh one by default.
    database: Option<DatabaseConnection>,
    local_accounts: bool,
    trust_forwarded_headers: bool,
    branding: Branding,
    job_visibility: JobVisibility,
    /// The jobs CUPS holds; one of bob's by default.
    jobs: Option<Vec<Job>>,
    /// The printers CUPS holds; the office printer by default.
    printers: Option<Vec<Printer>>,
    /// Whether recto can administer CUPS: `Harness::admin` records what it was asked.
    managed: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            max_upload_size: 1024 * 1024,
            output_dir: None,
            result_ttl: Duration::from_secs(3600),
            issuer: Some(UNREACHABLE_ISSUER.into()),
            public_url: PUBLIC_URL,
            database: None,
            local_accounts: true,
            trust_forwarded_headers: false,
            branding: default_branding(),
            job_visibility: JobVisibility::All,
            jobs: None,
            printers: None,
            managed: false,
        }
    }
}

struct Harness {
    router: Router,
    print: Arc<MockPrintService>,
    admin: Arc<MockPrinterAdmin>,
    scan: Arc<MockScannerService>,
    results: Arc<ResultStore>,
    sessions: DatabaseSessionStore,
    database: DatabaseConnection,
    _dir: TempDir,
}

impl Harness {
    async fn new() -> Self {
        Self::with(Setup::default()).await
    }

    async fn with(setup: Setup) -> Self {
        let database = match setup.database {
            Some(database) => database,
            None => recto_domain::testing::database().await,
        };
        let dir = temporary_directory();
        let print = Arc::new(
            MockPrintService::new()
                .with_printers(setup.printers.unwrap_or_else(|| vec![office()]))
                .with_jobs(setup.jobs.unwrap_or_else(|| {
                    vec![Job {
                        id: 7,
                        printer: "office".into(),
                        name: Some("minutes.pdf".into()),
                        owner: Some("bob".into()),
                        state: JobState::Processing,
                        pages: None,
                        created: None,
                    }]
                })),
        );
        let scan = Arc::new(MockScannerService::default());
        let results = Arc::new(ResultStore::new(
            dir.path().join("results"),
            setup.result_ttl,
        ));
        let site = dir.path().join("site");
        std::fs::create_dir_all(site.join("pkg")).unwrap();
        std::fs::write(site.join("pkg/recto.css"), STYLESHEET).unwrap();
        let admin = Arc::new(MockPrinterAdmin::new());
        let state = AppState {
            printer_admin: setup
                .managed
                .then(|| admin.clone() as Arc<dyn recto_printing::PrinterAdmin>),
            print: print.clone(),
            scan: scan.clone(),
            results: results.clone(),
            output_dir: setup.output_dir,
            max_upload_size: setup.max_upload_size,
            leptos_options: LeptosOptions::builder()
                .output_name("recto")
                .site_root(site.to_string_lossy().into_owned())
                .build(),
            database: database.clone(),
            branding: Arc::new(setup.branding),
            job_visibility: setup.job_visibility,
        };
        let public_url = Url::parse(setup.public_url).unwrap();
        let openid = setup.issuer.map(|issuer| {
            let openid = OpenId::new(OpenIdSettings {
                issuer: Url::parse(&issuer).unwrap(),
                client_id: "recto".into(),
                redirect_url: public_url.join("auth/callback").unwrap(),
                extra_scopes: vec![],
                required_claim: None,
                required_value: None,
                administrator_claim: None,
                administrator_value: None,
                groups_claim: None,
            })
            .retry_after(Duration::ZERO);
            Arc::new(openid)
        });
        let auth = AuthState {
            local_accounts: setup.local_accounts,
            trust_forwarded_headers: setup.trust_forwarded_headers,
            ..AuthState::new(openid, public_url, database.clone())
        };
        let sessions = auth.sessions.clone();
        Self {
            router: router(state, auth),
            print,
            admin,
            scan,
            results,
            sessions,
            database,
            _dir: dir,
        }
    }

    async fn send(&self, request: Request<Body>) -> Response<Body> {
        self.router
            .clone()
            .oneshot(from_peer(request))
            .await
            .unwrap()
    }

    /// Creates a local user with a password.
    async fn local_user(&self, username: &str, password: &str) -> i64 {
        let user = users::create_local(
            &self.database,
            users::NewLocalUser {
                username: username.into(),
                display_name: String::new(),
                email: None,
                password: password.into(),
                is_administrator: false,
            },
            &PasswordPolicy { minimum_length: 12 },
        )
        .await
        .unwrap();
        user.id
    }

    /// Signs in through the password form and returns the session cookie it sets.
    async fn sign_in_with_password(&self, login: &str, password: &str) -> String {
        let response = self
            .send(form(
                "/sign-in",
                None,
                &[("login", login), ("password", password)],
            ))
            .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        session_cookie(response.headers()).expect("a session cookie")
    }

    /// Signs a user in, creating it at its first sign-in as the provider's callback does, and
    /// returns the session cookie to send back.
    async fn sign_in(&self, subject: &str, username: &str) -> String {
        let identity = Identity {
            issuer: "https://id.test/".into(),
            subject: subject.into(),
            preferred_username: Some(username.into()),
            ..Identity::default()
        };
        let user = users::sign_in_with_identity(&self.database, &identity)
            .await
            .unwrap();
        let session = Session::new(None, Arc::new(self.sessions.clone()), None);
        let method = SignInMethod::SingleSignOn {
            id_token: "token".into(),
        };
        recto::session::sign_in(&session, user.id, method)
            .await
            .unwrap();
        session.save().await.unwrap();
        format!("recto_session={}", session.id().unwrap())
    }

    /// The id of a user, which owns the user's scan results.
    async fn user_id(&self, username: &str) -> i64 {
        users::find_by_username(&self.database, username)
            .await
            .unwrap()
            .expect("a signed-in user")
            .id
    }

    /// How many sessions the database keeps.
    async fn session_count(&self) -> u64 {
        sessions::Entity::find()
            .count(&self.database)
            .await
            .unwrap()
    }
}

/// Where test requests come from, as the server's connection info would say; set unless the
/// request already names its peer.
const PEER: [u8; 4] = [192, 0, 2, 1];

fn from_peer(mut request: Request<Body>) -> Request<Body> {
    if request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_none()
    {
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((PEER, 50_000))));
    }
    request
}

/// The request, as sent from `address`.
fn from_address(mut request: Request<Body>, address: [u8; 4]) -> Request<Body> {
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from((address, 50_000))));
    request
}

/// A URL-encoded form post from recto's own pages, with or without a session cookie.
fn form(uri: &str, cookie: Option<&str>, fields: &[(&str, &str)]) -> Request<Body> {
    let body: String = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish();
    let mut request = Request::post(uri)
        .header(header::ORIGIN, ORIGIN)
        .header("sec-fetch-site", "same-origin")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    request.body(Body::from(body)).unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

fn page(uri: &str, cookie: Option<&str>) -> Request<Body> {
    let mut request = Request::get(uri).header(header::ACCEPT, "text/html");
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    request.body(Body::empty()).unwrap()
}

/// A form post from recto's own pages.
fn post(uri: &str, cookie: &str, content_type: Option<&str>, body: Vec<u8>) -> Request<Body> {
    let mut request = Request::post(uri)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, ORIGIN)
        .header("sec-fetch-site", "same-origin");
    if let Some(content_type) = content_type {
        request = request.header(header::CONTENT_TYPE, content_type);
    }
    request.body(Body::from(body)).unwrap()
}

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("recto_session="))
        .map(|value| value.split(';').next().unwrap().to_owned())
}

/// The whole `Set-Cookie` header of the session cookie, attributes included.
fn set_cookie(headers: &HeaderMap) -> String {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("recto_session="))
        .expect("a session cookie")
        .to_owned()
}

fn location(response: &Response<Body>) -> String {
    response
        .headers()
        .get(header::LOCATION)
        .expect("a Location header")
        .to_str()
        .unwrap()
        .to_owned()
}

async fn text(response: Response<Body>) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

const BOUNDARY: &str = "recto-test-boundary";

/// A `multipart/form-data` body with text fields and an optional file named `file`.
fn multipart(fields: &[(&str, &str)], file: Option<(&str, &[u8])>) -> (String, Vec<u8>) {
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    if let Some((file_name, bytes)) = file {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; \
                 filename=\"{file_name}\"\r\nContent-Type: application/pdf\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={BOUNDARY}"), body)
}

async fn print(
    harness: &Harness,
    cookie: &str,
    fields: &[(&str, &str)],
    file: Option<(&str, &[u8])>,
) -> Response<Body> {
    let (content_type, body) = multipart(fields, file);
    harness
        .send(post("/print", cookie, Some(&content_type), body))
        .await
}

fn assert_redirect(response: &Response<Body>, expected: &str) {
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(response), expected);
}

#[tokio::test]
async fn health_is_public() {
    let response = Harness::new().await.send(get("/health")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(text(response).await, "ok");
}

#[tokio::test]
async fn pages_redirect_to_sign_in() {
    let harness = Harness::new().await;
    let response = harness.send(page("/", None)).await;
    assert_redirect(&response, "/sign-in?next=%2F");

    // Anything that is not a page answers 401 rather than a redirect a script cannot follow.
    let (content_type, body) = multipart(&[("printer", "office")], Some(("a.pdf", PDF)));
    let response = harness
        .send(post(
            "/print",
            "recto_session=none",
            Some(&content_type),
            body,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = harness
        .send(get(&format!("/scan/results/{}", uuid::Uuid::new_v4())))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // The pages explaining a failed sign-in must show without a session.
    let response = harness.send(page("/no-access", None)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text(response).await.contains("No access to recto"));

    // Once signed in, the same page renders.
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let response = harness.send(page("/", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn anonymous_server_function_calls_get_401() {
    // Server functions sit behind the sign-in gate because `leptos_routes` registers them
    // inside it; this keeps that true through Leptos upgrades.
    let harness = Harness::new().await;
    for endpoint in ["list_jobs", "list_printers", "scan"] {
        let request = Request::post(format!("/api/{endpoint}"))
            .header(header::ORIGIN, ORIGIN)
            .header("sec-fetch-site", "same-origin")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::empty())
            .unwrap();
        let response = harness.send(request).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{endpoint}");
        // Encoded as a server function error, so islands can tell a lapsed session from a
        // failed request.
        let error: leptos::prelude::ServerFnError = leptos::server_fn::error::FromServerFnError::de(
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        );
        assert_eq!(
            error,
            leptos::prelude::ServerFnError::ServerError(recto_app::messages::SIGNED_OUT.into())
        );
    }
}

#[tokio::test]
async fn print_submits_with_user_and_options() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let response = print(
        &harness,
        &cookie,
        &[
            ("printer", "office"),
            ("copies", "2"),
            ("sides", "two-sided-long-edge"),
            ("color_mode", "monochrome"),
            ("page_ranges", "5, 1-3"),
        ],
        Some(("report.pdf", PDF)),
    )
    .await;
    assert_redirect(&response, "/jobs?submitted=8");

    let submitted = harness.print.submitted();
    assert_eq!(submitted.len(), 1);
    let request = &submitted[0];
    assert_eq!(request.printer, "office");
    assert_eq!(request.user, "alice");
    assert_eq!(request.job_name, "report.pdf");
    assert_eq!(request.format, DocumentFormat::Pdf);
    assert_eq!(&request.document[..], PDF);
    assert_eq!(
        request.options,
        PrintOptions {
            copies: 2,
            sides: Sides::TwoSidedLongEdge,
            color_mode: ColorMode::Monochrome,
            page_ranges: Some(PageRanges(vec![(1, 3), (5, 5)])),
        }
    );
}

#[tokio::test]
async fn a_submitted_job_is_recorded() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let response = print(
        &harness,
        &cookie,
        &[("printer", "office")],
        Some(("report.pdf", PDF)),
    )
    .await;
    assert_redirect(&response, "/jobs?submitted=8");

    let records = jobs::for_cups_ids(&harness.database, &[8]).await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].username, "alice");
    assert_eq!(records[0].printer, "office");
    assert_eq!(records[0].state, "pending");
    let rows = print_jobs::Entity::find()
        .all(&harness.database)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].document_name, "report.pdf");
}

#[tokio::test]
async fn a_job_cups_refuses_is_not_recorded() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    harness.print.set_unavailable("connection refused");
    let response = print(
        &harness,
        &cookie,
        &[("printer", "office")],
        Some(("report.pdf", PDF)),
    )
    .await;
    assert_redirect(&response, "/?error=unavailable");
    let count = print_jobs::Entity::find()
        .count(&harness.database)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn print_enforces_printer_capabilities() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    for (fields, error) in [
        (vec![("copies", "11")], "invalid_copies"),
        (vec![("copies", "0")], "invalid_copies"),
        (vec![("copies", "many")], "invalid_copies"),
        (vec![("sides", "two-sided-short-edge")], "unsupported_sides"),
        (vec![("color_mode", "sepia")], "unsupported_color"),
        (vec![("page_ranges", "3-1")], "invalid_page_ranges"),
    ] {
        let mut fields = fields;
        fields.push(("printer", "office"));
        let response = print(&harness, &cookie, &fields, Some(("report.pdf", PDF))).await;
        assert_redirect(&response, &format!("/?error={error}"));
    }
    let response = print(
        &harness,
        &cookie,
        &[("printer", "basement")],
        Some(("report.pdf", PDF)),
    )
    .await;
    assert_redirect(&response, "/?error=printer_not_found");
    let response = print(&harness, &cookie, &[("printer", "office")], None).await;
    assert_redirect(&response, "/?error=no_file");
    assert!(harness.print.submitted().is_empty());

    // Images are one page: a page range means nothing for them and is not checked.
    let response = print(
        &harness,
        &cookie,
        &[("printer", "office"), ("page_ranges", "3-1")],
        Some(("photo.png", PNG)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let submitted = harness.print.submitted();
    assert_eq!(submitted[0].format, DocumentFormat::Png);
    assert_eq!(submitted[0].options, PrintOptions::default());
}

#[tokio::test]
async fn print_refuses_renamed_docx() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let docx = b"PK\x03\x04\x14\x00\x06\x00word/document.xml";
    let response = print(
        &harness,
        &cookie,
        &[("printer", "office")],
        Some(("report.pdf", docx)),
    )
    .await;
    assert_redirect(&response, "/?error=unsupported_format");
    let response = print(
        &harness,
        &cookie,
        &[("printer", "office")],
        Some(("empty.pdf", b"")),
    )
    .await;
    assert_redirect(&response, "/?error=empty_file");
    assert!(harness.print.submitted().is_empty());

    let response = harness
        .send(page("/?error=unsupported_format", Some(&cookie)))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        text(response)
            .await
            .contains("Only PDF, JPEG and PNG files can be printed")
    );
}

#[tokio::test]
async fn print_refuses_oversized_upload() {
    let harness = Harness::with(Setup {
        max_upload_size: 4096,
        ..Setup::default()
    })
    .await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let mut large = PDF.to_vec();
    large.resize(64 * 1024, b' ');
    let response = print(
        &harness,
        &cookie,
        &[("printer", "office")],
        Some(("large.pdf", &large)),
    )
    .await;
    assert_redirect(&response, "/?error=too_large");
    assert!(harness.print.submitted().is_empty());

    // Just over the limit, within the slack allowed for the other form fields.
    large.truncate(4097);
    let response = print(
        &harness,
        &cookie,
        &[("printer", "office")],
        Some(("large.pdf", &large)),
    )
    .await;
    assert_redirect(&response, "/?error=too_large");

    let response = harness.send(page("/?error=too_large", Some(&cookie))).await;
    assert!(text(response).await.contains("The file is too large"));
}

#[tokio::test]
async fn print_shows_unavailable_cups() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    harness.print.set_unavailable("connection refused");

    let response = harness.send(page("/", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        text(response)
            .await
            .contains("Printing service unavailable: connection refused")
    );

    let response = print(
        &harness,
        &cookie,
        &[("printer", "office")],
        Some(("report.pdf", PDF)),
    )
    .await;
    assert_redirect(&response, "/?error=unavailable");
}

#[tokio::test]
async fn cancel_uses_signed_in_user() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let response = harness
        .send(post("/jobs/7/cancel", &cookie, None, vec![]))
        .await;
    assert_redirect(&response, "/jobs");
    assert_eq!(harness.print.canceled(), [(7, "alice".to_owned())]);

    let response = harness
        .send(post("/jobs/99/cancel", &cookie, None, vec![]))
        .await;
    assert_redirect(&response, "/jobs?error=job_not_found");
}

#[tokio::test]
async fn cross_site_post_refused() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let forged = Request::post("/jobs/7/cancel")
        .header(header::COOKIE, &cookie)
        .header(header::ORIGIN, "https://evil.example")
        .header("sec-fetch-site", "cross-site")
        .body(Body::empty())
        .unwrap();
    let response = harness.send(forged).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    // The likely cause is recto opened at another address than its public URL.
    let message = text(response).await;
    assert!(
        message.contains("Open recto at http://recto.test"),
        "{message}"
    );
    assert!(harness.print.canceled().is_empty());

    let (content_type, body) = multipart(&[("printer", "office")], Some(("a.pdf", PDF)));
    let forged = Request::post("/print")
        .header(header::COOKIE, &cookie)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ORIGIN, "https://evil.example")
        .body(Body::from(body))
        .unwrap();
    assert_eq!(harness.send(forged).await.status(), StatusCode::FORBIDDEN);
    assert!(harness.print.submitted().is_empty());
}

#[tokio::test]
async fn scan_result_download_and_owner_isolation() {
    let harness = Harness::new().await;
    let alice = harness.sign_in("alice-subject", "alice").await;
    let alice_id = harness.user_id("alice").await.to_string();
    let bob = harness.sign_in("bob-subject", "bob").await;
    let result = harness
        .results
        .put(&alice_id, OutputFormat::Pdf, 1, PDF)
        .unwrap();
    let preview = format!("/scan/results/{}", result.id);
    let download = format!("{preview}/download");

    let request = |uri: &str, cookie: &str| {
        Request::get(uri)
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap()
    };
    let response = harness.send(request(&preview, &alice)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/pdf");
    assert!(
        response.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("inline")
    );
    assert_eq!(text(response).await.as_bytes(), PDF);

    let response = harness.send(request(&download, &alice)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_DISPOSITION],
        format!("attachment; filename=\"{}\"", result.file_name).as_str()
    );
    assert_eq!(text(response).await.as_bytes(), PDF);

    // Another user's result looks exactly like a missing one.
    for uri in [&preview, &download] {
        let response = harness.send(request(uri, &bob)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
    }
    let response = harness
        .send(post(&format!("{preview}/save"), &bob, None, vec![]))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = harness
        .send(request(
            &format!("/scan/results/{}", uuid::Uuid::new_v4()),
            &alice,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = harness
        .send(request("/scan/results/nonsense", &alice))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn expired_scan_result_is_gone() {
    let harness = Harness::with(Setup {
        result_ttl: Duration::ZERO,
        ..Setup::default()
    })
    .await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let alice_id = harness.user_id("alice").await.to_string();
    let result = harness
        .results
        .put(&alice_id, OutputFormat::Png, 1, PNG)
        .unwrap();
    let response = harness
        .send(
            Request::get(format!("/scan/results/{}/download", result.id))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn save_writes_to_the_output_folder() {
    let output = temporary_directory();
    let harness = Harness::with(Setup {
        output_dir: Some(output.path().to_owned()),
        ..Setup::default()
    })
    .await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let alice_id = harness.user_id("alice").await.to_string();
    let result = harness
        .results
        .put(&alice_id, OutputFormat::Pdf, 1, PDF)
        .unwrap();
    let response = harness
        .send(post(
            &format!("/scan/results/{}/save", result.id),
            &cookie,
            None,
            vec![],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let answer: serde_json::Value = serde_json::from_str(&text(response).await).unwrap();
    let saved = PathBuf::from(answer["path"].as_str().unwrap());
    assert_eq!(saved, output.path().join(&result.file_name));
    assert_eq!(std::fs::read(saved).unwrap(), PDF);
}

#[tokio::test]
async fn save_reports_unwritable_folder() {
    let base = temporary_directory();
    let missing = base.path().join("missing");
    let harness = Harness::with(Setup {
        output_dir: Some(missing.clone()),
        ..Setup::default()
    })
    .await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let alice_id = harness.user_id("alice").await.to_string();
    let result = harness
        .results
        .put(&alice_id, OutputFormat::Pdf, 1, PDF)
        .unwrap();
    let response = harness
        .send(post(
            &format!("/scan/results/{}/save", result.id),
            &cookie,
            None,
            vec![],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let answer: serde_json::Value = serde_json::from_str(&text(response).await).unwrap();
    let error = answer["error"].as_str().unwrap();
    assert!(
        error.starts_with(&format!("Cannot write to {}", missing.display())),
        "{error}"
    );

    // The download keeps working when the folder does not.
    let response = harness
        .send(
            Request::get(format!("/scan/results/{}/download", result.id))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);

    // Without a configured folder, there is nothing to save to.
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let alice_id = harness.user_id("alice").await.to_string();
    let result = harness
        .results
        .put(&alice_id, OutputFormat::Pdf, 1, PDF)
        .unwrap();
    let response = harness
        .send(post(
            &format!("/scan/results/{}/save", result.id),
            &cookie,
            None,
            vec![],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn sign_in_with_unavailable_provider_offers_a_retry() {
    let harness = Harness::new().await;
    let response = harness.send(page("/auth/login?next=/jobs", None)).await;
    assert_redirect(&response, "/auth/error?reason=unavailable&next=%2Fjobs");

    let response = harness
        .send(page("/auth/error?reason=unavailable&next=%2Fjobs", None))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("Sign-in provider unavailable"), "{html}");
    assert!(html.contains("href=\"/sign-in?next=%2Fjobs\""), "{html}");

    // A path to another site is never kept as the place to come back to.
    let response = harness
        .send(page("/auth/login?next=//evil.example/x", None))
        .await;
    assert_redirect(&response, "/auth/error?reason=unavailable&next=%2F");
}

/// A provider that answers discovery, enough to start a sign-in.
async fn provider() -> MockServer {
    let server = MockServer::start().await;
    let base = server.uri();
    let document = serde_json::json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/authorize"),
        "token_endpoint": format!("{base}/token"),
        "end_session_endpoint": format!("{base}/logout"),
        "jwks_uri": format!("{base}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"]
    });
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(document))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"keys": []})))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn sign_in_state_is_single_use() {
    let provider = provider().await;
    let harness = Harness::with(Setup {
        issuer: Some(provider.uri()),
        ..Setup::default()
    })
    .await;
    let response = harness.send(page("/auth/login?next=/jobs", None)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let authorize = Url::parse(&location(&response)).unwrap();
    assert!(
        authorize
            .as_str()
            .starts_with(&format!("{}/authorize", provider.uri()))
    );
    let state = authorize
        .query_pairs()
        .find(|(name, _)| name == "state")
        .unwrap()
        .1
        .into_owned();
    let cookie = session_cookie(response.headers()).expect("a session cookie");
    // A sign-in in progress lives for the provider's round trip, not for 12 hours.
    assert!(
        set_cookie(response.headers()).contains("Max-Age=600"),
        "{}",
        set_cookie(response.headers())
    );
    let id: Id = cookie.trim_start_matches("recto_session=").parse().unwrap();
    let record = harness.sessions.load(&id).await.unwrap().unwrap();
    let left = record.expiry_date - OffsetDateTime::now_utc();
    assert!(
        left.whole_seconds() <= 600 && left.whole_seconds() > 590,
        "{left}"
    );

    let response = harness
        .send(page("/auth/callback?code=c&state=forged", Some(&cookie)))
        .await;
    assert_redirect(&response, "/auth/error?reason=state&next=%2Fjobs");

    // The failed attempt used up the pending sign-in: the right state no longer works either.
    let response = harness
        .send(page(
            &format!("/auth/callback?code=c&state={state}"),
            Some(&cookie),
        ))
        .await;
    assert_redirect(&response, "/auth/error?reason=state&next=%2F");
}

#[tokio::test]
async fn sign_out_ends_the_session() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let response = harness
        .send(post("/auth/logout", &cookie, None, vec![]))
        .await;
    // The provider was never discovered, so it has no end-session address to go to.
    assert_redirect(&response, "/auth/signed-out");
    let response = harness.send(page("/", Some(&cookie))).await;
    assert_redirect(&response, "/sign-in?next=%2F");
    let response = harness.send(page("/auth/signed-out", None)).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn sign_out_without_an_id_token_still_ends_the_provider_session() {
    // On /no-access the provider signed the user in, but recto kept no session and no ID
    // token: signing out must still reach the provider, or the next sign-in is the same
    // refused account again.
    let provider = provider().await;
    let harness = Harness::with(Setup {
        issuer: Some(provider.uri()),
        ..Setup::default()
    })
    .await;
    let response = harness.send(page("/auth/login?next=/", None)).await;
    let cookie = session_cookie(response.headers()).expect("a session cookie");
    let response = harness
        .send(post("/auth/logout", &cookie, None, vec![]))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let logout = Url::parse(&location(&response)).unwrap();
    assert_eq!(logout.path(), "/logout");
    let query: Vec<(String, String)> = logout.query_pairs().into_owned().collect();
    assert!(
        query.contains(&("client_id".into(), "recto".into())),
        "{logout}"
    );
    assert!(
        query.contains(&(
            "post_logout_redirect_uri".into(),
            "http://recto.test/auth/signed-out".into()
        )),
        "{logout}"
    );
    assert!(
        !query.iter().any(|(name, _)| name == "id_token_hint"),
        "{logout}"
    );
}

#[tokio::test]
async fn sign_out_follows_how_the_session_signed_in() {
    let provider = provider().await;
    let harness = Harness::with(Setup {
        issuer: Some(provider.uri()),
        ..Setup::default()
    })
    .await;
    // A first sign-in discovers the provider and its end-session endpoint.
    let response = harness.send(page("/auth/login?next=/", None)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    // Single sign-on: the provider's session ends too, with the ID token as a hint.
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let response = harness
        .send(post("/auth/logout", &cookie, None, vec![]))
        .await;
    let logout = Url::parse(&location(&response)).unwrap();
    assert_eq!(logout.path(), "/logout");
    assert!(
        logout
            .query_pairs()
            .any(|(name, value)| name == "id_token_hint" && value == "token"),
        "{logout}"
    );
    let response = harness.send(page("/", Some(&cookie))).await;
    assert_redirect(&response, "/sign-in?next=%2F");

    // A password, even for a user who also signs in through the provider: back to the sign-in
    // page, since the provider has no session of it.
    harness.local_user("bob", ALICE_PASSWORD).await;
    let cookie = harness.sign_in_with_password("bob", ALICE_PASSWORD).await;
    let response = harness
        .send(post("/auth/logout", &cookie, None, vec![]))
        .await;
    assert_redirect(&response, "/sign-in");
    let response = harness.send(page("/", Some(&cookie))).await;
    assert_redirect(&response, "/sign-in?next=%2F");
    users::set_password(
        &harness.database,
        harness.user_id("alice").await,
        ALICE_PASSWORD.to_owned(),
        &PasswordPolicy { minimum_length: 12 },
    )
    .await
    .unwrap();
    let cookie = harness.sign_in_with_password("alice", ALICE_PASSWORD).await;
    let response = harness
        .send(post("/auth/logout", &cookie, None, vec![]))
        .await;
    assert_redirect(&response, "/sign-in");
}

#[tokio::test]
async fn responses_carry_security_headers() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let response = harness.send(page("/", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(headers["referrer-policy"], "same-origin");
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    let policy = headers[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .to_owned();
    for directive in [
        "default-src 'self'",
        "frame-ancestors 'none'",
        "object-src 'none'",
        "'wasm-unsafe-eval'",
    ] {
        assert!(
            policy.contains(directive),
            "{directive} missing in {policy}"
        );
    }
    // The hydration script runs under a fresh nonce named in the policy.
    let html = text(response).await;
    let nonce = html
        .split("nonce=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("a script with a nonce");
    assert!(policy.contains(&format!("'nonce-{nonce}'")), "{policy}");
    assert!(!policy.contains("'unsafe-inline' 'nonce"), "{policy}");

    // Everything else cannot be framed or sniffed either.
    for request in [
        get("/health"),
        get("/pkg/recto.css"),
        page("/auth/signed-out", None),
    ] {
        let response = harness.send(request).await;
        let headers = response.headers();
        assert!(
            headers[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("frame-ancestors 'none'")
        );
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers["referrer-policy"], "same-origin");
    }
}

#[tokio::test]
async fn session_cookie_is_renewed_with_its_attributes() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    // Every signed-in request pushes the inactivity expiry back and says so in the cookie.
    for _ in 0..2 {
        let response = harness.send(page("/", Some(&cookie))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let header = set_cookie(response.headers());
        assert!(header.starts_with(&format!("{cookie};")), "{header}");
        for attribute in ["HttpOnly", "SameSite=Lax", "Max-Age=43200", "Path=/"] {
            assert!(
                header.contains(attribute),
                "{attribute} missing in {header}"
            );
        }
        assert!(!header.contains("Secure"), "{header}");
    }

    let harness = Harness::with(Setup {
        public_url: "https://recto.test/",
        ..Setup::default()
    })
    .await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let response = harness.send(page("/", Some(&cookie))).await;
    assert!(set_cookie(response.headers()).contains("Secure"));

    // Anonymous visits store nothing and set no cookie.
    let response = harness.send(page("/no-access", None)).await;
    assert!(session_cookie(response.headers()).is_none());
    assert_eq!(harness.session_count().await, 1);
}

#[tokio::test]
async fn fallback_never_renders_the_interface_for_anonymous_visitors() {
    let harness = Harness::new().await;
    for path in ["/no-access/", "/jobs/", "/anything/", "/auth/error/x"] {
        let response = harness.send(page(path, None)).await;
        let next: String = url::form_urlencoded::byte_serialize(path.as_bytes()).collect();
        assert_redirect(&response, &format!("/sign-in?next={next}"));
        let body = text(response).await;
        assert!(!body.contains("<html"), "{path}: {body}");
    }
    let response = harness.send(get("/anything")).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!text(response).await.contains("<html"));

    // Assets stay public.
    let response = harness.send(get("/pkg/recto.css")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(text(response).await, STYLESHEET);

    // A signed-in user gets the interface's not-found page.
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let response = harness.send(page("/anything/", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(text(response).await.contains("Page not found"));
}

/// A server function call from recto's own pages.
fn api(endpoint: &str, cookie: &str) -> Request<Body> {
    Request::post(format!("/api/{endpoint}"))
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, ORIGIN)
        .header("sec-fetch-site", "same-origin")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn disabled_user_is_signed_out_on_next_request() {
    let harness = Harness::new().await;
    let browser = harness.sign_in("alice-subject", "alice").await;
    let island = harness.sign_in("alice-subject", "alice").await;
    let administrator = harness.sign_in("admin-subject", "admin").await;
    assert_eq!(
        harness.send(page("/", Some(&browser))).await.status(),
        StatusCode::OK
    );
    let alice = harness.user_id("alice").await;
    let admin = harness.user_id("admin").await;

    users::set_disabled(&harness.database, admin, alice, true)
        .await
        .unwrap();

    let response = harness.send(page("/", Some(&browser))).await;
    assert_redirect(&response, "/sign-in?next=%2F");
    let response = harness.send(api("list_jobs", &island)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    // Both sessions of the disabled user are gone; the administrator's stays.
    assert_eq!(harness.session_count().await, 1);
    assert_eq!(
        harness.send(page("/", Some(&administrator))).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn password_change_ends_existing_sessions() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let alice = harness.user_id("alice").await;
    let policy = PasswordPolicy { minimum_length: 12 };

    users::set_password(
        &harness.database,
        alice,
        "a new long password".into(),
        &policy,
    )
    .await
    .unwrap();

    let response = harness.send(page("/", Some(&cookie))).await;
    assert_redirect(&response, "/sign-in?next=%2F");
    // Signing in again after the change works.
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let response = harness.send(page("/", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn sessions_survive_a_new_router_on_the_same_database() {
    let before = Harness::new().await;
    let cookie = before.sign_in("alice-subject", "alice").await;

    // A restart: new services and a new router, on the same database.
    let after = Harness::with(Setup {
        database: Some(before.database.clone()),
        ..Setup::default()
    })
    .await;
    drop(before);

    let response = after.send(page("/", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn sign_in_routes_are_absent_without_openid_connect() {
    let harness = Harness::with(Setup {
        issuer: None,
        ..Setup::default()
    })
    .await;
    for uri in ["/auth/login?next=/jobs", "/auth/callback?code=c&state=s"] {
        let response = harness.send(page(uri, None)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
    }

    // Signing out still ends the session, without a provider to send the browser to.
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let response = harness
        .send(post("/auth/logout", &cookie, None, vec![]))
        .await;
    assert_redirect(&response, "/auth/signed-out");
    let response = harness.send(page("/", Some(&cookie))).await;
    assert_redirect(&response, "/sign-in?next=%2F");
}

#[tokio::test]
async fn unwritable_database_fails_at_startup() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temporary_directory();
    let locked = dir.path().join("read-only");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let url = format!("sqlite://{}/recto.db", locked.display());

    let error = recto::state::open_database(&url).await.unwrap_err();

    // Let the temporary directory be removed.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(error.starts_with("cannot open the database "), "{error}");
    assert!(error.contains(&locked.display().to_string()), "{error}");
    // The setting to fix.
    assert!(error.contains("RECTO_DATABASE_URL"), "{error}");
}

const ALICE_PASSWORD: &str = "correct horse battery";

#[tokio::test]
async fn password_sign_in_succeeds_and_redirects_to_next() {
    let harness = Harness::new().await;
    harness.local_user("alice", ALICE_PASSWORD).await;

    // Both ways to sign in are offered, and both bring the visitor back to `next`.
    let response = harness.send(page("/sign-in?next=/jobs", None)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("action=\"/sign-in\""), "{html}");
    assert!(html.contains("name=\"next\" value=\"/jobs\""), "{html}");
    assert!(html.contains("href=\"/auth/login?next=%2Fjobs\""), "{html}");
    assert!(html.contains("Sign in with single sign-on"), "{html}");

    // A session planted before sign-in is replaced, not promoted (session fixation).
    let planted = Session::new(None, Arc::new(harness.sessions.clone()), None);
    planted.insert("planted", true).await.unwrap();
    planted.save().await.unwrap();
    let planted = format!("recto_session={}", planted.id().unwrap());
    let response = harness
        .send(form(
            "/sign-in",
            Some(&planted),
            &[
                ("login", "alice"),
                ("password", ALICE_PASSWORD),
                ("next", "/jobs"),
            ],
        ))
        .await;
    assert_redirect(&response, "/jobs");
    let cookie = session_cookie(response.headers()).expect("a session cookie");
    assert_ne!(cookie, planted);
    let response = harness.send(page("/jobs", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = harness.send(page("/jobs", Some(&planted))).await;
    assert_redirect(&response, "/sign-in?next=%2Fjobs");

    // Signed in, the sign-in page sends the user on.
    let response = harness
        .send(page("/sign-in?next=/scan", Some(&cookie)))
        .await;
    assert_redirect(&response, "/scan");

    // Only paths on this site are followed after sign-in.
    let response = harness
        .send(form(
            "/sign-in",
            None,
            &[
                ("login", "alice"),
                ("password", ALICE_PASSWORD),
                ("next", "//evil.example/x"),
            ],
        ))
        .await;
    assert_redirect(&response, "/");
}

/// The page without what differs from one rendering to the next: the nonce (wherever Leptos
/// puts it), and the login typed.
fn without_nonce_and_login(html: &str, login: &str) -> String {
    let nonce = html
        .split("nonce=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("a nonce");
    html.replace(nonce, "")
        .replace(&format!("value=\"{login}\""), "value=\"\"")
}

#[tokio::test]
async fn password_sign_in_failures_share_one_message() {
    let harness = Harness::new().await;
    harness.local_user("alice", ALICE_PASSWORD).await;
    let disabled = harness.local_user("dave", ALICE_PASSWORD).await;
    let administrator = harness.local_user("admin", ALICE_PASSWORD).await;
    users::set_disabled(&harness.database, administrator, disabled, true)
        .await
        .unwrap();
    // A single sign-on user, without a password.
    harness.sign_in("carol-subject", "carol").await;
    let sessions = harness.session_count().await;

    let mut pages = Vec::new();
    for (login, password) in [
        ("alice", "wrong password, long enough"),
        ("mallory", ALICE_PASSWORD),
        ("carol", ALICE_PASSWORD),
        ("dave", ALICE_PASSWORD),
    ] {
        let response = harness
            .send(form(
                "/sign-in",
                None,
                &[("login", login), ("password", password), ("next", "/jobs")],
            ))
            .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{login}");
        assert!(session_cookie(response.headers()).is_none(), "{login}");
        let html = text(response).await;
        assert!(
            html.contains("The username or password is wrong."),
            "{login}: {html}"
        );
        // The login stays typed in; the password never comes back.
        assert!(
            html.contains(&format!("value=\"{login}\"")),
            "{login}: {html}"
        );
        assert!(!html.contains(password), "{login}: {html}");
        assert!(
            html.contains("name=\"next\" value=\"/jobs\""),
            "{login}: {html}"
        );
        pages.push(without_nonce_and_login(&html, login));
    }
    for page in &pages[1..] {
        let differs = pages[0]
            .bytes()
            .zip(page.bytes())
            .position(|(a, b)| a != b)
            .unwrap_or(pages[0].len().min(page.len()));
        assert!(
            page == &pages[0],
            "the failures must not be told apart, yet they differ from: {:?}",
            &page[differs.saturating_sub(80)..(differs + 80).min(page.len())]
        );
    }
    assert_eq!(harness.session_count().await, sessions);
}

/// Six sign-in attempts sent at once, each from the forwarded address of `forwarded`; the
/// statuses that came back.
async fn six_attempts(harness: &Harness, forwarded: impl Fn(usize) -> String) -> Vec<StatusCode> {
    let mut attempts = tokio::task::JoinSet::new();
    for attempt in 0..6 {
        let mut request = form(
            "/sign-in",
            None,
            &[
                ("login", "alice"),
                ("password", "wrong password, long enough"),
            ],
        );
        request
            .headers_mut()
            .insert("x-forwarded-for", forwarded(attempt).parse().unwrap());
        let router = harness.router.clone();
        attempts.spawn(async move { router.oneshot(from_peer(request)).await.unwrap() });
    }
    let mut statuses = Vec::new();
    while let Some(response) = attempts.join_next().await {
        let response = response.unwrap();
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            let html = text(response).await;
            assert!(html.contains("Too many attempts"), "{html}");
            assert!(html.contains("value=\"alice\""), "{html}");
            statuses.push(StatusCode::TOO_MANY_REQUESTS);
        } else {
            statuses.push(response.status());
        }
    }
    statuses
}

fn limited(statuses: &[StatusCode]) -> usize {
    statuses
        .iter()
        .filter(|status| **status == StatusCode::TOO_MANY_REQUESTS)
        .count()
}

#[tokio::test]
async fn password_sign_in_is_rate_limited() {
    // By default the forwarded headers are ignored: every attempt comes from the one peer, so a
    // client cannot escape the limit by naming another address.
    let harness = Harness::new().await;
    harness.local_user("alice", ALICE_PASSWORD).await;
    let statuses = six_attempts(&harness, |attempt| format!("10.0.0.{attempt}")).await;
    assert_eq!(limited(&statuses), 1, "{statuses:?}");

    // Behind a trusted proxy, each forwarded address has its own budget.
    let harness = Harness::with(Setup {
        trust_forwarded_headers: true,
        ..Setup::default()
    })
    .await;
    harness.local_user("alice", ALICE_PASSWORD).await;
    let statuses = six_attempts(&harness, |attempt| format!("10.0.0.{attempt}")).await;
    assert_eq!(limited(&statuses), 0, "{statuses:?}");
    let statuses = six_attempts(&harness, |_| "10.0.1.1".to_owned()).await;
    assert_eq!(limited(&statuses), 1, "{statuses:?}");

    // An IPv6 client picks its address within a /64: the network shares one budget.
    let statuses = six_attempts(&harness, |attempt| format!("2001:db8:0:1::{attempt}")).await;
    assert_eq!(limited(&statuses), 1, "{statuses:?}");
    let statuses = six_attempts(&harness, |attempt| format!("2001:db8:1:{attempt}::1")).await;
    assert_eq!(limited(&statuses), 0, "{statuses:?}");
}

/// A password no account can have, refused without the cost of a hash: enough to spend the
/// budget of attempts quickly.
fn overlong_password() -> String {
    "x".repeat(passwords::MAXIMUM_LENGTH + 1)
}

#[tokio::test]
async fn the_right_password_waits_once_the_budget_is_spent() {
    let harness = Harness::new().await;
    harness.local_user("alice", ALICE_PASSWORD).await;
    let overlong = overlong_password();
    for _ in 0..5 {
        let response = harness
            .send(form(
                "/sign-in",
                None,
                &[("login", "alice"), ("password", &overlong)],
            ))
            .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    let response = harness
        .send(form(
            "/sign-in",
            None,
            &[("login", "alice"), ("password", ALICE_PASSWORD)],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(session_cookie(response.headers()).is_none());
    assert!(text(response).await.contains("Too many attempts"));
}

#[tokio::test]
async fn password_changes_share_the_sign_in_budget() {
    let harness = Harness::new().await;
    harness.local_user("alice", ALICE_PASSWORD).await;
    // One attempt of the budget.
    let cookie = harness.sign_in_with_password("alice", ALICE_PASSWORD).await;
    let change = |current: &str| {
        form(
            "/account/password",
            Some(&cookie),
            &[
                ("current", current),
                ("new", "another long password"),
                ("confirm", "another long password"),
            ],
        )
    };
    let overlong = overlong_password();
    for _ in 0..4 {
        let response = harness.send(change(&overlong)).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    let response = harness.send(change(ALICE_PASSWORD)).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let html = text(response).await;
    assert!(html.contains("Too many attempts"), "{html}");
    // Nothing changed: the session and the password are as they were.
    let response = harness.send(page("/account", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::OK);
    let user = harness.user("alice").await;
    assert!(passwords::verify(ALICE_PASSWORD.to_owned(), user.password_hash.unwrap()).await);
}

#[tokio::test]
async fn sign_in_page_redirects_with_only_sso() {
    let harness = Harness::with(Setup {
        local_accounts: false,
        ..Setup::default()
    })
    .await;
    let response = harness.send(page("/", None)).await;
    assert_redirect(&response, "/sign-in?next=%2F");
    let response = harness.send(page("/sign-in?next=/jobs", None)).await;
    assert_redirect(&response, "/auth/login?next=%2Fjobs");

    // With only local accounts, the page has the form even before any account has a password,
    // and no single sign-on link.
    let harness = Harness::with(Setup {
        issuer: None,
        ..Setup::default()
    })
    .await;
    let response = harness.send(page("/sign-in", None)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("action=\"/sign-in\""), "{html}");
    assert!(!html.contains("/auth/login"), "{html}");
    // Signed out, the way back is the sign-in page, which works without a provider.
    let response = harness.send(page("/auth/signed-out", None)).await;
    assert!(text(response).await.contains("href=\"/sign-in\""));
}

#[tokio::test]
async fn sign_in_page_goes_to_the_provider_until_an_account_has_a_password() {
    let harness = Harness::new().await;
    // A single sign-on user has no password: the form could sign nobody in.
    harness.sign_in("carol-subject", "carol").await;
    let response = harness.send(page("/sign-in?next=/jobs", None)).await;
    assert_redirect(&response, "/auth/login?next=%2Fjobs");

    harness.local_user("alice", ALICE_PASSWORD).await;
    let response = harness.send(page("/sign-in?next=/jobs", None)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("action=\"/sign-in\""), "{html}");
    assert!(html.contains("name=\"next\" value=\"/jobs\""), "{html}");
}

#[tokio::test]
async fn sign_in_routes_absent_without_local_accounts() {
    let harness = Harness::with(Setup {
        local_accounts: false,
        ..Setup::default()
    })
    .await;
    harness.local_user("alice", ALICE_PASSWORD).await;
    let response = harness
        .send(form(
            "/sign-in",
            None,
            &[("login", "alice"), ("password", ALICE_PASSWORD)],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(session_cookie(response.headers()).is_none());

    // A password is of no use without local accounts, so there is no Account page either.
    let alice = harness.user_id("alice").await;
    let session = Session::new(None, Arc::new(harness.sessions.clone()), None);
    recto::session::sign_in(&session, alice, SignInMethod::Password)
        .await
        .unwrap();
    session.save().await.unwrap();
    let cookie = format!("recto_session={}", session.id().unwrap());
    let response = harness.send(page("/account", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = harness
        .send(form(
            "/account/password",
            Some(&cookie),
            &[
                ("current", ALICE_PASSWORD),
                ("new", "another long password"),
                ("confirm", "another long password"),
            ],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn changing_password_keeps_this_session_only() {
    let harness = Harness::new().await;
    harness.local_user("alice", ALICE_PASSWORD).await;
    let this = harness.sign_in_with_password("alice", ALICE_PASSWORD).await;
    let other = harness.sign_in_with_password("alice", ALICE_PASSWORD).await;

    // Users with a password find the Account page in the header.
    let response = harness.send(page("/", Some(&this))).await;
    assert!(text(response).await.contains("href=\"/account\""));
    let response = harness.send(page("/account", Some(&this))).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("action=\"/account/password\""), "{html}");
    assert!(html.contains("At least 12 characters."), "{html}");

    // From another address than the sign-ins: both forms share one budget of attempts.
    let change = |current: &str, new: &str, confirm: &str| {
        from_address(
            form(
                "/account/password",
                Some(&this),
                &[("current", current), ("new", new), ("confirm", confirm)],
            ),
            [192, 0, 2, 2],
        )
    };
    for (request, message) in [
        (
            change(
                "not my password",
                "another long password",
                "another long password",
            ),
            "The current password is wrong.",
        ),
        (
            change(
                ALICE_PASSWORD,
                "another long password",
                "another long passwork",
            ),
            "The new passwords do not match.",
        ),
        (
            change(ALICE_PASSWORD, "too short", "too short"),
            "The new password must be at least 12 characters long.",
        ),
    ] {
        let response = harness.send(request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{message}");
        let html = text(response).await;
        assert!(html.contains(message), "{message}: {html}");
        assert!(!html.contains(ALICE_PASSWORD), "{html}");
    }
    // Refused changes leave both sessions signed in.
    for cookie in [&this, &other] {
        let response = harness.send(page("/", Some(cookie))).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    let response = harness
        .send(change(
            ALICE_PASSWORD,
            "another long password",
            "another long password",
        ))
        .await;
    assert_redirect(&response, "/account?changed=1");
    // This session carries on under a new id.
    let renewed = session_cookie(response.headers()).expect("a new session id");
    assert_ne!(renewed, this);
    let response = harness
        .send(page("/account?changed=1", Some(&renewed)))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text(response).await.contains("Your password was changed."));

    // The other session ended; the new password signs in, the old one no longer does.
    for ended in [&other, &this] {
        let response = harness.send(page("/", Some(ended))).await;
        assert_redirect(&response, "/sign-in?next=%2F");
    }
    harness
        .sign_in_with_password("alice", "another long password")
        .await;
    let response = harness
        .send(form(
            "/sign-in",
            None,
            &[("login", "alice"), ("password", ALICE_PASSWORD)],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn account_page_is_only_for_users_with_a_password() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("carol-subject", "carol").await;
    let response = harness.send(page("/", Some(&cookie))).await;
    assert!(!text(response).await.contains("href=\"/account\""));
    let response = harness.send(page("/account", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(text(response).await.contains("Page not found"));
    let response = harness
        .send(form(
            "/account/password",
            Some(&cookie),
            &[
                ("current", ""),
                ("new", "a long new password"),
                ("confirm", "a long new password"),
            ],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(
        users::find_by_username(&harness.database, "carol")
            .await
            .unwrap()
            .unwrap()
            .password_hash
            .is_none()
    );

    // Anonymous visitors are sent to sign in first.
    let response = harness.send(page("/account", None)).await;
    assert_redirect(&response, "/sign-in?next=%2Faccount");
    let response = harness
        .send(form(
            "/account/password",
            None,
            &[("current", ""), ("new", ""), ("confirm", "")],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn password_form_refuses_cross_site_posts() {
    let harness = Harness::new().await;
    harness.local_user("alice", ALICE_PASSWORD).await;
    let cookie = harness.sign_in_with_password("alice", ALICE_PASSWORD).await;
    let sessions = harness.session_count().await;

    let forged = |uri: &str, cookie: Option<&str>, fields: &[(&str, &str)]| {
        let mut request = form(uri, cookie, fields);
        let headers = request.headers_mut();
        headers.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        headers.insert("sec-fetch-site", "cross-site".parse().unwrap());
        request
    };
    let response = harness
        .send(forged(
            "/sign-in",
            None,
            &[("login", "alice"), ("password", ALICE_PASSWORD)],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(session_cookie(response.headers()).is_none());
    assert_eq!(harness.session_count().await, sessions);

    let response = harness
        .send(forged(
            "/account/password",
            Some(&cookie),
            &[
                ("current", ALICE_PASSWORD),
                ("new", "chosen by the attacker"),
                ("confirm", "chosen by the attacker"),
            ],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    // The password is unchanged, and the session still signed in.
    harness.sign_in_with_password("alice", ALICE_PASSWORD).await;
    let response = harness.send(page("/", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::OK);
}

impl Harness {
    /// Signs in a single sign-on user made administrator by hand and returns the session cookie.
    async fn administrator(&self, subject: &str, username: &str) -> String {
        let cookie = self.sign_in(subject, username).await;
        let id = self.user_id(username).await;
        users::set_administrator_from_command_line(&self.database, id, true)
            .await
            .unwrap();
        cookie
    }

    async fn user(&self, username: &str) -> recto_entity::users::Model {
        users::find_by_username(&self.database, username)
            .await
            .unwrap()
            .expect("a user")
    }
}

/// The administration forms of a user, as the Users page posts them.
fn administration_forms(id: i64) -> [(String, Vec<(&'static str, &'static str)>); 4] {
    [
        ("/administration/users".to_owned(), new_user("dave")),
        (
            format!("/administration/users/{id}/administrator"),
            vec![("value", "false")],
        ),
        (
            format!("/administration/users/{id}/disabled"),
            vec![("value", "true")],
        ),
        (
            format!("/administration/users/{id}/password"),
            vec![("password", "chosen by someone else")],
        ),
    ]
}

const DAVE_PASSWORD: &str = "dave's long password";

fn new_user(username: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![
        ("username", username),
        ("display_name", "Dave Example"),
        ("email", "dave@example.test"),
        ("password", DAVE_PASSWORD),
    ]
}

#[tokio::test]
async fn administration_is_hidden_from_non_admins() {
    let harness = Harness::new().await;
    let admin = harness.administrator("admin-subject", "admin").await;
    let carol = harness.sign_in("carol-subject", "carol").await;
    let admin_id = harness.user_id("admin").await;

    // Administrators find it in the navigation; nobody else does.
    let response = harness.send(page("/", Some(&admin))).await;
    assert!(
        text(response)
            .await
            .contains("href=\"/administration/users\"")
    );
    let response = harness.send(page("/", Some(&carol))).await;
    assert!(!text(response).await.contains("/administration"));
    // The section's own address leads administrators to its first page.
    let response = harness.send(page("/administration", Some(&admin))).await;
    assert_redirect(&response, "/administration/users");

    for path in [
        "/administration",
        "/administration/users",
        "/administration/groups",
        "/administration/users/",
        "/administration/groups/",
    ] {
        let response = harness.send(page(path, Some(&carol))).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        let html = text(response).await;
        assert!(html.contains("Page not found"), "{path}: {html}");
        assert!(
            !html.contains("admin-subject") && !html.contains(">admin<"),
            "{html}"
        );
    }
    for (uri, fields) in administration_forms(admin_id) {
        let response = harness.send(form(&uri, Some(&carol), &fields)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        assert!(text(response).await.contains("Page not found"), "{uri}");
    }
    // Not even a malformed form tells that the route exists.
    let response = harness
        .send(form(
            &format!("/administration/users/{admin_id}/disabled"),
            Some(&carol),
            &[("value", "maybe")],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let admin_row = harness.user("admin").await;
    assert!(admin_row.is_administrator && !admin_row.is_disabled);
    assert!(admin_row.password_hash.is_none());
    assert!(
        users::find_by_username(&harness.database, "dave")
            .await
            .unwrap()
            .is_none()
    );

    // Anonymous visitors are sent to sign in, as on every page.
    let response = harness.send(page("/administration/users", None)).await;
    assert_redirect(&response, "/sign-in?next=%2Fadministration%2Fusers");
    for (uri, fields) in administration_forms(admin_id) {
        let response = harness.send(form(&uri, None, &fields)).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
}

#[tokio::test]
async fn administration_pages_have_their_own_titles() {
    let harness = Harness::new().await;
    let admin = harness.administrator("admin-subject", "admin").await;
    for (path, title) in [
        (
            "/administration/users",
            "<title>Users · Administration · recto</title>",
        ),
        (
            "/administration/groups",
            "<title>Groups · Administration · recto</title>",
        ),
    ] {
        let response = harness.send(page(path, Some(&admin))).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let html = text(response).await;
        assert!(html.contains(title), "{path}: {html}");
        assert!(html.contains(">Administration</h1>"), "{path}: {html}");
    }
}

#[tokio::test]
async fn crafted_notices_put_nothing_on_the_users_page() {
    let harness = Harness::new().await;
    let admin = harness.administrator("admin-subject", "admin").await;
    let id = harness.user_id("admin").await;
    let users_page = |query: String| page(&format!("/administration/users?{query}"), Some(&admin));

    // A notice of the server names a listed user.
    let response = harness
        .send(users_page(format!("done=created&user={id}")))
        .await;
    assert!(
        text(response)
            .await
            .contains("The account admin was created.")
    );

    for query in [
        format!("done=%3Cscript%3Ealert(1)%3C%2Fscript%3E&user={id}"),
        format!("done=Your+account+was+deleted&user={id}"),
        format!("done=created&user={}", id + 100),
        "done=created&user=admin".to_owned(),
        "done=created".to_owned(),
        format!("user={id}"),
    ] {
        let response = harness.send(users_page(query.clone())).await;
        assert_eq!(response.status(), StatusCode::OK, "{query}");
        let html = text(response).await;
        assert!(!html.contains("role=\"status\""), "{query}: {html}");
        assert!(!html.contains("was created"), "{query}: {html}");
        assert!(!html.contains("<script>alert"), "{query}: {html}");
        assert!(!html.contains("was deleted"), "{query}: {html}");
    }
}

#[tokio::test]
async fn admin_creates_a_local_user_who_can_sign_in() {
    let harness = Harness::new().await;
    let admin = harness.administrator("admin-subject", "admin").await;

    let response = harness
        .send(page("/administration/users", Some(&admin)))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("New local user"), "{html}");
    assert!(html.contains("action=\"/administration/users\""), "{html}");

    let response = harness
        .send(form(
            "/administration/users",
            Some(&admin),
            &new_user("dave"),
        ))
        .await;
    let dave = harness.user_id("dave").await;
    assert_redirect(
        &response,
        &format!("/administration/users?done=created&user={dave}"),
    );
    let response = harness.send(page(&location(&response), Some(&admin))).await;
    let html = text(response).await;
    assert!(html.contains("The account dave was created."), "{html}");
    assert!(html.contains("Dave Example"), "{html}");
    assert!(html.contains("dave@example.test"), "{html}");
    let row = harness.user("dave").await;
    assert!(!row.is_administrator);

    let session = harness.sign_in_with_password("dave", DAVE_PASSWORD).await;
    assert_eq!(
        harness.send(page("/", Some(&session))).await.status(),
        StatusCode::OK
    );

    // A refused account keeps what was typed, except the password, and says why.
    let mut taken = new_user("dave");
    taken.push(("administrator", "on"));
    for (fields, message) in [
        (taken, "That username is taken."),
        (
            vec![
                ("username", "Erin!"),
                ("display_name", "Erin"),
                ("password", DAVE_PASSWORD),
            ],
            "The username must be 1 to 64 characters",
        ),
        (
            vec![
                ("username", "erin"),
                ("display_name", "Erin"),
                ("email", "dave@example.test"),
                ("password", DAVE_PASSWORD),
            ],
            "That email address belongs to another user.",
        ),
        (
            vec![("username", "erin"), ("password", "too short")],
            "The password must be at least 12 characters long.",
        ),
    ] {
        let username = fields[0].1;
        let response = harness
            .send(form("/administration/users", Some(&admin), &fields))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{message}");
        let html = text(response).await;
        assert!(html.contains(message), "{message}: {html}");
        assert!(html.contains(&format!("value=\"{username}\"")), "{html}");
        assert!(
            !html.contains(DAVE_PASSWORD) && !html.contains("too short"),
            "{html}"
        );
    }
    assert!(
        users::find_by_username(&harness.database, "erin")
            .await
            .unwrap()
            .is_none()
    );

    // A new password ends the user's sessions; the new one signs in.
    let response = harness
        .send(form(
            &format!("/administration/users/{dave}/password"),
            Some(&admin),
            &[("password", "a fresh long password")],
        ))
        .await;
    assert_redirect(
        &response,
        &format!("/administration/users?done=password_set&user={dave}"),
    );
    let response = harness.send(page("/", Some(&session))).await;
    assert_redirect(&response, "/sign-in?next=%2F");
    harness
        .sign_in_with_password("dave", "a fresh long password")
        .await;
    let response = harness
        .send(form(
            &format!("/administration/users/{dave}/password"),
            Some(&admin),
            &[("password", "short")],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        text(response)
            .await
            .contains("dave: The password must be at least 12 characters long.")
    );

    // An administrator created as one.
    let mut fields = new_user("erin");
    fields[2] = ("email", "");
    fields.push(("administrator", "on"));
    let response = harness
        .send(form("/administration/users", Some(&admin), &fields))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let erin = harness.user("erin").await;
    assert!(erin.is_administrator && erin.email.is_none());

    // Without local accounts there is no form, and posting one is refused.
    let harness = Harness::with(Setup {
        local_accounts: false,
        ..Setup::default()
    })
    .await;
    let admin = harness.administrator("admin-subject", "admin").await;
    let response = harness
        .send(page("/administration/users", Some(&admin)))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(!html.contains("New local user"), "{html}");
    let response = harness
        .send(form(
            "/administration/users",
            Some(&admin),
            &new_user("dave"),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_disables_a_user_and_their_session_ends() {
    let harness = Harness::new().await;
    let admin = harness.administrator("admin-subject", "admin").await;
    let alice = harness.sign_in("alice-subject", "alice").await;
    let alice_id = harness.user_id("alice").await;

    let response = harness
        .send(form(
            &format!("/administration/users/{alice_id}/disabled"),
            Some(&admin),
            &[("value", "true")],
        ))
        .await;
    assert_redirect(
        &response,
        &format!("/administration/users?done=disabled&user={alice_id}"),
    );
    assert!(harness.user("alice").await.is_disabled);
    let response = harness.send(page("/", Some(&alice))).await;
    assert_redirect(&response, "/sign-in?next=%2F");
    let response = harness
        .send(page(
            &format!("/administration/users?done=disabled&user={alice_id}"),
            Some(&admin),
        ))
        .await;
    let html = text(response).await;
    assert!(
        html.contains("alice is disabled and was signed out."),
        "{html}"
    );
    assert!(html.contains("Disabled"), "{html}");

    let response = harness
        .send(form(
            &format!("/administration/users/{alice_id}/disabled"),
            Some(&admin),
            &[("value", "false")],
        ))
        .await;
    assert_redirect(
        &response,
        &format!("/administration/users?done=enabled&user={alice_id}"),
    );
    assert!(!harness.user("alice").await.is_disabled);

    // Administration is granted and removed the same way.
    for (value, done, expected) in [("true", "granted", true), ("false", "removed", false)] {
        let response = harness
            .send(form(
                &format!("/administration/users/{alice_id}/administrator"),
                Some(&admin),
                &[("value", value)],
            ))
            .await;
        assert_redirect(
            &response,
            &format!("/administration/users?done={done}&user={alice_id}"),
        );
        assert_eq!(harness.user("alice").await.is_administrator, expected);
    }

    // A user that does not exist.
    let response = harness
        .send(form(
            "/administration/users/999999/disabled",
            Some(&admin),
            &[("value", "true")],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(text(response).await.contains("That user no longer exists."));
}

#[tokio::test]
async fn admin_cannot_disable_or_demote_themselves() {
    let harness = Harness::new().await;
    let admin = harness.administrator("admin-subject", "admin").await;
    harness.sign_in("alice-subject", "alice").await;
    let admin_id = harness.user_id("admin").await;
    let alice_id = harness.user_id("alice").await;

    let response = harness
        .send(page("/administration/users", Some(&admin)))
        .await;
    let html = text(response).await;
    for action in ["disabled", "administrator", "password"] {
        assert!(
            !html.contains(&format!("/administration/users/{admin_id}/{action}")),
            "{action}: {html}"
        );
    }
    for action in ["disabled", "administrator"] {
        assert!(
            html.contains(&format!("/administration/users/{alice_id}/{action}")),
            "{action}: {html}"
        );
    }

    for (action, value) in [("disabled", "true"), ("administrator", "false")] {
        let response = harness
            .send(form(
                &format!("/administration/users/{admin_id}/{action}"),
                Some(&admin),
                &[("value", value)],
            ))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{action}");
        let html = text(response).await;
        assert!(
            html.contains("You cannot change this on your own account."),
            "{html}"
        );
    }
    let row = harness.user("admin").await;
    assert!(row.is_administrator && !row.is_disabled);
    let response = harness
        .send(page("/administration/users", Some(&admin)))
        .await;
    assert_eq!(response.status(), StatusCode::OK);

    // Nor can an administrator set their own password here: it would sign them out everywhere,
    // where the Account page keeps their session.
    let harness = Harness::new().await;
    harness.local_user("root", ALICE_PASSWORD).await;
    let root_id = harness.user_id("root").await;
    users::set_administrator_from_command_line(&harness.database, root_id, true)
        .await
        .unwrap();
    let root = harness.sign_in_with_password("root", ALICE_PASSWORD).await;
    let response = harness
        .send(form(
            &format!("/administration/users/{root_id}/password"),
            Some(&root),
            &[("password", "chosen on the wrong page")],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let html = text(response).await;
    assert!(
        html.contains("root: You cannot change this on your own account."),
        "{html}"
    );
    // The password is unchanged, and the session still signed in.
    harness.sign_in_with_password("root", ALICE_PASSWORD).await;
    let response = harness
        .send(page("/administration/users", Some(&root)))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn claim_granted_admin_has_no_manual_toggle() {
    let harness = Harness::new().await;
    let admin = harness.administrator("admin-subject", "admin").await;
    let bob = users::sign_in_with_identity(
        &harness.database,
        &Identity {
            issuer: "https://id.test/".into(),
            subject: "bob-subject".into(),
            preferred_username: Some("bob".into()),
            administrator: Some(true),
            ..Identity::default()
        },
    )
    .await
    .unwrap();
    assert!(bob.administrator_from_claim);

    let response = harness
        .send(page("/administration/users", Some(&admin)))
        .await;
    let html = text(response).await;
    assert!(html.contains("From the identity provider"), "{html}");
    assert!(
        !html.contains(&format!("/administration/users/{}/administrator", bob.id)),
        "{html}"
    );
    // Disabling stays possible.
    assert!(
        html.contains(&format!("/administration/users/{}/disabled", bob.id)),
        "{html}"
    );
    // A hand-made post changes nothing either way: the provider decides.
    for value in ["false", "true"] {
        let response = harness
            .send(form(
                &format!("/administration/users/{}/administrator", bob.id),
                Some(&admin),
                &[("value", value)],
            ))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{value}");
        let html = text(response).await;
        assert!(
            html.contains("bob: Administration of this user comes from the identity provider."),
            "{html}"
        );
        let row = harness.user("bob").await;
        assert!(
            row.is_administrator && row.administrator_from_claim,
            "{value}"
        );
    }
}

#[tokio::test]
async fn groups_page_lists_members() {
    let harness = Harness::new().await;
    let admin = harness.administrator("admin-subject", "admin").await;
    for (subject, username, name, groups) in [
        (
            "alice-subject",
            "alice",
            "Alice Martin",
            vec!["family", "office"],
        ),
        ("bob-subject", "bob", "Bob Martin", vec!["family"]),
        ("carol-subject", "carol", "Carol Smith", vec!["office"]),
    ] {
        users::sign_in_with_identity(
            &harness.database,
            &Identity {
                issuer: "https://id.test/".into(),
                subject: subject.into(),
                preferred_username: Some(username.into()),
                name: Some(name.into()),
                groups: Some(groups.into_iter().map(str::to_owned).collect()),
                ..Identity::default()
            },
        )
        .await
        .unwrap();
    }
    // Carol leaves the office: the group stays, empty.
    users::sign_in_with_identity(
        &harness.database,
        &Identity {
            issuer: "https://id.test/".into(),
            subject: "carol-subject".into(),
            preferred_username: Some("carol".into()),
            groups: Some(vec![]),
            ..Identity::default()
        },
    )
    .await
    .unwrap();
    users::sign_in_with_identity(
        &harness.database,
        &Identity {
            issuer: "https://id.test/".into(),
            subject: "alice-subject".into(),
            preferred_username: Some("alice".into()),
            name: Some("Alice Martin".into()),
            groups: Some(vec!["family".into()]),
            ..Identity::default()
        },
    )
    .await
    .unwrap();

    let response = harness
        .send(page("/administration/groups", Some(&admin)))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("from the identity provider"), "{html}");
    let family = html.find(">family<").expect("the family group");
    let office = html.find(">office<").expect("the office group");
    assert!(family < office, "{html}");
    let family_section = &html[family..office];
    assert!(family_section.contains("Alice Martin"), "{html}");
    assert!(family_section.contains("Bob Martin"), "{html}");
    assert!(!family_section.contains("Carol Smith"), "{html}");
    assert!(html[office..].contains("No members"), "{html}");
    // No form: groups are read-only.
    assert!(!html.contains("action=\"/administration/groups"), "{html}");

    // The Users page shows each user's groups too.
    let response = harness
        .send(page("/administration/users", Some(&admin)))
        .await;
    let html = text(response).await;
    let bob = html.find("Bob Martin").expect("bob's row");
    assert!(html[bob..].contains("family"), "{html}");
}

#[tokio::test]
async fn administration_forms_refuse_cross_site_posts() {
    let harness = Harness::new().await;
    let admin = harness.administrator("admin-subject", "admin").await;
    harness.local_user("alice", ALICE_PASSWORD).await;
    let alice_id = harness.user_id("alice").await;

    for (uri, fields) in administration_forms(alice_id) {
        let mut request = form(&uri, Some(&admin), &fields);
        let headers = request.headers_mut();
        headers.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        headers.insert("sec-fetch-site", "cross-site".parse().unwrap());
        let response = harness.send(request).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{uri}");
    }
    let alice = harness.user("alice").await;
    assert!(!alice.is_disabled && !alice.is_administrator);
    harness.sign_in_with_password("alice", ALICE_PASSWORD).await;
    assert!(
        users::find_by_username(&harness.database, "dave")
            .await
            .unwrap()
            .is_none()
    );
}

/// Every `Set-Cookie` value of a response.
fn set_cookies(response: &Response<Body>) -> Vec<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn the_theme_form_sets_a_cookie_and_returns() {
    let harness = Harness::new().await;
    let response = harness
        .send(form(
            "/theme",
            None,
            &[("theme", "dark"), ("next", "/jobs")],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/jobs");
    let cookies = set_cookies(&response);
    let cookie = cookies
        .iter()
        .find(|value| value.starts_with("recto_theme=dark"))
        .unwrap_or_else(|| panic!("a theme cookie in {cookies:?}"));
    for attribute in ["Path=/", "Max-Age=31536000", "SameSite=Lax", "HttpOnly"] {
        assert!(
            cookie.contains(attribute),
            "{attribute} missing in {cookie}"
        );
    }
    assert!(!cookie.contains("Secure"), "{cookie}");
}

#[tokio::test]
async fn the_theme_cookie_is_secure_behind_https() {
    let harness = Harness::with(Setup {
        public_url: "https://recto.test/",
        ..Setup::default()
    })
    .await;
    let mut request = form("/theme", None, &[("theme", "light")]);
    request
        .headers_mut()
        .insert(header::ORIGIN, "https://recto.test".parse().unwrap());
    let response = harness.send(request).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let cookies = set_cookies(&response);
    assert!(
        cookies
            .iter()
            .any(|value| value.starts_with("recto_theme=light") && value.contains("Secure")),
        "{cookies:?}"
    );
}

#[tokio::test]
async fn the_system_theme_clears_the_cookie() {
    let harness = Harness::new().await;
    let response = harness
        .send(form("/theme", None, &[("theme", "system")]))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/");
    let cookies = set_cookies(&response);
    assert!(
        cookies
            .iter()
            .any(|value| value.starts_with("recto_theme=;") && value.contains("Max-Age=0")),
        "{cookies:?}"
    );
}

#[tokio::test]
async fn the_theme_form_only_returns_to_local_paths() {
    let harness = Harness::new().await;
    let response = harness
        .send(form(
            "/theme",
            None,
            &[("theme", "light"), ("next", "//evil.example")],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/");
}

#[tokio::test]
async fn an_unknown_theme_is_refused() {
    let harness = Harness::new().await;
    let response = harness
        .send(form("/theme", None, &[("theme", "purple")]))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(set_cookies(&response).is_empty());
}

#[tokio::test]
async fn the_theme_form_refuses_cross_site_posts() {
    let harness = Harness::new().await;
    let mut request = form("/theme", None, &[("theme", "dark")]);
    request
        .headers_mut()
        .insert("sec-fetch-site", "cross-site".parse().unwrap());
    let response = harness.send(request).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(set_cookies(&response).is_empty());
}

#[tokio::test]
async fn brand_icons_are_public() {
    let harness = Harness::new().await;
    for path in [
        "/brand/icon-192.png",
        "/brand/icon-512.png",
        "/brand/icon-maskable-512.png",
        "/brand/apple-touch-icon.png",
    ] {
        let response = harness.send(get(path)).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let headers = response.headers();
        assert_eq!(headers[header::CONTENT_TYPE], "image/png", "{path}");
        assert_eq!(
            headers[header::CACHE_CONTROL],
            "public, max-age=3600",
            "{path}"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(body.starts_with(&PNG[..8]), "{path}");
    }
}

#[tokio::test]
async fn without_a_logo_there_is_none() {
    let harness = Harness::new().await;
    let response = harness.send(get("/brand/logo")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_svg_logo_is_sandboxed() {
    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#;
    let harness = Harness::with(Setup {
        branding: Branding {
            logo: Some(BrandFile {
                bytes: axum::body::Bytes::from_static(SVG.as_bytes()),
                content_type: "image/svg+xml",
            }),
            ..default_branding()
        },
        ..Setup::default()
    })
    .await;
    let response = harness.send(get("/brand/logo")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    assert_eq!(headers[header::CONTENT_TYPE], "image/svg+xml");
    assert_eq!(headers[header::CACHE_CONTROL], "public, max-age=3600");
    let policy = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    for directive in ["sandbox", "default-src 'none'", "frame-ancestors 'none'"] {
        assert!(policy.contains(directive), "{policy}");
    }
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(text(response).await, SVG);
}

#[tokio::test]
async fn a_cross_site_refusal_names_the_brand() {
    let harness = Harness::with(Setup {
        branding: Branding {
            name: "Maison".into(),
            ..default_branding()
        },
        ..Setup::default()
    })
    .await;
    let request = Request::post("/theme")
        .header(header::ORIGIN, "http://evil.test")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("theme=dark"))
        .unwrap();
    let response = harness.send(request).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(text(response).await.contains("Open Maison at"));
}

#[tokio::test]
async fn the_manifest_is_public() {
    let harness = Harness::with(Setup {
        branding: Branding {
            name: "Maison".into(),
            color: "#aa3300".into(),
            ..default_branding()
        },
        ..Setup::default()
    })
    .await;
    let response = harness.send(get("/manifest.webmanifest")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/manifest+json"
    );
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(manifest["name"], "Maison");
    assert_eq!(manifest["display"], "standalone");
    assert_eq!(manifest["theme_color"], "#aa3300");
    assert_eq!(manifest["start_url"], "/");
    let icons = manifest["icons"].as_array().unwrap();
    assert_eq!(icons.len(), 3);
    assert_eq!(
        icons
            .iter()
            .filter(|icon| icon["purpose"] == "maskable")
            .count(),
        1
    );
}

#[tokio::test]
async fn the_service_worker_is_public_and_versioned() {
    let harness = Harness::new().await;
    let response = harness.send(get("/sw.js")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    assert_eq!(
        headers[header::CONTENT_TYPE],
        "text/javascript; charset=utf-8"
    );
    assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let source = String::from_utf8(body.to_vec()).unwrap();
    assert!(source.contains(concat!("recto-", env!("CARGO_PKG_VERSION"))));
    assert!(!source.contains("__VERSION__"));
}

fn job_of(id: i32, owner: &str) -> Job {
    Job {
        id,
        printer: "office".into(),
        name: Some(format!("document-{id}.pdf")),
        owner: Some(owner.into()),
        state: JobState::Processing,
        pages: None,
        created: Some(chrono::Utc::now().timestamp()),
    }
}

/// A private harness with jobs 1 (alice's), 2 (bob's) and 3 (unrecorded); returns alice's cookie.
async fn private_jobs() -> (Harness, String) {
    let harness = Harness::with(Setup {
        job_visibility: JobVisibility::Own,
        jobs: Some(vec![
            job_of(1, "alice"),
            job_of(2, "alice"),
            job_of(3, "carol"),
        ]),
        ..Setup::default()
    })
    .await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    harness.sign_in("bob-subject", "bob").await;
    for (name, id) in [("alice", 1), ("bob", 2)] {
        let user = harness.user_id(name).await;
        jobs::record(
            &harness.database,
            jobs::NewJob {
                user_id: user,
                printer: "office".into(),
                cups_job_id: id,
                document_name: format!("document-{id}.pdf"),
            },
        )
        .await
        .unwrap();
    }
    (harness, cookie)
}

#[tokio::test]
async fn others_jobs_cannot_be_canceled() {
    let (harness, cookie) = private_jobs().await;
    let response = harness
        .send(post("/jobs/2/cancel", &cookie, None, vec![]))
        .await;
    assert_redirect(&response, "/jobs?error=job_not_found");
    assert!(harness.print.canceled().is_empty());

    let response = harness
        .send(post("/jobs/1/cancel", &cookie, None, vec![]))
        .await;
    assert_redirect(&response, "/jobs");
    assert_eq!(harness.print.canceled(), [(1, "alice".to_owned())]);
}

#[tokio::test]
async fn unrecorded_jobs_are_canceled_by_administrators_only() {
    let (harness, cookie) = private_jobs().await;
    let response = harness
        .send(post("/jobs/3/cancel", &cookie, None, vec![]))
        .await;
    assert_redirect(&response, "/jobs?error=job_not_found");
    assert!(harness.print.canceled().is_empty());

    let admin = harness.sign_in("root-subject", "root").await;
    let root = harness.user_id("root").await;
    users::set_administrator_from_command_line(&harness.database, root, true)
        .await
        .unwrap();
    let response = harness
        .send(post("/jobs/3/cancel", &admin, None, vec![]))
        .await;
    assert_redirect(&response, "/jobs");
    assert_eq!(harness.print.canceled(), [(3, "root".to_owned())]);
}

#[tokio::test]
async fn ownership_fails_closed() {
    let (harness, cookie) = private_jobs().await;
    // Users and sessions stay readable; only the records are gone.
    sea_orm::ConnectionTrait::execute_unprepared(&harness.database, "DROP TABLE print_jobs")
        .await
        .unwrap();
    let response = harness
        .send(post("/jobs/1/cancel", &cookie, None, vec![]))
        .await;
    assert_redirect(&response, "/jobs?error=unavailable");
    assert!(harness.print.canceled().is_empty());
}

/// A harness with the printers "open" and "lab"; "lab" is limited to the group "staff". The
/// plain user "alice" is in no group, "sam" is in "staff", "root" is an administrator in none.
async fn with_a_restricted_printer() -> Harness {
    let harness = Harness::with(Setup {
        printers: Some(vec![printer_named("open"), printer_named("lab")]),
        ..Setup::default()
    })
    .await;
    harness.member_of("sam-subject", "sam", &["staff"]).await;
    restrict_to_staff(&harness, DeviceKind::Printer, "lab").await;
    harness
}

fn printer_named(name: &str) -> Printer {
    Printer {
        name: name.into(),
        is_default: name == "open",
        ..office()
    }
}

async fn restrict_to_staff(harness: &Harness, kind: DeviceKind, device: &str) {
    let staff = groups::Entity::find()
        .filter(groups::Column::Name.eq("staff"))
        .one(&harness.database)
        .await
        .unwrap()
        .expect("the staff group exists once a member signed in")
        .id;
    access::set(&harness.database, kind, device, Access::Groups(vec![staff]))
        .await
        .unwrap();
}

impl Harness {
    /// Signs in a user of these groups (created at sign-in) and returns the session cookie.
    async fn member_of(&self, subject: &str, username: &str, groups: &[&str]) -> String {
        let identity = Identity {
            issuer: "https://id.test/".into(),
            subject: subject.into(),
            preferred_username: Some(username.into()),
            groups: Some(groups.iter().map(|group| (*group).to_owned()).collect()),
            ..Identity::default()
        };
        users::sign_in_with_identity(&self.database, &identity)
            .await
            .unwrap();
        self.sign_in(subject, username).await
    }
}

#[tokio::test]
async fn printing_to_a_restricted_printer_is_refused() {
    let harness = with_a_restricted_printer().await;
    let plain = harness.sign_in("alice-subject", "alice").await;
    let response = print(
        &harness,
        &plain,
        &[("printer", "lab")],
        Some(("a.pdf", PDF)),
    )
    .await;
    assert_redirect(&response, "/?error=printer_not_found");
    assert!(harness.print.submitted().is_empty());

    // The open printer still prints, and so do a member and an administrator on "lab".
    let response = print(
        &harness,
        &plain,
        &[("printer", "open")],
        Some(("a.pdf", PDF)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(location(&response).starts_with("/jobs?submitted="));
    let member = harness.sign_in("sam-subject", "sam").await;
    let admin = harness.administrator("root-subject", "root").await;
    for cookie in [member, admin] {
        let response = print(
            &harness,
            &cookie,
            &[("printer", "lab")],
            Some(("a.pdf", PDF)),
        )
        .await;
        assert!(location(&response).starts_with("/jobs?submitted="));
    }
}

#[tokio::test]
async fn printing_to_a_restricted_printer_in_another_case_is_refused() {
    // CUPS finds printers whatever the case of their name, so "LAB" reaches "lab".
    let harness = with_a_restricted_printer().await;
    let plain = harness.sign_in("alice-subject", "alice").await;
    for name in ["LAB", "Lab"] {
        let response = print(&harness, &plain, &[("printer", name)], Some(("a.pdf", PDF))).await;
        assert_redirect(&response, "/?error=printer_not_found");
    }
    assert!(harness.print.submitted().is_empty());

    // A member printing to "LAB" reaches "lab" under its own name.
    let member = harness.sign_in("sam-subject", "sam").await;
    let response = print(
        &harness,
        &member,
        &[("printer", "LAB")],
        Some(("a.pdf", PDF)),
    )
    .await;
    assert!(location(&response).starts_with("/jobs?submitted="));
    assert_eq!(harness.print.submitted()[0].printer, "lab");
}

#[tokio::test]
async fn scanning_on_a_restricted_scanner_is_refused() {
    let harness = with_a_restricted_printer().await;
    restrict_to_staff(&harness, DeviceKind::Scanner, "mock:0").await;
    let plain = harness.sign_in("alice-subject", "alice").await;
    let response = harness
        .send(post(
            "/api/preview",
            &plain,
            Some("application/x-www-form-urlencoded"),
            b"id=mock%3A0".to_vec(),
        ))
        .await;
    assert!(text(response).await.contains("Scanner not found"));
    assert!(harness.scan.opened().is_empty());
    assert!(harness.scan.scan_requests().is_empty());

    let member = harness.sign_in("sam-subject", "sam").await;
    let response = harness
        .send(post(
            "/api/preview",
            &member,
            Some("application/x-www-form-urlencoded"),
            b"id=mock%3A0".to_vec(),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(harness.scan.scan_requests().len(), 1);
}

#[tokio::test]
async fn access_fails_closed() {
    let harness = with_a_restricted_printer().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    sea_orm::ConnectionTrait::execute_unprepared(
        &harness.database,
        "DROP TABLE device_access_groups",
    )
    .await
    .unwrap();
    sea_orm::ConnectionTrait::execute_unprepared(&harness.database, "DROP TABLE device_access")
        .await
        .unwrap();
    // The rules are unreadable, so neither the restricted printer nor the open one prints.
    for printer in ["lab", "open"] {
        let response = print(
            &harness,
            &cookie,
            &[("printer", printer)],
            Some(("a.pdf", PDF)),
        )
        .await;
        assert_redirect(&response, "/?error=unavailable");
    }
    assert!(harness.print.submitted().is_empty());
}

fn bureau() -> Printer {
    Printer {
        name: "Bureau / 2e étage".into(),
        ..office()
    }
}

impl Harness {
    /// Signs in a user of these groups, and returns the id of the first.
    async fn staff(&self, groups: &[&str]) -> i64 {
        users::sign_in_with_identity(
            &self.database,
            &Identity {
                issuer: "https://id.test/".into(),
                subject: "staff-subject".into(),
                preferred_username: Some("staff".into()),
                groups: Some(groups.iter().map(|group| (*group).to_owned()).collect()),
                ..Identity::default()
            },
        )
        .await
        .unwrap();
        groups::Entity::find()
            .filter(groups::Column::Name.eq(groups[0]))
            .one(&self.database)
            .await
            .unwrap()
            .expect("a group")
            .id
    }

    async fn printer_rules(&self) -> std::collections::HashMap<String, Access> {
        access::rules(&self.database, DeviceKind::Printer)
            .await
            .unwrap()
    }
}

async fn device_harness() -> (Harness, String, i64) {
    let harness = Harness::with(Setup {
        printers: Some(vec![bureau()]),
        ..Setup::default()
    })
    .await;
    let admin = harness.administrator("admin-subject", "admin").await;
    let staff = harness.staff(&["staff"]).await;
    (harness, admin, staff)
}

#[tokio::test]
async fn an_administrator_restricts_a_printer() {
    let (harness, admin, staff) = device_harness().await;
    let staff = staff.to_string();
    let response = harness
        .send(form(
            "/administration/printers/access",
            Some(&admin),
            &[
                ("device", "Bureau / 2e étage"),
                ("access", "groups"),
                ("groups", &staff),
            ],
        ))
        .await;
    assert_redirect(&response, "/administration/printers?done=access");
    let rules = harness.printer_rules().await;
    assert_eq!(rules.len(), 1);
    assert_eq!(
        rules["Bureau / 2e étage"],
        Access::Groups(vec![staff.parse().unwrap()])
    );
}

#[tokio::test]
async fn groups_with_none_selected_mean_administrators_only() {
    let (harness, admin, _) = device_harness().await;
    let response = harness
        .send(form(
            "/administration/scanners/access",
            Some(&admin),
            &[("device", "net:scan-1"), ("access", "groups")],
        ))
        .await;
    assert_redirect(&response, "/administration/scanners?done=access");
    let rules = access::rules(&harness.database, DeviceKind::Scanner)
        .await
        .unwrap();
    assert_eq!(rules["net:scan-1"], Access::Groups(vec![]));
    assert!(harness.printer_rules().await.is_empty());
}

#[tokio::test]
async fn everyone_removes_the_rule() {
    let (harness, admin, staff) = device_harness().await;
    access::set(
        &harness.database,
        DeviceKind::Printer,
        "Bureau / 2e étage",
        Access::Groups(vec![staff]),
    )
    .await
    .unwrap();
    let response = harness
        .send(form(
            "/administration/printers/access",
            Some(&admin),
            &[("device", "Bureau / 2e étage"), ("access", "everyone")],
        ))
        .await;
    assert_redirect(&response, "/administration/printers?done=access");
    assert!(harness.printer_rules().await.is_empty());
}

#[tokio::test]
async fn an_unknown_group_is_refused() {
    let (harness, admin, _) = device_harness().await;
    for fields in [
        vec![
            ("device", "Bureau / 2e étage"),
            ("access", "groups"),
            ("groups", "99999"),
        ],
        vec![("device", "  "), ("access", "everyone")],
    ] {
        let response = harness
            .send(form(
                "/administration/printers/access",
                Some(&admin),
                &fields,
            ))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let html = text(response).await;
        assert!(html.contains("role=\"alert\""), "{html}");
        assert!(harness.printer_rules().await.is_empty());
    }
}

#[tokio::test]
async fn a_rule_for_a_missing_device_can_be_forgotten() {
    let (harness, admin, _) = device_harness().await;
    access::set(
        &harness.database,
        DeviceKind::Printer,
        "gone",
        Access::Groups(vec![]),
    )
    .await
    .unwrap();
    let response = harness
        .send(form(
            "/administration/printers/forget",
            Some(&admin),
            &[("device", "gone")],
        ))
        .await;
    assert_redirect(&response, "/administration/printers?done=forgotten");
    assert!(harness.printer_rules().await.is_empty());
}

#[tokio::test]
async fn device_forms_need_an_administrator() {
    let (harness, _, staff) = device_harness().await;
    let plain = harness.sign_in("alice-subject", "alice").await;
    let staff = staff.to_string();
    access::set(
        &harness.database,
        DeviceKind::Printer,
        "gone",
        Access::Groups(vec![]),
    )
    .await
    .unwrap();
    for (uri, fields) in [
        (
            "/administration/printers/access",
            vec![
                ("device", "x"),
                ("access", "groups"),
                ("groups", &staff[..]),
            ],
        ),
        ("/administration/printers/forget", vec![("device", "gone")]),
        (
            "/administration/scanners/access",
            vec![("device", "x"), ("access", "groups")],
        ),
        ("/administration/scanners/forget", vec![("device", "x")]),
    ] {
        let response = harness.send(form(uri, Some(&plain), &fields)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
    }
    let rules = harness.printer_rules().await;
    assert_eq!(rules.len(), 1);
    assert!(rules.contains_key("gone"));
}

#[tokio::test]
async fn a_failed_recording_still_reports_success() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    sea_orm::ConnectionTrait::execute_unprepared(&harness.database, "DROP TABLE print_jobs")
        .await
        .unwrap();
    let response = print(
        &harness,
        &cookie,
        &[("printer", "office")],
        Some(("a.pdf", PDF)),
    )
    .await;
    // CUPS has the job: it prints whatever the records say.
    assert!(location(&response).starts_with("/jobs?submitted="));
    assert_eq!(harness.print.submitted().len(), 1);
}

#[tokio::test]
async fn the_job_list_fails_closed_with_private_jobs() {
    let (harness, cookie) = private_jobs().await;
    let admin = harness.administrator("root-subject", "root").await;
    sea_orm::ConnectionTrait::execute_unprepared(&harness.database, "DROP TABLE print_jobs")
        .await
        .unwrap();
    let response = harness.send(api("list_jobs", &cookie)).await;
    assert!(!response.status().is_success());
    let body = text(response).await;
    assert!(
        body.contains("Something went wrong. Try again in a moment."),
        "{body}"
    );
    assert!(!body.contains("document-1.pdf"));

    // Administrators see every job anyway, so the records only name the owners.
    let response = harness.send(api("list_jobs", &admin)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await;
    for id in 1..=3 {
        assert!(body.contains(&format!("document-{id}.pdf")), "{body}");
    }
}

#[tokio::test]
async fn device_lists_fail_closed() {
    let harness = with_a_restricted_printer().await;
    let plain = harness.sign_in("alice-subject", "alice").await;
    let admin = harness.administrator("root-subject", "root").await;
    for table in ["device_access_groups", "device_access"] {
        sea_orm::ConnectionTrait::execute_unprepared(
            &harness.database,
            &format!("DROP TABLE {table}"),
        )
        .await
        .unwrap();
    }
    let list_scanners = |cookie: &str| {
        post(
            "/api/list_scanners",
            cookie,
            Some("application/x-www-form-urlencoded"),
            b"refresh=false".to_vec(),
        )
    };
    for request in [api("list_printers", &plain), list_scanners(&plain)] {
        let response = harness.send(request).await;
        assert!(!response.status().is_success());
        let body = text(response).await;
        assert!(
            body.contains("Something went wrong. Try again in a moment."),
            "{body}"
        );
        assert!(
            !body.contains("mock:0") && !body.contains("\"open\""),
            "{body}"
        );
    }

    // Administrators may use everything, so nothing is read for them.
    let response = harness.send(api("list_printers", &admin)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text(response).await.contains("lab"));
    let response = harness.send(list_scanners(&admin)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text(response).await.contains("mock:0"));
}

#[tokio::test]
async fn a_rule_keeps_every_ticked_group() {
    let (harness, admin, staff) = device_harness().await;
    let lab = harness.staff(&["staff", "lab"]).await;
    assert_eq!(lab, staff);
    let lab = groups::Entity::find()
        .filter(groups::Column::Name.eq("lab"))
        .one(&harness.database)
        .await
        .unwrap()
        .unwrap()
        .id;
    let (first, second) = (staff.to_string(), lab.to_string());
    let response = harness
        .send(form(
            "/administration/printers/access",
            Some(&admin),
            &[
                ("device", "Bureau / 2e étage"),
                ("access", "groups"),
                ("groups", &first),
                ("groups", &second),
            ],
        ))
        .await;
    assert_redirect(&response, "/administration/printers?done=access");
    let mut expected = vec![staff, lab];
    expected.sort_unstable();
    assert_eq!(
        harness.printer_rules().await["Bureau / 2e étage"],
        Access::Groups(expected)
    );
}

#[tokio::test]
async fn the_printers_page_lists_the_rules_when_cups_is_down() {
    let (harness, admin, staff) = device_harness().await;
    harness.print.set_unavailable("stopped");
    let response = harness
        .send(page("/administration/printers", Some(&admin)))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("CUPS did not answer"), "{html}");
    // No rule and no listing: CUPS may well have printers, so the page does not say it has none.
    assert!(!html.contains("No printers"), "{html}");

    access::set(
        &harness.database,
        DeviceKind::Printer,
        "Bureau / 2e étage",
        Access::Groups(vec![staff]),
    )
    .await
    .unwrap();
    let response = harness
        .send(page("/administration/printers", Some(&admin)))
        .await;
    let html = text(response).await;
    assert!(html.contains("CUPS did not answer"), "{html}");
    assert!(html.contains("Bureau / 2e étage"), "{html}");
    assert!(html.contains("Groups: staff"), "{html}");
}

async fn managed_harness() -> (Harness, String) {
    let harness = Harness::with(Setup {
        managed: true,
        ..Setup::default()
    })
    .await;
    let admin = harness.administrator("admin-subject", "admin").await;
    (harness, admin)
}

fn add_form<'a>(name: &'a str, address: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("name", name),
        ("address", address),
        ("driver", "everywhere"),
    ]
}

#[tokio::test]
async fn an_administrator_adds_an_ipp_everywhere_printer() {
    let (harness, admin) = managed_harness().await;
    let response = harness
        .send(form(
            "/administration/printers/add",
            Some(&admin),
            &[
                ("name", "lab"),
                ("address", "ipp://192.168.1.20/ipp/print"),
                ("driver", "everywhere"),
                ("description", "Lab laser"),
                ("location", ""),
            ],
        ))
        .await;
    assert_redirect(&response, "/administration/printers?done=added");
    assert_eq!(
        harness.admin.calls(),
        vec![AdminCall::Add(recto_printing::NewPrinter {
            name: "lab".into(),
            device_uri: "ipp://192.168.1.20/ipp/print".into(),
            driver: recto_printing::Driver::Everywhere,
            description: Some("Lab laser".into()),
            location: None,
        })]
    );

    let response = harness
        .send(form(
            "/administration/printers/add",
            Some(&admin),
            &[
                ("name", "old"),
                ("address", "socket://192.168.1.21:9100"),
                ("driver", "drv:///hp/laserjet.ppd"),
            ],
        ))
        .await;
    assert_redirect(&response, "/administration/printers?done=added");
    let calls = harness.admin.calls();
    let Some(AdminCall::Add(printer)) = calls.last() else {
        panic!("an add");
    };
    assert_eq!(
        printer.driver,
        recto_printing::Driver::Ppd("drv:///hp/laserjet.ppd".into())
    );
}

#[tokio::test]
async fn bad_names_and_addresses_are_refused() {
    let (harness, admin) = managed_harness().await;
    let long = "a".repeat(128);
    const NAME: &str = "The name must";
    const ADDRESS: &str = "The address must";
    let cases = [
        ("my printer", "ipp://192.168.1.20/ipp/print", NAME),
        ("a/b", "ipp://192.168.1.20/ipp/print", NAME),
        ("", "ipp://192.168.1.20/ipp/print", NAME),
        (long.as_str(), "ipp://192.168.1.20/ipp/print", NAME),
        // Other actions take CUPS's wider rule, but a new printer gets a plain name.
        ("Büro", "ipp://192.168.1.20/ipp/print", NAME),
        ("lab", "192.168.1.20", ADDRESS),
        ("lab", "file:///etc/passwd", ADDRESS),
        ("lab", "cups-pdf:/", ADDRESS),
    ];
    for (name, address, message) in cases {
        let response = harness
            .send(form(
                "/administration/printers/add",
                Some(&admin),
                &add_form(name, address),
            ))
            .await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{name} {address}"
        );
        let html = text(response).await;
        assert!(html.contains("role=\"alert\""), "{html}");
        assert!(html.contains(message), "{name} {address}: {html}");
    }
    assert!(harness.admin.calls().is_empty());
}

#[tokio::test]
async fn actions_take_any_name_cups_accepts() {
    let (harness, admin) = managed_harness().await;
    // Queues made with lpadmin may have names the Add form would not give.
    for name in ["Büro", "HP+", "lab@home"] {
        let response = harness
            .send(form(
                "/administration/printers/pause",
                Some(&admin),
                &[("printer", name)],
            ))
            .await;
        assert_redirect(&response, "/administration/printers?done=paused");
        assert_eq!(
            harness.admin.calls().last(),
            Some(&AdminCall::Pause(name.into()))
        );
    }
    let long = "é".repeat(64);
    for name in [
        "a/b",
        "a b",
        "a#b",
        "a?b",
        "a'b",
        "a\"b",
        "a\\b",
        "a\tb",
        long.as_str(),
    ] {
        let response = harness
            .send(form(
                "/administration/printers/pause",
                Some(&admin),
                &[("printer", name)],
            ))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{name:?}");
        assert!(text(response).await.contains("The name must"), "{name:?}");
    }
    let response = harness
        .send(form(
            "/administration/printers/pause",
            Some(&admin),
            &[("printer", "")],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(harness.admin.calls().len(), 3);
}

#[tokio::test]
async fn descriptions_and_locations_are_bounded_plain_text() {
    let (harness, admin) = managed_harness().await;
    let longest = "a".repeat(1023);
    let too_long = "a".repeat(1024);
    for (description, location) in [
        (too_long.as_str(), ""),
        ("", too_long.as_str()),
        ("Laser\nprinter", ""),
        ("", "Study\u{7}"),
    ] {
        let response = harness
            .send(form(
                "/administration/printers/describe",
                Some(&admin),
                &[
                    ("printer", "office"),
                    ("description", description),
                    ("location", location),
                ],
            ))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(
            text(response).await.contains("at most 1023 characters"),
            "{description:?} {location:?}"
        );

        let mut fields = add_form("lab", "ipp://192.168.1.20/ipp/print");
        fields.push(("description", description));
        fields.push(("location", location));
        let response = harness
            .send(form("/administration/printers/add", Some(&admin), &fields))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let html = text(response).await;
        assert!(html.contains("at most 1023 characters"), "{html}");
        // What was typed is kept.
        assert!(html.contains("ipp://192.168.1.20/ipp/print"), "{html}");
    }
    assert!(harness.admin.calls().is_empty());

    // Characters, not bytes: 1023 accented letters are fine.
    let accented = "é".repeat(1023);
    for text in [longest.as_str(), accented.as_str()] {
        let response = harness
            .send(form(
                "/administration/printers/describe",
                Some(&admin),
                &[
                    ("printer", "office"),
                    ("description", text),
                    ("location", text),
                ],
            ))
            .await;
        assert_redirect(&response, "/administration/printers?done=described");
    }
    assert_eq!(harness.admin.calls().len(), 2);
}

#[tokio::test]
async fn adding_never_replaces_a_printer() {
    let (harness, admin) = managed_harness().await;
    // CUPS finds queues whatever the case, so `Office` would replace `office`.
    let response = harness
        .send(form(
            "/administration/printers/add",
            Some(&admin),
            &add_form("Office", "ipp://192.168.1.20/ipp/print"),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let html = text(response).await;
    assert!(
        html.contains("A printer with this name already exists."),
        "{html}"
    );
    // What was typed is kept.
    assert!(html.contains("ipp://192.168.1.20/ipp/print"), "{html}");
    assert!(harness.admin.calls().is_empty());

    // Without the list, recto cannot tell, so it does not add.
    harness.print.set_unavailable("stopped");
    let response = harness
        .send(form(
            "/administration/printers/add",
            Some(&admin),
            &add_form("lab", "ipp://192.168.1.20/ipp/print"),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let html = text(response).await;
    assert!(html.contains("CUPS did not answer. Try again"), "{html}");
    assert!(harness.admin.calls().is_empty());
}

#[tokio::test]
async fn an_unreachable_printer_is_reported() {
    let (harness, admin) = managed_harness().await;
    harness
        .admin
        .fail_with(recto_printing::AdminError::Unreachable(
            "Unable to connect to 192.0.2.10:631".into(),
        ));
    let response = harness
        .send(form(
            "/administration/printers/add",
            Some(&admin),
            &add_form("lab", "ipp://192.0.2.10/ipp/print"),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let html = text(response).await;
    assert!(
        html.contains("The printer could not be reached: Unable to connect to 192.0.2.10:631"),
        "{html}"
    );
    // What was typed is kept.
    assert!(html.contains("ipp://192.0.2.10/ipp/print"), "{html}");
}

#[tokio::test]
async fn a_refused_administration_points_to_the_setup() {
    let (harness, admin) = managed_harness().await;
    harness.admin.fail_with(recto_printing::AdminError::Refused);
    let response = harness
        .send(form(
            "/administration/printers/pause",
            Some(&admin),
            &[("printer", "office")],
        ))
        .await;
    let html = text(response).await;
    assert!(html.contains("CUPS refused the change"), "{html}");
    assert!(html.contains("how-to/add-printer.html"), "{html}");
}

#[tokio::test]
async fn printer_actions_reach_cups() {
    let (harness, admin) = managed_harness().await;
    for (action, fields, done, call) in [
        (
            "describe",
            vec![
                ("printer", "office"),
                ("description", "Laser"),
                ("location", ""),
            ],
            "described",
            AdminCall::Describe {
                name: "office".into(),
                description: Some("Laser".into()),
                location: Some(String::new()),
            },
        ),
        (
            "default",
            vec![("printer", "office")],
            "default",
            AdminCall::SetDefault("office".into()),
        ),
        (
            "pause",
            vec![("printer", "office")],
            "paused",
            AdminCall::Pause("office".into()),
        ),
        (
            "resume",
            vec![("printer", "office")],
            "resumed",
            AdminCall::Resume("office".into()),
        ),
        (
            "accept",
            vec![("printer", "office")],
            "accepting",
            AdminCall::Accept("office".into()),
        ),
        (
            "reject",
            vec![("printer", "office")],
            "rejecting",
            AdminCall::Reject("office".into()),
        ),
    ] {
        let response = harness
            .send(form(
                &format!("/administration/printers/{action}"),
                Some(&admin),
                &fields,
            ))
            .await;
        assert_redirect(&response, &format!("/administration/printers?done={done}"));
        assert_eq!(harness.admin.calls().last(), Some(&call), "{action}");
    }
    assert_eq!(harness.admin.calls().len(), 6);

    let response = harness
        .send(form("/administration/printers/pause", Some(&admin), &[]))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(harness.admin.calls().len(), 6);

    // A name CUPS would not accept never reaches it.
    let response = harness
        .send(form(
            "/administration/printers/pause",
            Some(&admin),
            &[("printer", "../office")],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(harness.admin.calls().len(), 6);
}

#[tokio::test]
async fn removing_needs_confirmation_and_forgets_the_rule() {
    let (harness, admin) = managed_harness().await;
    access::set(
        &harness.database,
        DeviceKind::Printer,
        "office",
        Access::Groups(vec![]),
    )
    .await
    .unwrap();
    let response = harness
        .send(form(
            "/administration/printers/remove",
            Some(&admin),
            &[("printer", "office")],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let html = text(response).await;
    assert!(html.contains("Tick the confirmation"), "{html}");
    assert!(harness.admin.calls().is_empty());
    assert_eq!(harness.printer_rules().await.len(), 1);

    let response = harness
        .send(form(
            "/administration/printers/remove",
            Some(&admin),
            &[("printer", "office"), ("confirm", "yes")],
        ))
        .await;
    assert_redirect(&response, "/administration/printers?done=removed");
    assert_eq!(
        harness.admin.calls(),
        vec![AdminCall::Delete("office".into())]
    );
    assert!(harness.printer_rules().await.is_empty());
}

#[tokio::test]
async fn a_failed_removal_keeps_the_rule() {
    let (harness, admin) = managed_harness().await;
    access::set(
        &harness.database,
        DeviceKind::Printer,
        "office",
        Access::Groups(vec![]),
    )
    .await
    .unwrap();
    harness
        .admin
        .fail_with(recto_printing::AdminError::Rejected("Busy".into()));
    let response = harness
        .send(form(
            "/administration/printers/remove",
            Some(&admin),
            &[("printer", "office"), ("confirm", "yes")],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(harness.printer_rules().await.len(), 1);
}

const PRINTER_ROUTES: [&str; 8] = [
    "add", "describe", "default", "pause", "resume", "accept", "reject", "remove",
];

#[tokio::test]
async fn printer_management_needs_an_administrator() {
    let (harness, _) = managed_harness().await;
    let plain = harness.sign_in("alice-subject", "alice").await;
    for route in PRINTER_ROUTES {
        let response = harness
            .send(form(
                &format!("/administration/printers/{route}"),
                Some(&plain),
                &[
                    ("printer", "office"),
                    ("name", "lab"),
                    ("address", "ipp://192.168.1.20/ipp/print"),
                    ("driver", "everywhere"),
                    ("confirm", "yes"),
                ],
            ))
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{route}");
    }
    assert!(harness.admin.calls().is_empty());
}

#[tokio::test]
async fn printer_management_is_off_without_the_socket() {
    let harness = Harness::new().await;
    let admin = harness.administrator("admin-subject", "admin").await;
    for route in PRINTER_ROUTES {
        let response = harness
            .send(form(
                &format!("/administration/printers/{route}"),
                Some(&admin),
                &[
                    ("printer", "office"),
                    ("name", "lab"),
                    ("address", "ipp://192.168.1.20/ipp/print"),
                    ("driver", "everywhere"),
                    ("confirm", "yes"),
                ],
            ))
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{route}");
    }
    assert!(harness.admin.calls().is_empty());
}
