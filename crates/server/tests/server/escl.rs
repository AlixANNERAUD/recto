//! recto's scanners over eSCL: scan apps scan with a device password, on the scanners its owner
//! may use, or without one from the networks the operator opened, on the LAN listener only; the
//! documents go back to the app, and every scan is logged.

use std::{
    collections::VecDeque,
    net::IpAddr,
    pin::Pin,
    task::{Context, Poll},
};

use axum::{body::Bytes, http::Method};
use chrono::Utc;
use recto_domain::device_passwords;
use recto_entity::scan_jobs;
use recto_scanning::{ScanError, escl};

use super::*;

/// Device requests from the server itself: the test server is plain HTTP, where only loopback
/// clients may send device passwords.
const LOOPBACK: [u8; 4] = [127, 0, 0, 1];
const MOCK_SCANNER: &str = "mock:0";

/// The mock scanner's eSCL root under `/eSCL/`.
fn root() -> String {
    format!("/eSCL/{}", escl::scanner_key(MOCK_SCANNER))
}

/// The mock scanner's eSCL root on a personal address.
fn personal_root(secret: &str) -> String {
    format!("/device/{secret}/eSCL/{}", escl::scanner_key(MOCK_SCANNER))
}

async fn device_secret(harness: &Harness, user: i64) -> String {
    device_passwords::create(&harness.database, user, "Laptop", Utc::now())
        .await
        .unwrap()
        .secret
}

/// A grey 75 dpi scan of the glass in `format`, from `source`.
fn settings(source: &str, format: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><scan:ScanSettings xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm" xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03"><pwg:Version>2.0</pwg:Version><pwg:InputSource>{source}</pwg:InputSource><scan:ColorMode>Grayscale8</scan:ColorMode><scan:DocumentFormatExt>{format}</scan:DocumentFormatExt><scan:XResolution>75</scan:XResolution><scan:YResolution>75</scan:YResolution></scan:ScanSettings>"#
    )
}

/// An eSCL request to `path`, with Basic credentials holding `secret` when there is one.
fn escl_request(method: Method, path: &str, secret: Option<&str>, body: Body) -> Request<Body> {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(secret) = secret {
        let credentials = data_encoding::BASE64.encode(format!("macbook:{secret}").as_bytes());
        request = request.header(header::AUTHORIZATION, format!("Basic {credentials}"));
    }
    request.body(body).unwrap()
}

fn from_ip(mut request: Request<Body>, address: IpAddr) -> Request<Body> {
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::new(address, 50_000)));
    request
}

fn get_escl(path: &str, secret: Option<&str>) -> Request<Body> {
    from_address(
        escl_request(Method::GET, path, secret, Body::empty()),
        LOOPBACK,
    )
}

fn post_job(root: &str, secret: Option<&str>, settings: String) -> Request<Body> {
    let mut request = escl_request(
        Method::POST,
        &format!("{root}/ScanJobs"),
        secret,
        Body::from(settings),
    );
    request
        .headers_mut()
        .insert(header::CONTENT_TYPE, "text/xml".parse().unwrap());
    from_address(request, LOOPBACK)
}

/// Creates a job and returns its path, which must start with `root`.
async fn create_job(harness: &Harness, root: &str, secret: Option<&str>, format: &str) -> String {
    let response = harness
        .send(post_job(root, secret, settings("Platen", format)))
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
    let job = location(&response);
    assert!(
        job.starts_with(&format!("{root}/ScanJobs/")),
        "{job} is not under {root}"
    );
    job
}

async fn body_bytes(response: Response<Body>) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

async fn scan_rows(harness: &Harness) -> Vec<scan_jobs::Model> {
    scan_jobs::Entity::find()
        .all(&harness.database)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_device_scans_a_pdf() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let secret = device_secret(&harness, alice).await;

    let capabilities = harness
        .send(get_escl(
            &format!("{}/ScannerCapabilities", root()),
            Some(&secret),
        ))
        .await;
    assert_eq!(capabilities.status(), StatusCode::OK);
    assert_eq!(capabilities.headers()[header::CONTENT_TYPE], "text/xml");
    assert_eq!(capabilities.headers()[header::CACHE_CONTROL], "no-cache");
    let xml = text(capabilities).await;
    assert!(
        xml.contains("<pwg:MakeAndModel>recto Recto Mock scanner</pwg:MakeAndModel>"),
        "{xml}"
    );

    let job = create_job(&harness, &root(), Some(&secret), "application/pdf").await;
    let next = harness
        .send(get_escl(&format!("{job}/NextDocument"), Some(&secret)))
        .await;
    assert_eq!(next.status(), StatusCode::OK);
    assert_eq!(next.headers()[header::CONTENT_TYPE], "application/pdf");
    assert_eq!(next.headers()[header::CACHE_CONTROL], "no-store");
    assert!(body_bytes(next).await.starts_with(b"%PDF"));
    let again = harness
        .send(get_escl(&format!("{job}/NextDocument"), Some(&secret)))
        .await;
    assert_eq!(again.status(), StatusCode::NOT_FOUND);

    let info = harness
        .send(get_escl(&format!("{job}/ScanImageInfo"), Some(&secret)))
        .await;
    assert_eq!(info.status(), StatusCode::OK);
    assert!(text(info).await.contains("<scan:ActualWidth>"));

    let status = harness
        .send(get_escl(
            &format!("{}/ScannerStatus", root()),
            Some(&secret),
        ))
        .await;
    assert_eq!(status.status(), StatusCode::OK);
    let xml = text(status).await;
    assert!(xml.contains("<pwg:State>Idle</pwg:State>"), "{xml}");
    assert!(
        xml.contains(&format!("<pwg:JobUri>{job}</pwg:JobUri>")),
        "{xml}"
    );
    assert!(
        xml.contains("<pwg:JobState>Completed</pwg:JobState>"),
        "{xml}"
    );

    let delete = harness
        .send(from_address(
            escl_request(Method::DELETE, &job, Some(&secret), Body::empty()),
            LOOPBACK,
        ))
        .await;
    assert_eq!(delete.status(), StatusCode::OK);
    let gone = harness
        .send(from_address(
            escl_request(Method::DELETE, &job, Some(&secret), Body::empty()),
            LOOPBACK,
        ))
        .await;
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);

    let rows = scan_rows(&harness).await;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.user_id, Some(alice));
    assert_eq!(row.username, "alice");
    assert_eq!(row.scanner, MOCK_SCANNER);
    assert_eq!(row.source, "escl");
    assert_eq!(row.format, "pdf");
    assert_eq!(row.pages, 1);
    assert_eq!(row.result_id, None);
}

#[tokio::test]
async fn feeder_pages_are_one_document_each() {
    let harness = Harness::with(Setup {
        scanner: Some(MockScannerService::default().with_feeder_pages(3)),
        ..Setup::default()
    })
    .await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let secret = device_secret(&harness, alice).await;

    let response = harness
        .send(post_job(
            &root(),
            Some(&secret),
            settings("Feeder", "image/png"),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let job = location(&response);
    for _ in 0..3 {
        let next = harness
            .send(get_escl(&format!("{job}/NextDocument"), Some(&secret)))
            .await;
        assert_eq!(next.status(), StatusCode::OK);
        assert_eq!(next.headers()[header::CONTENT_TYPE], "image/png");
    }
    let done = harness
        .send(get_escl(&format!("{job}/NextDocument"), Some(&secret)))
        .await;
    assert_eq!(done.status(), StatusCode::NOT_FOUND);
    let rows = scan_rows(&harness).await;
    assert_eq!((rows[0].pages, rows[0].format.as_str()), (3, "png"));
}

#[tokio::test]
async fn a_personal_scan_address_keeps_its_path() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let secret = device_secret(&harness, alice).await;
    let root = personal_root(&secret);

    let capabilities = harness
        .send(get_escl(&format!("{root}/ScannerCapabilities"), None))
        .await;
    assert_eq!(capabilities.status(), StatusCode::OK);

    let job = create_job(&harness, &root, None, "image/jpeg").await;
    let status = harness
        .send(get_escl(&format!("{root}/ScannerStatus"), None))
        .await;
    let xml = text(status).await;
    assert!(
        xml.contains(&format!("<pwg:JobUri>{job}</pwg:JobUri>")),
        "{xml}"
    );
    let next = harness
        .send(get_escl(&format!("{job}/NextDocument"), None))
        .await;
    assert_eq!(next.status(), StatusCode::OK);
    assert_eq!(next.headers()[header::CONTENT_TYPE], "image/jpeg");

    // Basic credentials see the same job on their own root, never on the personal one.
    let basic_status = harness
        .send(get_escl(
            &format!("{}/ScannerStatus", self::root()),
            Some(&secret),
        ))
        .await;
    let xml = text(basic_status).await;
    assert!(!xml.contains("/device/"), "{xml}");

    // A wrong secret is an address that does not exist.
    let wrong = personal_root("abcdefghijklmnopqrstuvwxyz234567");
    let response = harness
        .send(get_escl(&format!("{wrong}/ScannerCapabilities"), None))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(response.headers().get(header::LOCATION).is_none());
}

#[tokio::test]
async fn scanners_the_user_may_not_use_do_not_exist() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let secret = device_secret(&harness, alice).await;

    // Without credentials, a prompt.
    let anonymous = harness
        .send(get_escl(&format!("{}/ScannerCapabilities", root()), None))
        .await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    assert!(anonymous.headers().contains_key(header::WWW_AUTHENTICATE));

    // A scanner that is not listed.
    let unknown = harness
        .send(get_escl(
            "/eSCL/00000000/ScannerCapabilities",
            Some(&secret),
        ))
        .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    // A scanner for administrators only.
    access::set(
        &harness.database,
        DeviceKind::Scanner,
        MOCK_SCANNER,
        Access::Groups(vec![]),
    )
    .await
    .unwrap();
    for path in ["ScannerCapabilities", "ScannerStatus"] {
        let response = harness
            .send(get_escl(&format!("{}/{path}", root()), Some(&secret)))
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    let response = harness
        .send(post_job(
            &root(),
            Some(&secret),
            settings("Platen", "image/png"),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(harness.scan.scan_requests().is_empty());
}

#[tokio::test]
async fn open_networks_match_the_tcp_peer_only() {
    // Behind TLS, as in production: a refusal is a prompt, not a refusal of plain HTTP.
    let harness = Harness::with(Setup {
        escl_open_networks: "192.168.1.0/24",
        trust_forwarded_headers: true,
        public_url: "https://recto.test/",
        ..Setup::default()
    })
    .await;
    let inside: IpAddr = "192.168.1.20".parse().unwrap();
    let mapped: IpAddr = "::ffff:192.168.1.20".parse().unwrap();
    let outside: IpAddr = "203.0.113.9".parse().unwrap();
    let post = |address: IpAddr| {
        let mut request = escl_request(
            Method::POST,
            &format!("{}/ScanJobs", root()),
            None,
            Body::from(settings("Platen", "image/png")),
        );
        request
            .headers_mut()
            .insert("x-forwarded-for", "192.168.1.20".parse().unwrap());
        from_ip(request, address)
    };

    for address in [inside, mapped] {
        let response = harness.send_lan(post(address)).await;
        assert_eq!(response.status(), StatusCode::CREATED, "{address}");
        let job = location(&response);
        let next = harness
            .send_lan(from_ip(
                escl_request(
                    Method::GET,
                    &format!("{job}/NextDocument"),
                    None,
                    Body::empty(),
                ),
                address,
            ))
            .await;
        assert_eq!(next.status(), StatusCode::OK, "{address}");
    }
    let rows = scan_rows(&harness).await;
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .all(|row| row.user_id.is_none() && row.username.is_empty() && row.source == "escl")
    );

    // A forwarded header names a client inside, but the connection comes from outside.
    let forged = harness.send_lan(post(outside)).await;
    assert_eq!(forged.status(), StatusCode::UNAUTHORIZED);
    // The main listener sits behind the proxy: never anonymous.
    let main = harness.send(post(inside)).await;
    assert_eq!(main.status(), StatusCode::UNAUTHORIZED);

    // Only scanners open to everyone.
    let staff = groups::ActiveModel {
        name: sea_orm::ActiveValue::Set("staff".into()),
        created_at: sea_orm::ActiveValue::Set(Utc::now().fixed_offset()),
        ..Default::default()
    };
    let staff = sea_orm::ActiveModelTrait::insert(staff, &harness.database)
        .await
        .unwrap();
    access::set(
        &harness.database,
        DeviceKind::Scanner,
        MOCK_SCANNER,
        Access::Groups(vec![staff.id]),
    )
    .await
    .unwrap();
    let restricted = harness.send_lan(post(inside)).await;
    assert_eq!(restricted.status(), StatusCode::NOT_FOUND);
    assert_eq!(scan_rows(&harness).await.len(), 2);
}

#[tokio::test]
async fn an_empty_feeder_answers_404() {
    let harness = Harness::with(Setup {
        scanner: Some(MockScannerService::default().with_scan_delay(Duration::from_millis(300))),
        ..Setup::default()
    })
    .await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let secret = device_secret(&harness, alice).await;

    let response = harness
        .send(post_job(
            &root(),
            Some(&secret),
            settings("Feeder", "image/png"),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let job = location(&response);
    // The scan reads the feeder after its delay, and finds it empty. The failure would also
    // fail the listing of scanners, so it ends before the app asks for the document.
    harness.scan.fail_with(Some(ScanError::NoDocuments));
    tokio::time::sleep(Duration::from_millis(600)).await;
    harness.scan.fail_with(None);
    let next = harness
        .send(get_escl(&format!("{job}/NextDocument"), Some(&secret)))
        .await;
    assert_eq!(next.status(), StatusCode::NOT_FOUND);

    let status = harness
        .send(get_escl(
            &format!("{}/ScannerStatus", root()),
            Some(&secret),
        ))
        .await;
    let xml = text(status).await;
    assert!(
        xml.contains("<scan:AdfState>ScannerAdfEmpty</scan:AdfState>"),
        "{xml}"
    );
    assert!(scan_rows(&harness).await.is_empty());
}

#[tokio::test]
async fn a_busy_scanner_answers_503() {
    let harness = Harness::with(Setup {
        scanner: Some(MockScannerService::default().with_scan_delay(Duration::from_millis(300))),
        ..Setup::default()
    })
    .await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let secret = device_secret(&harness, alice).await;

    let first = harness
        .send(post_job(
            &root(),
            Some(&secret),
            settings("Platen", "image/png"),
        ))
        .await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let second = harness
        .send(post_job(
            &root(),
            Some(&secret),
            settings("Platen", "image/png"),
        ))
        .await;
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(second.headers().contains_key(header::RETRY_AFTER));
    assert_eq!(second.headers()[header::CACHE_CONTROL], "no-cache");

    let status = harness
        .send(get_escl(
            &format!("{}/ScannerStatus", root()),
            Some(&secret),
        ))
        .await;
    assert!(
        text(status)
            .await
            .contains("<pwg:State>Processing</pwg:State>")
    );
}

#[tokio::test]
async fn a_canceled_scan_keeps_its_scanner_busy() {
    let harness = Harness::with(Setup {
        scanner: Some(MockScannerService::default().with_scan_delay(Duration::from_millis(500))),
        ..Setup::default()
    })
    .await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let secret = device_secret(&harness, alice).await;

    // A job deleted as soon as it is created: the scanner is still scanning its page.
    let first = harness
        .send(post_job(
            &root(),
            Some(&secret),
            settings("Platen", "image/png"),
        ))
        .await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let job = first.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    let delete = harness
        .send(from_address(
            escl_request(Method::DELETE, &job, Some(&secret), Body::empty()),
            LOOPBACK,
        ))
        .await;
    assert_eq!(delete.status(), StatusCode::OK);
    let second = harness
        .send(post_job(
            &root(),
            Some(&secret),
            settings("Platen", "image/png"),
        ))
        .await;
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    let status = harness
        .send(get_escl(
            &format!("{}/ScannerStatus", root()),
            Some(&secret),
        ))
        .await;
    assert!(
        text(status)
            .await
            .contains("<pwg:State>Processing</pwg:State>")
    );

    // Once the scan is back, the scanner takes the next job.
    let started = std::time::Instant::now();
    loop {
        let next = harness
            .send(post_job(
                &root(),
                Some(&secret),
                settings("Platen", "image/png"),
            ))
            .await;
        if next.status() == StatusCode::CREATED {
            break;
        }
        assert_eq!(next.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(started.elapsed() < Duration::from_secs(5));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn settings_are_read_within_bounds() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let secret = device_secret(&harness, alice).await;

    let doctype = r#"<?xml version="1.0"?><!DOCTYPE x [<!ENTITY a "aaaa">]><scan:ScanSettings xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03"/>"#;
    let response = harness
        .send(post_job(&root(), Some(&secret), doctype.to_owned()))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let huge = settings("Platen", "image/png").replace(
        "<pwg:Version>",
        &format!("<!--{}--><pwg:Version>", "x".repeat(70 * 1024)),
    );
    let response = harness.send(post_job(&root(), Some(&secret), huge)).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let tiff = settings("Platen", "image/tiff");
    let response = harness.send(post_job(&root(), Some(&secret), tiff)).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    // Without credentials, the body is never read.
    let response = harness
        .send(post_job(&root(), None, settings("Platen", "image/png")))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(harness.scan.scan_requests().is_empty());
}

/// A body that sends `chunks`, then nothing, ever: a client that stalls.
struct Stalled {
    chunks: VecDeque<Bytes>,
}

impl futures_core::Stream for Stalled {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.chunks.pop_front() {
            Some(chunk) => Poll::Ready(Some(Ok(chunk))),
            None => Poll::Pending,
        }
    }
}

#[tokio::test]
async fn stalled_settings_time_out() {
    let harness = Harness::with(Setup {
        device_read_timeout: Duration::from_millis(200),
        ..Setup::default()
    })
    .await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let secret = device_secret(&harness, alice).await;
    let settings = settings("Platen", "image/png");
    for chunks in [
        vec![],
        vec![Bytes::copy_from_slice(&settings.as_bytes()[..100])],
    ] {
        let body = Body::from_stream(Stalled {
            chunks: chunks.into(),
        });
        let request = from_address(
            escl_request(
                Method::POST,
                &format!("{}/ScanJobs", root()),
                Some(&secret),
                body,
            ),
            LOOPBACK,
        );
        let response = tokio::time::timeout(Duration::from_secs(10), harness.send(request))
            .await
            .expect("recto stops waiting");
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    }
    assert!(harness.scan.scan_requests().is_empty());
}

#[tokio::test]
async fn escl_is_off_without_os_integration() {
    let harness = Harness::with(Setup {
        os_integration: false,
        ..Setup::default()
    })
    .await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let secret = device_secret(&harness, alice).await;
    for path in [
        format!("{}/ScannerCapabilities", root()),
        format!("{}/ScannerCapabilities", personal_root(&secret)),
    ] {
        let response = harness.send(get_escl(&path, Some(&secret))).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn unknown_escl_addresses_are_bare_404s() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let secret = device_secret(&harness, alice).await;
    for path in [
        format!("{}/Nothing", root()),
        format!("{}/ScanJobs/not-a-uuid/NextDocument", root()),
        format!(
            "{}/ScanJobs/6f1c9a52-4e0b-4c55-9a3e-2d1f0b7c8e91/NextDocument",
            root()
        ),
        format!("{}/Nothing", personal_root(&secret)),
    ] {
        let response = harness.send(get_escl(&path, Some(&secret))).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(response.headers().get(header::LOCATION).is_none(), "{path}");
    }
    // A browser with a session but no device password is refused, never sent to sign in: from
    // the network over plain HTTP, before credentials are even looked for.
    let cookie = harness.sign_in_with_password("alice", ALICE_PASSWORD).await;
    let response = harness
        .send(page("/eSCL/x/ScannerCapabilities", Some(&cookie)))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(response.headers().get(header::LOCATION).is_none());
}
