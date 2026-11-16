//! The pages of the interface and the server functions behind them, rendered through the router
//! with mock printing and scanning services and a fresh test database. Users are signed in by
//! writing their session into the router's session store, as in `tests/server.rs`.

use std::{path::Path, sync::Arc, time::Duration};

use axum::{
    Router,
    body::Body,
    http::{Request, Response, StatusCode, header},
};
use leptos::config::LeptosOptions;
use recto::session::SignInMethod;
use recto::session_store::DatabaseSessionStore;
use recto::{
    branding::{BrandOptions, Branding, default_branding},
    serve::{AuthState, router},
};
use recto_app::state::AppState;
use recto_authentication::{OpenId, OpenIdSettings};
use recto_domain::{
    testing::{TempDir, temporary_directory},
    users::{self, Identity},
};
use recto_printing::{
    Capabilities, ColorMode, Job, JobState, Printer, PrinterState, Sides, StateReason, Supply,
    testing::MockPrintService,
};
use recto_scanning::{MockScannerService, ResultStore, ScanError};
use sea_orm::DatabaseConnection;
use tower::ServiceExt;
use tower_sessions::Session;
use url::Url;

const PUBLIC_URL: &str = "http://recto.test/";
const ORIGIN: &str = "http://recto.test";

fn printer(name: &str, is_default: bool) -> Printer {
    Printer {
        name: name.into(),
        description: None,
        location: None,
        make_and_model: None,
        is_default,
        state: PrinterState::Idle,
        reasons: vec![],
        accepting_jobs: true,
        supplies: vec![],
        capabilities: Capabilities {
            max_copies: 10,
            sides: vec![Sides::OneSided],
            color_modes: vec![ColorMode::Monochrome],
        },
    }
}

/// The default printer: single-sided, black and white.
fn kitchen() -> Printer {
    printer("kitchen", true)
}

/// Offline, with duplex and color.
fn garage() -> Printer {
    Printer {
        state: PrinterState::Stopped,
        reasons: StateReason::parse("offline-report").into_iter().collect(),
        capabilities: Capabilities {
            max_copies: 10,
            sides: vec![Sides::OneSided, Sides::TwoSidedLongEdge],
            color_modes: vec![ColorMode::Color, ColorMode::Monochrome],
        },
        ..printer("garage", false)
    }
}

struct Harness {
    router: Router,
    scan: Arc<MockScannerService>,
    sessions: DatabaseSessionStore,
    database: DatabaseConnection,
    _dir: TempDir,
}

impl Harness {
    async fn new(printers: Vec<Printer>, jobs: Vec<Job>) -> Self {
        Self::branded(printers, jobs, default_branding()).await
    }

    async fn branded(printers: Vec<Printer>, jobs: Vec<Job>, branding: Branding) -> Self {
        let database = recto_domain::testing::database().await;
        let dir = temporary_directory();
        let print = Arc::new(
            MockPrintService::new()
                .with_printers(printers)
                .with_jobs(jobs),
        );
        let scan = Arc::new(MockScannerService::default());
        let site = dir.path().join("site");
        std::fs::create_dir_all(&site).unwrap();
        let state = AppState {
            print,
            scan: scan.clone(),
            results: Arc::new(ResultStore::new(
                dir.path().join("results"),
                Duration::from_secs(3600),
            )),
            output_dir: Some(dir.path().join("saved")),
            max_upload_size: 1024 * 1024,
            leptos_options: LeptosOptions::builder()
                .output_name("recto")
                .site_root(site.to_string_lossy().into_owned())
                .build(),
            database: database.clone(),
            branding: Arc::new(branding),
        };
        let public_url = Url::parse(PUBLIC_URL).unwrap();
        let openid = OpenId::new(OpenIdSettings {
            issuer: Url::parse("http://127.0.0.1:9/").unwrap(),
            client_id: "recto".into(),
            redirect_url: public_url.join("auth/callback").unwrap(),
            extra_scopes: vec![],
            required_claim: None,
            required_value: None,
            administrator_claim: None,
            administrator_value: None,
            groups_claim: None,
        });
        let auth = AuthState::new(Some(Arc::new(openid)), public_url, database.clone());
        let sessions = auth.sessions.clone();
        Self {
            router: router(state, auth),
            scan,
            sessions,
            database,
            _dir: dir,
        }
    }

    async fn send(&self, request: Request<Body>) -> Response<Body> {
        self.router.clone().oneshot(request).await.unwrap()
    }

    /// Signs a user in and returns the session cookie to send back.
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

    /// The HTML of a page for a signed-in user.
    async fn page(&self, uri: &str, cookie: &str) -> String {
        let request = Request::get(uri)
            .header(header::ACCEPT, "text/html")
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap();
        let response = self.send(request).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        text(response).await
    }

    /// Calls a server function the way an island does.
    async fn call(
        &self,
        endpoint: &str,
        cookie: &str,
        content_type: &str,
        body: String,
    ) -> Response<Body> {
        let request = Request::post(format!("/api/{endpoint}"))
            .header(header::COOKIE, cookie)
            .header(header::ORIGIN, ORIGIN)
            .header("sec-fetch-site", "same-origin")
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body))
            .unwrap();
        self.send(request).await
    }
}

async fn text(response: Response<Body>) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn job(id: i32, owner: Option<&str>, state: JobState) -> Job {
    Job {
        id,
        printer: "kitchen".into(),
        name: Some(format!("document-{id}.pdf")),
        owner: owner.map(str::to_owned),
        state,
        pages: Some(3),
        created: Some(1_700_000_000),
    }
}

#[tokio::test]
async fn print_page_lists_printers_and_supported_options() {
    let harness = Harness::new(vec![garage(), kitchen()], vec![]).await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let html = harness.page("/", &cookie).await;

    // A plain multipart form posting to the print route.
    assert!(html.contains(r#"action="/print""#), "{html}");
    assert!(html.contains(r#"enctype="multipart/form-data""#), "{html}");
    assert!(html.contains(r#"accept=".pdf,.jpg,.jpeg,.png""#), "{html}");
    // Both printers, the default one preselected and the offline one marked.
    assert!(
        html.contains(r#"<option value="kitchen" selected"#),
        "{html}"
    );
    assert!(html.contains(r#"<option value="garage""#), "{html}");
    assert!(html.contains("garage (offline)"), "{html}");
    // The default printer has neither duplex nor color: no field offers them.
    assert!(html.contains(r#"name="copies""#), "{html}");
    assert!(html.contains(r#"name="page_ranges""#), "{html}");
    assert!(html.contains("e.g. 1-3, 5"), "{html}");
    assert!(!html.contains(r#"name="sides""#), "{html}");
    assert!(!html.contains(r#"name="color_mode""#), "{html}");

    // A default printer with duplex and color offers both.
    let harness = Harness::new(
        vec![
            Printer {
                is_default: true,
                ..garage()
            },
            printer("kitchen", false),
        ],
        vec![],
    )
    .await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let html = harness.page("/", &cookie).await;
    assert!(html.contains(r#"name="sides""#), "{html}");
    assert!(html.contains(r#"value="two-sided-long-edge""#), "{html}");
    assert!(!html.contains(r#"value="two-sided-short-edge""#), "{html}");
    assert!(html.contains(r#"name="color_mode""#), "{html}");
    // The status card of the selected printer.
    assert!(html.contains("Offline"), "{html}");
}

#[tokio::test]
async fn jobs_page_shows_another_user_for_hidden_owner() {
    let harness = Harness::new(
        vec![kitchen()],
        vec![
            job(7, None, JobState::Processing),
            job(8, Some("bob"), JobState::Completed),
        ],
    )
    .await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let html = harness.page("/jobs?submitted=7", &cookie).await;

    assert!(html.contains("another user"), "{html}");
    assert!(html.contains("bob"), "{html}");
    assert!(html.contains("document-7.pdf"), "{html}");
    // Only active jobs can be canceled, through a plain form.
    assert!(html.contains(r#"action="/jobs/7/cancel""#), "{html}");
    assert!(!html.contains(r#"action="/jobs/8/cancel""#), "{html}");
    assert!(html.contains("Sent to the printer as job 7."), "{html}");

    let html = harness.page("/jobs?error=job_not_found", &cookie).await;
    assert!(html.contains("That job no longer exists."), "{html}");
}

#[tokio::test]
async fn printers_page_shows_supply_levels() {
    let supplies = vec![
        Supply {
            name: "Black toner".into(),
            level: Some(80),
            color: Some("#000000".into()),
            kind: "toner".into(),
        },
        Supply {
            name: "Waste toner".into(),
            level: None,
            color: None,
            kind: "waste-toner".into(),
        },
    ];
    let office = Printer {
        supplies,
        reasons: StateReason::parse("toner-low-warning")
            .into_iter()
            .collect(),
        ..kitchen()
    };
    let harness = Harness::new(vec![office, garage()], vec![]).await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let html = harness.page("/printers", &cookie).await;

    assert!(
        html.contains("kitchen") && html.contains("garage"),
        "{html}"
    );
    assert!(html.contains("Black toner"), "{html}");
    assert!(html.contains("80%"), "{html}");
    assert!(html.contains("width:80%"), "{html}");
    assert!(html.contains("background-color:#000000"), "{html}");
    assert!(html.contains("Unknown level"), "{html}");
    assert!(html.contains("Toner low"), "{html}");
    assert!(html.contains("Offline"), "{html}");
}

#[tokio::test]
async fn scan_page_renders_noscript_notice() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let html = harness.page("/scan", &cookie).await;

    assert!(html.contains("<noscript>"), "{html}");
    assert!(html.contains("Scanning needs JavaScript"), "{html}");
    assert!(html.contains("<leptos-island"), "{html}");
    assert!(html.contains("Mock scanner"), "{html}");
}

/// Every key of the English locale, as `a.b.c`.
fn locale_keys() -> Vec<String> {
    fn walk(prefix: &str, value: &serde_json::Value, keys: &mut Vec<String>) {
        if let serde_json::Value::Object(map) = value {
            for (name, child) in map {
                let key = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}.{name}")
                };
                walk(&key, child, keys);
                keys.push(key);
            }
        }
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/locales/en.json");
    let locale: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut keys = Vec::new();
    walk("", &locale, &mut keys);
    // Single words ("app") can appear in any text; dotted keys cannot.
    keys.retain(|key| key.contains('.'));
    keys
}

#[tokio::test]
async fn pages_have_no_missing_translations() {
    let harness = Harness::new(
        vec![kitchen(), garage()],
        vec![job(7, None, JobState::Processing)],
    )
    .await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let keys = locale_keys();
    assert!(keys.iter().any(|key| key == "print.errors.too_large"));
    for uri in [
        "/",
        "/?error=too_large",
        "/printers",
        "/jobs",
        "/jobs?submitted=7",
        "/scan",
        "/no-access",
    ] {
        let html = harness.page(uri, &cookie).await;
        assert!(!html.contains("[missing"), "{uri}: {html}");
        for key in &keys {
            assert!(
                !html.contains(key.as_str()),
                "{uri} shows the raw key {key}"
            );
        }
    }
}

#[tokio::test]
async fn layout_names_the_user_and_marks_the_current_page() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let html = harness.page("/printers", &cookie).await;

    assert!(html.contains("alice"), "{html}");
    assert!(html.contains(r#"action="/auth/logout""#), "{html}");
    for target in ["/", "/printers", "/jobs", "/scan"] {
        assert!(html.contains(&format!(r#"href="{target}""#)), "{target}");
    }
    assert!(html.contains(r#"aria-current="page""#), "{html}");
}

#[tokio::test]
async fn preview_and_scan_store_results_for_the_user() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let alice = harness.sign_in("alice-subject", "alice").await;
    let bob = harness.sign_in("bob-subject", "bob").await;

    let response = harness
        .call(
            "preview",
            &alice,
            "application/x-www-form-urlencoded",
            "id=mock%3A0".into(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let outcome: serde_json::Value = serde_json::from_str(&text(response).await).unwrap();
    assert!(outcome["file_name"].as_str().unwrap().ends_with(".png"));
    assert_eq!(outcome["pages"], 1);
    assert_eq!(outcome["can_save"], true);
    let image = format!("/scan/results/{}", outcome["id"].as_str().unwrap());
    let get = |cookie: &str| {
        Request::get(&image)
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap()
    };
    let response = harness.send(get(&alice)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(
        harness.send(get(&bob)).await.status(),
        StatusCode::NOT_FOUND
    );

    // The selection, as fractions of the preview, becomes millimetres of the scanner's area.
    let body = serde_json::json!({
        "id": "mock:0",
        "settings": { "resolution": 150, "mode": "Gray", "source": "Flatbed", "area": null },
        "selection": { "x": 0.5, "y": 0.0, "width": 0.5, "height": 0.5 },
        "format": "Pdf",
    });
    let response = harness
        .call("scan", &alice, "application/json", body.to_string())
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let outcome: serde_json::Value = serde_json::from_str(&text(response).await).unwrap();
    assert!(outcome["file_name"].as_str().unwrap().ends_with(".pdf"));
    let (_, request) = harness.scan.scan_requests().pop().unwrap();
    assert!(!request.single_page);
    let settings = request.settings;
    assert_eq!(settings.resolution, 150);
    assert_eq!(settings.source.as_deref(), Some("Flatbed"));
    let area = settings.area.unwrap();
    assert!((area.left - 107.95).abs() < 0.01, "{area:?}");
    assert!((area.bottom - 148.5).abs() < 0.01, "{area:?}");
}

#[tokio::test]
async fn scan_errors_are_readable() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let form = "application/x-www-form-urlencoded";

    harness.scan.fail_with(Some(ScanError::Busy));
    let response = harness
        .call("preview", &cookie, form, "id=mock%3A0".into())
        .await;
    assert!(!response.status().is_success());
    assert!(
        text(response)
            .await
            .contains("The scanner is busy. Try again in a moment.")
    );

    harness.scan.fail_with(None);
    let response = harness
        .call("preview", &cookie, form, "id=unplugged".into())
        .await;
    assert!(text(response).await.contains("Scanner not found"));

    // A setting the scanner does not offer is refused before scanning.
    let body = serde_json::json!({
        "id": "mock:0",
        "settings": { "resolution": 123, "mode": "Gray", "source": null, "area": null },
        "selection": null,
        "format": "Png",
    });
    let response = harness
        .call("scan", &cookie, "application/json", body.to_string())
        .await;
    assert!(
        text(response)
            .await
            .contains("The scanner does not support this")
    );
    assert!(harness.scan.scan_requests().is_empty());
}

#[tokio::test]
async fn unlisted_scanner_ids_are_never_opened() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let form = "application/x-www-form-urlencoded";
    // SANE's `net` backend would connect to any host named in an id it does not know.
    let foreign = "net%3A203.0.113.9%3Atest%3A0";
    for (endpoint, body) in [
        ("scanner_capabilities", format!("id={foreign}")),
        ("preview", format!("id={foreign}")),
    ] {
        let response = harness.call(endpoint, &cookie, form, body).await;
        assert!(
            text(response).await.contains("Scanner not found"),
            "{endpoint}"
        );
    }
    let body = serde_json::json!({
        "id": "net:203.0.113.9:test:0",
        "settings": { "resolution": 150, "mode": "Gray", "source": null, "area": null },
        "selection": null,
        "format": "Pdf",
    });
    let response = harness
        .call("scan", &cookie, "application/json", body.to_string())
        .await;
    assert!(text(response).await.contains("Scanner not found"));
    assert!(
        harness.scan.opened().is_empty(),
        "{:?}",
        harness.scan.opened()
    );

    // A listed scanner still opens.
    let response = harness
        .call("scanner_capabilities", &cookie, form, "id=mock%3A0".into())
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(harness.scan.opened(), ["mock:0"]);
}

#[tokio::test]
async fn preview_reads_one_page_from_the_chosen_source() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let form = "application/x-www-form-urlencoded";
    let response = harness
        .call("preview", &cookie, form, "id=mock%3A0&source=ADF".into())
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let (_, request) = harness.scan.scan_requests().pop().unwrap();
    assert_eq!(request.settings.source.as_deref(), Some("ADF"));
    assert!(request.single_page, "a preview never pulls a stack through");

    // A source the scanner does not have is refused before scanning.
    let response = harness
        .call("preview", &cookie, form, "id=mock%3A0&source=Tray".into())
        .await;
    assert!(
        text(response)
            .await
            .contains("The scanner does not support this")
    );
    assert_eq!(harness.scan.scan_requests().len(), 1);
}

/// The HTML of a page for a signed-in user, sent with an extra cookie.
async fn page_with_theme(harness: &Harness, uri: &str, session: &str, theme: &str) -> String {
    let request = Request::get(uri)
        .header(header::ACCEPT, "text/html")
        .header(header::COOKIE, format!("{session}; recto_theme={theme}"))
        .body(Body::empty())
        .unwrap();
    let response = harness.send(request).await;
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    text(response).await
}

#[tokio::test]
async fn a_chosen_theme_is_rendered_by_the_server() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let html = page_with_theme(&harness, "/jobs", &cookie, "dark").await;
    assert!(html.contains(r#"data-theme="dark""#), "{html}");
    assert!(html.contains(r#"<meta name="color-scheme" content="dark""#));
    for other in ["Follow the device", "Light"] {
        let label = format!(r#"aria-label="{other}""#);
        let at = html.find(&label).unwrap();
        let button = &html[html[..at].rfind("<button").unwrap()..];
        assert!(
            button[..button.find('>').unwrap()].contains(r#"aria-pressed="false""#),
            "{other} is not pressed"
        );
    }
    assert!(
        html.contains(r#"aria-label="Dark""#) && {
            let at = html.find(r#"aria-label="Dark""#).unwrap();
            let button = &html[html[..at].rfind("<button").unwrap()..];
            button[..button.find('>').unwrap()].contains(r#"aria-pressed="true""#)
        },
        "the Dark button is pressed"
    );
}

#[tokio::test]
async fn a_junk_theme_cookie_renders_the_system_theme() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let html = page_with_theme(&harness, "/jobs", &cookie, "purple").await;
    let root = &html[html.find("<html").unwrap()..];
    assert!(
        !root[..root.find('>').unwrap()].contains("data-theme"),
        "{html}"
    );
    assert!(html.contains(r#"<meta name="color-scheme" content="light dark""#));
}

#[tokio::test]
async fn the_sign_in_page_has_the_switch() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let html = sign_in_page(&harness).await;
    assert!(html.contains(r#"action="/theme""#), "{html}");
}

/// The branding of `options` over the defaults.
fn branding(change: impl FnOnce(&mut BrandOptions)) -> Branding {
    let mut options = BrandOptions::default();
    change(&mut options);
    recto::branding::load(&options).unwrap()
}

/// The sign-in page for an anonymous visitor. A local account must exist, or the page sends
/// visitors to single sign-on.
async fn sign_in_page(harness: &Harness) -> String {
    users::create_local(
        &harness.database,
        users::NewLocalUser {
            username: "bob".into(),
            display_name: String::new(),
            email: None,
            password: "correct horse battery".into(),
            is_administrator: false,
        },
        &recto_domain::passwords::PasswordPolicy { minimum_length: 12 },
    )
    .await
    .unwrap();
    let request = Request::get("/sign-in")
        .header(header::ACCEPT, "text/html")
        .body(Body::empty())
        .unwrap();
    let response = harness.send(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    text(response).await
}

#[tokio::test]
async fn the_brand_name_is_in_the_header_and_titles() {
    let harness = Harness::branded(
        vec![kitchen()],
        vec![],
        branding(|o| {
            o.brand_name = "Maison".into();
            o.brand_short_name = Some("Mai".into());
        }),
    )
    .await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let html = harness.page("/printers", &cookie).await;
    let header = &html[html.find("<header").unwrap()..html.find("</header>").unwrap()];
    assert!(header.contains("Maison"), "{header}");
    assert!(html.contains("<title>Printers · Maison</title>"), "{html}");
    assert!(html.contains(r#"<meta name="application-name" content="Maison""#));
    assert!(html.contains(r#"<meta name="apple-mobile-web-app-title" content="Mai""#));
    assert!(!html.contains("· recto"), "{html}");
    // Without a logo, the glyph stays.
    assert!(!html.contains(r#"src="/brand/logo""#), "{html}");
    assert!(header.contains(GLYPH), "{header}");
    assert!(html.contains(r#"<link rel="icon" href="/brand/icon-192.png""#));
    assert!(html.contains(r#"<link rel="apple-touch-icon" href="/brand/apple-touch-icon.png""#));
}

/// The start of the printer glyph's square, which a logo replaces.
const GLYPH: &str = r#"<span aria-hidden="true" class="bg-primary text-primary-foreground"#;

#[tokio::test]
async fn a_logo_replaces_the_glyph() {
    let logo = recto::branding::BrandFile {
        bytes: axum::body::Bytes::from_static(b"<svg/>"),
        content_type: "image/svg+xml",
    };
    let harness = Harness::branded(
        vec![kitchen()],
        vec![],
        Branding {
            logo: Some(logo),
            ..default_branding()
        },
    )
    .await;
    let image = r#"<img src="/brand/logo" alt="""#;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let html = harness.page("/printers", &cookie).await;
    let header = &html[html.find("<header").unwrap()..html.find("</header>").unwrap()];
    assert!(header.contains(image), "{header}");
    assert!(!header.contains(GLYPH), "{header}");
    let sign_in = sign_in_page(&harness).await;
    assert!(sign_in.contains(image), "{sign_in}");
    assert!(!sign_in.contains(GLYPH), "{sign_in}");
}

#[tokio::test]
async fn an_accent_is_inlined_and_the_default_is_not() {
    let harness = Harness::branded(
        vec![kitchen()],
        vec![],
        branding(|o| o.brand_color = "#8A2BE2".into()),
    )
    .await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let html = harness.page("/printers", &cookie).await;
    let head = &html[..html.find("</head>").unwrap()];
    let style = head.find("<style>").expect("an inline style");
    assert!(head.find("/pkg/recto.css").unwrap() < style, "{head}");
    assert!(head[style..].contains("--primary:#8a2be2;"), "{head}");
    assert!(head.contains(r#":root[data-theme="dark"]{"#), "{head}");
    assert!(head.contains(r##"<meta name="theme-color" content="#8a2be2""##));

    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let html = harness.page("/printers", &cookie).await;
    assert!(!html.contains("<style"), "{html}");
    assert!(html.contains(r##"<meta name="theme-color" content="#0f6cbd""##));
}

#[tokio::test]
async fn the_sign_in_page_carries_the_brand() {
    let harness = Harness::branded(
        vec![kitchen()],
        vec![],
        branding(|o| o.brand_name = "Maison".into()),
    )
    .await;
    let html = sign_in_page(&harness).await;
    assert!(html.contains("<title>Sign in · Maison</title>"), "{html}");
    let main = &html[html.find("<main").unwrap()..];
    assert!(main.contains("Maison"), "{main}");
}

#[tokio::test]
async fn the_offline_page_is_public() {
    let harness = Harness::new(vec![], vec![]).await;
    let request = Request::get("/offline")
        .header(header::ACCEPT, "text/html")
        .body(Body::empty())
        .unwrap();
    let response = harness.send(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("You are offline"), "{html}");
    assert!(html.contains(r#"href="/""#), "{html}");
    assert!(!html.contains(r#"action="/theme""#), "{html}");
}

#[tokio::test]
async fn pages_link_the_manifest_and_worker() {
    let harness = Harness::new(vec![kitchen()], vec![]).await;
    let cookie = harness.sign_in("alice-subject", "alice").await;
    let html = harness.page("/printers", &cookie).await;
    assert!(
        html.contains(r#"<link rel="manifest" href="/manifest.webmanifest""#),
        "{html}"
    );
    assert!(html.contains(r#"src="/pwa.js""#), "{html}");
}
