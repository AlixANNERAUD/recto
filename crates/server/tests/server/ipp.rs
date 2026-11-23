//! recto as an IPP printer: documents from computers and phones are held, never printed, until
//! their owner releases them; devices authenticate before their documents are read, see only
//! the jobs they sent, and keep their personal address in every URI they are given.

use std::{
    collections::VecDeque,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};

use axum::body::Bytes;
use chrono::{TimeDelta, Utc};
use ipp::model::Operation as IppOperation;
use ipp::prelude::{DelimiterTag, IppAttribute, IppRequestResponse, IppValue, IppVersion, Uri};
use ipp::{parser::IppParser, reader::IppReader};
use recto_domain::{
    device_passwords,
    held::{self, HeldState},
};
use recto_printing::testing::sample_pdf;

use super::*;

/// Device requests come from the server itself: the test server is plain HTTP, where only
/// loopback clients may send device passwords.
const LOOPBACK: [u8; 4] = [127, 0, 0, 1];
const PRINTER_URI: &str = "ipp://recto.test:80/ipp/print";

struct Device {
    id: i64,
    secret: String,
}

async fn device(harness: &Harness, user: i64, name: &str) -> Device {
    let created = device_passwords::create(&harness.database, user, name, Utc::now())
        .await
        .unwrap();
    Device {
        id: created.id,
        secret: created.secret,
    }
}

fn request(operation: IppOperation) -> IppRequestResponse {
    let uri: Uri = "ipp://recto.test/ipp/print".parse().unwrap();
    IppRequestResponse::new(IppVersion::v2_0(), operation, Some(uri)).unwrap()
}

fn operation_attribute(request: &mut IppRequestResponse, name: &str, value: IppValue) {
    request.attributes_mut().add(
        DelimiterTag::OperationAttributes,
        IppAttribute::with_name(name, value).unwrap(),
    );
}

fn job_request(operation: IppOperation, job: i32) -> IppRequestResponse {
    let mut request = request(operation);
    operation_attribute(&mut request, "job-id", IppValue::Integer(job));
    request
}

fn body(request: &IppRequestResponse, document: &[u8]) -> Vec<u8> {
    let mut body = request.to_bytes().to_vec();
    body.extend_from_slice(document);
    body
}

fn print_job(name: &str, document: &[u8]) -> Vec<u8> {
    let mut request = request(IppOperation::PrintJob);
    operation_attribute(
        &mut request,
        "job-name",
        IppValue::new_name_without_language(name).unwrap(),
    );
    body(&request, document)
}

/// An IPP post to `path` from the server itself, with Basic credentials holding `secret`.
fn post_ipp(path: &str, secret: Option<&str>, body: Body) -> Request<Body> {
    let mut request = Request::post(path).header(header::CONTENT_TYPE, "application/ipp");
    if let Some(secret) = secret {
        let credentials = data_encoding::BASE64.encode(format!("macbook:{secret}").as_bytes());
        request = request.header(header::AUTHORIZATION, format!("Basic {credentials}"));
    }
    from_address(request.body(body).unwrap(), LOOPBACK)
}

/// The path of a device's personal address.
fn personal(device: &Device) -> String {
    format!("/device/{}/ipp/print", device.secret)
}

/// Sends an IPP request and parses the answer, which must be a 200 carrying IPP.
async fn ipp(harness: &Harness, request: Request<Body>) -> IppRequestResponse {
    let response = harness.send(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/ipp");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    IppParser::new(IppReader::new(std::io::Cursor::new(bytes.to_vec())))
        .parse()
        .unwrap()
}

fn status(answer: &IppRequestResponse) -> u16 {
    answer.header().operation_or_status as u16
}

/// The attribute `name` of the first group tagged `tag`.
fn attribute(answer: &IppRequestResponse, tag: DelimiterTag, name: &str) -> Option<IppValue> {
    answer
        .attributes()
        .groups_of(tag)
        .next()?
        .get(name)
        .map(|attribute| attribute.value().clone())
}

fn job_attribute(answer: &IppRequestResponse, name: &str) -> IppValue {
    attribute(answer, DelimiterTag::JobAttributes, name)
        .unwrap_or_else(|| panic!("{name} is missing"))
}

fn printer_attribute(answer: &IppRequestResponse, name: &str) -> IppValue {
    attribute(answer, DelimiterTag::PrinterAttributes, name)
        .unwrap_or_else(|| panic!("{name} is missing"))
}

fn text_of(value: &IppValue) -> String {
    match value {
        IppValue::Uri(uri) => uri.as_str().to_owned(),
        IppValue::TextWithoutLanguage(text) => text.as_ref().to_owned(),
        IppValue::Keyword(keyword) => keyword.as_str().to_owned(),
        other => panic!("not a string: {other:?}"),
    }
}

fn job_id(answer: &IppRequestResponse) -> i32 {
    *job_attribute(answer, "job-id").as_integer().unwrap()
}

/// The ids of every job group of a Get-Jobs answer.
fn job_ids(answer: &IppRequestResponse) -> Vec<i32> {
    answer
        .attributes()
        .groups_of(DelimiterTag::JobAttributes)
        .filter_map(|group| group.get("job-id")?.value().as_integer().copied())
        .collect()
}

/// The documents held on disk.
fn held_files(harness: &Harness) -> usize {
    std::fs::read_dir(harness.held.dir()).unwrap().count()
}

/// A body sent in the chunks given, counting how many recto pulled.
struct Chunks {
    chunks: VecDeque<Bytes>,
    pulled: Arc<AtomicUsize>,
}

impl futures_core::Stream for Chunks {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let chunk = self.chunks.pop_front();
        if chunk.is_some() {
            self.pulled.fetch_add(1, Ordering::SeqCst);
        }
        Poll::Ready(chunk.map(Ok))
    }
}

/// `bytes` as a body of an 8-byte chunk then chunks of 1 KiB, and the count of chunks pulled.
fn chunked(bytes: &[u8]) -> (Body, Arc<AtomicUsize>) {
    let pulled = Arc::new(AtomicUsize::new(0));
    let mut chunks = VecDeque::from([Bytes::copy_from_slice(&bytes[..8])]);
    chunks.extend(bytes[8..].chunks(1024).map(Bytes::copy_from_slice));
    let body = Body::from_stream(Chunks {
        chunks,
        pulled: Arc::clone(&pulled),
    });
    (body, pulled)
}

#[tokio::test]
async fn a_pdf_from_a_device_is_held_not_printed() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let laptop = device(&harness, alice, "Laptop").await;

    let document = sample_pdf(2);
    let answer = ipp(
        &harness,
        post_ipp(
            "/ipp/print",
            Some(&laptop.secret),
            Body::from(print_job("minutes.pdf", &document)),
        ),
    )
    .await;

    assert_eq!(status(&answer), 0x0000);
    // Completed for the device: its part is done.
    assert_eq!(job_attribute(&answer, "job-state"), IppValue::Enum(9));
    let message = text_of(&job_attribute(&answer, "job-state-message"));
    assert!(message.contains("recto"), "{message}");
    let id = job_id(&answer);
    assert_eq!(
        text_of(&job_attribute(&answer, "job-uri")),
        format!("{PRINTER_URI}/{id}")
    );
    assert!(harness.print.submitted().is_empty());
    let rows = print_jobs::Entity::find()
        .all(&harness.database)
        .await
        .unwrap();
    assert!(rows.is_empty());

    let held = held::list(&harness.database, alice).await.unwrap();
    assert_eq!(held.len(), 1);
    let job = &held[0];
    assert_eq!(job.id, i64::from(id));
    assert_eq!(job.state, HeldState::Held);
    assert_eq!(job.name, "minutes.pdf");
    assert_eq!(job.document_pages, Some(2));
    assert_eq!(job.device_password_id, Some(laptop.id));
    assert_eq!(job.stored_format.as_deref(), Some("application/pdf"));
    assert_eq!(
        harness.held.read(job.file.as_ref().unwrap()).unwrap(),
        document
    );
}

#[tokio::test]
async fn create_job_then_send_document_holds_the_job() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let laptop = device(&harness, alice, "Laptop").await;
    let send = |body: Vec<u8>| post_ipp("/ipp/print", Some(&laptop.secret), Body::from(body));

    let mut create = request(IppOperation::CreateJob);
    operation_attribute(
        &mut create,
        "job-name",
        IppValue::new_name_without_language("notes.pdf").unwrap(),
    );
    create.attributes_mut().add(
        DelimiterTag::JobAttributes,
        IppAttribute::with_name("copies", IppValue::Integer(3)).unwrap(),
    );
    let answer = ipp(&harness, send(body(&create, b""))).await;
    assert_eq!(status(&answer), 0x0000);
    assert_eq!(job_attribute(&answer, "job-state"), IppValue::Enum(3));
    let id = job_id(&answer);

    // Waiting for its document, it is the one job not completed.
    let answer = ipp(&harness, send(body(&request(IppOperation::GetJobs), b""))).await;
    assert_eq!(job_ids(&answer), vec![id]);

    let mut document = job_request(IppOperation::SendDocument, id);
    operation_attribute(&mut document, "last-document", IppValue::Boolean(true));
    let answer = ipp(&harness, send(body(&document, &sample_pdf(2)))).await;
    assert_eq!(status(&answer), 0x0000);
    assert_eq!(job_attribute(&answer, "job-state"), IppValue::Enum(9));
    assert_eq!(job_id(&answer), id);

    let job = held::get(&harness.database, alice, i64::from(id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.state, HeldState::Held);
    assert_eq!(job.name, "notes.pdf");
    assert_eq!(job.document_pages, Some(2));
    assert!(job.requested.contains("\"copies\":3"), "{}", job.requested);
    // A second document is one too many.
    let answer = ipp(&harness, send(body(&document, &sample_pdf(1)))).await;
    assert_eq!(status(&answer), 0x0509);
    assert!(harness.print.submitted().is_empty());
}

#[tokio::test]
async fn attributes_are_open_and_jobs_need_credentials() {
    let harness = Harness::new().await;
    // From the network, over plain HTTP, without credentials: what the printer is.
    let response = harness
        .send(
            Request::post("/ipp/print")
                .header(header::CONTENT_TYPE, "application/ipp")
                .body(Body::from(body(
                    &request(IppOperation::GetPrinterAttributes),
                    b"",
                )))
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let answer = IppParser::new(IppReader::new(std::io::Cursor::new(bytes.to_vec())))
        .parse()
        .unwrap();
    assert_eq!(status(&answer), 0x0000);
    assert_eq!(
        text_of(&printer_attribute(&answer, "printer-uri-supported")),
        PRINTER_URI
    );
    assert_eq!(
        text_of(&printer_attribute(&answer, "uri-authentication-supported")),
        "basic"
    );
    assert_eq!(
        text_of(&printer_attribute(&answer, "printer-more-info")),
        "http://recto.test/jobs"
    );
    // PDF, and the rasters recto converts to it.
    let formats = printer_attribute(&answer, "document-format-supported");
    let formats = format!("{formats:?}");
    for format in ["application/pdf", "image/pwg-raster", "image/urf"] {
        assert!(formats.contains(format), "{formats}");
    }
    assert!(format!("{:?}", printer_attribute(&answer, "urf-supported")).contains("RS300"));

    // A job without credentials is refused once its header is read, before its document.
    let (sent, pulled) = chunked(&print_job("secret.pdf", &sample_pdf(30)));
    let response = harness.send(post_ipp("/ipp/print", None, sent)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().contains_key(header::WWW_AUTHENTICATE));
    assert_eq!(pulled.load(Ordering::SeqCst), 1);
    // Wrong credentials alike.
    let (sent, pulled) = chunked(&print_job("secret.pdf", &sample_pdf(30)));
    let response = harness
        .send(post_ipp("/ipp/print", Some(&"a".repeat(32)), sent))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(pulled.load(Ordering::SeqCst), 1);
    assert_eq!(held_files(&harness), 0);
}

#[tokio::test]
async fn a_personal_address_is_checked_before_the_body() {
    let harness = Harness::new().await;
    let (sent, pulled) = chunked(&print_job("secret.pdf", &sample_pdf(30)));
    let path = format!("/device/{}/ipp/print", "a".repeat(32));
    let response = harness.send(post_ipp(&path, None, sent)).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(!response.headers().contains_key(header::LOCATION));
    assert_eq!(pulled.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn answers_on_a_personal_address_keep_it() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let phone = device(&harness, alice, "Phone").await;
    let address = format!("ipp://recto.test:80{}", personal(&phone));

    let answer = ipp(
        &harness,
        post_ipp(
            &personal(&phone),
            None,
            Body::from(body(&request(IppOperation::GetPrinterAttributes), b"")),
        ),
    )
    .await;
    assert_eq!(
        text_of(&printer_attribute(&answer, "printer-uri-supported")),
        address
    );
    // Android refuses an address that asks for Basic.
    assert_eq!(
        text_of(&printer_attribute(&answer, "uri-authentication-supported")),
        "none"
    );

    let answer = ipp(
        &harness,
        post_ipp(
            &personal(&phone),
            None,
            Body::from(print_job("photo.pdf", &sample_pdf(1))),
        ),
    )
    .await;
    let id = job_id(&answer);
    let job_uri = text_of(&job_attribute(&answer, "job-uri"));
    assert_eq!(job_uri, format!("{address}/{id}"));

    // The device follows the job URI it was given, naming the job by it.
    let mut attributes =
        IppRequestResponse::new(IppVersion::v2_0(), IppOperation::GetJobAttributes, None).unwrap();
    operation_attribute(
        &mut attributes,
        "job-uri",
        IppValue::new_uri(&job_uri).unwrap(),
    );
    let path = format!("{}/{id}", personal(&phone));
    let answer = ipp(
        &harness,
        post_ipp(&path, None, Body::from(body(&attributes, b""))),
    )
    .await;
    assert_eq!(status(&answer), 0x0000);
    assert_eq!(job_id(&answer), id);
    assert_eq!(text_of(&job_attribute(&answer, "job-uri")), job_uri);
    assert_eq!(text_of(&job_attribute(&answer, "job-printer-uri")), address);
}

#[tokio::test]
async fn a_device_sees_only_its_own_jobs() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let laptop = device(&harness, alice, "Laptop").await;
    let phone = device(&harness, alice, "Phone").await;
    let send = |device: &Device, body: Vec<u8>| {
        post_ipp("/ipp/print", Some(&device.secret), Body::from(body))
    };
    let from_laptop =
        job_id(&ipp(&harness, send(&laptop, print_job("a.pdf", &sample_pdf(1)))).await);
    let from_phone = job_id(&ipp(&harness, send(&phone, print_job("b.pdf", &sample_pdf(1)))).await);

    let mut all = request(IppOperation::GetJobs);
    operation_attribute(
        &mut all,
        "which-jobs",
        IppValue::new_keyword("all").unwrap(),
    );
    let answer = ipp(&harness, send(&laptop, body(&all, b""))).await;
    assert_eq!(job_ids(&answer), vec![from_laptop]);
    let answer = ipp(&harness, send(&phone, body(&all, b""))).await;
    assert_eq!(job_ids(&answer), vec![from_phone]);

    for operation in [IppOperation::GetJobAttributes, IppOperation::CancelJob] {
        let answer = ipp(
            &harness,
            send(&laptop, body(&job_request(operation, from_phone), b"")),
        )
        .await;
        assert_eq!(status(&answer), 0x0406, "{operation:?}");
    }
    // The Jobs page shows both.
    assert_eq!(held::list(&harness.database, alice).await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_revoked_password_stops_the_next_request() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let laptop = device(&harness, alice, "Laptop").await;
    let answer = ipp(
        &harness,
        post_ipp(
            "/ipp/print",
            Some(&laptop.secret),
            Body::from(print_job("a.pdf", &sample_pdf(1))),
        ),
    )
    .await;
    assert_eq!(status(&answer), 0x0000);

    device_passwords::revoke(&harness.database, alice, laptop.id)
        .await
        .unwrap();
    let response = harness
        .send(post_ipp(
            "/ipp/print",
            Some(&laptop.secret),
            Body::from(print_job("b.pdf", &sample_pdf(1))),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = harness
        .send(post_ipp(
            &personal(&laptop),
            None,
            Body::from(print_job("b.pdf", &sample_pdf(1))),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    // The job it sent before stays on the Jobs page.
    assert_eq!(held::list(&harness.database, alice).await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_oversized_body_is_refused() {
    let harness = Harness::with(Setup {
        max_upload_size: 1024,
        ..Setup::default()
    })
    .await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let laptop = device(&harness, alice, "Laptop").await;
    let huge = vec![b'%'; 200 * 1024];
    // With its length announced, and sent in chunks without one.
    let response = harness
        .send(post_ipp(
            "/ipp/print",
            Some(&laptop.secret),
            Body::from(print_job("huge.pdf", &huge)),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let (sent, _) = chunked(&print_job("huge.pdf", &huge));
    let response = harness
        .send(post_ipp("/ipp/print", Some(&laptop.secret), sent))
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    // A document over the limit within the envelope's slack is refused by IPP.
    let answer = ipp(
        &harness,
        post_ipp(
            "/ipp/print",
            Some(&laptop.secret),
            Body::from(print_job("big.pdf", &sample_pdf(30))),
        ),
    )
    .await;
    // client-error-request-entity-too-large.
    assert_eq!(status(&answer), 0x0408);
    assert!(
        held::list(&harness.database, alice)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(held_files(&harness), 0);
}

#[tokio::test]
async fn the_cap_refuses_before_writing() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let laptop = device(&harness, alice, "Laptop").await;
    for number in 0..held::MAX_JOBS_PER_USER {
        held::create_incoming(
            &harness.database,
            alice,
            laptop.id,
            &format!("job {number}"),
            "{}",
            Utc::now(),
            TimeDelta::hours(24),
            i64::MAX,
        )
        .await
        .unwrap();
    }
    // A write would fail, and answer server-error-internal-error: the cap must refuse first.
    use std::os::unix::fs::PermissionsExt;
    let dir = harness.held.dir().to_owned();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let answer = ipp(
        &harness,
        post_ipp(
            "/ipp/print",
            Some(&laptop.secret),
            Body::from(print_job("one more.pdf", &sample_pdf(1))),
        ),
    )
    .await;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(status(&answer), 0x050B);
    assert_eq!(held_files(&harness), 0);
}

#[tokio::test]
async fn unknown_device_paths_never_redirect() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let laptop = device(&harness, alice, "Laptop").await;
    let cookie = harness.sign_in_with_password("alice", ALICE_PASSWORD).await;
    for uri in [
        format!("/device/{}/anything", laptop.secret),
        personal(&laptop),
        format!("{}/7", personal(&laptop)),
        "/ipp/print".to_owned(),
        "/ipp/other".to_owned(),
        "/eSCL/x".to_owned(),
    ] {
        for cookie in [None, Some(cookie.as_str())] {
            let response = harness.send(page(&uri, cookie)).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
            assert!(!response.headers().contains_key(header::LOCATION), "{uri}");
        }
    }
    for uri in [
        format!("/device/{}/ipp/other", laptop.secret),
        "/ipp/other".to_owned(),
        "/ipp/print/x/y".to_owned(),
    ] {
        let response = harness
            .send(post_ipp(&uri, Some(&laptop.secret), Body::empty()))
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        assert!(!response.headers().contains_key(header::LOCATION), "{uri}");
    }
}

#[tokio::test]
async fn the_os_cannot_cancel_a_held_job() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let laptop = device(&harness, alice, "Laptop").await;
    let send = |body: Vec<u8>| post_ipp("/ipp/print", Some(&laptop.secret), Body::from(body));
    let held_id = job_id(&ipp(&harness, send(print_job("a.pdf", &sample_pdf(1)))).await);

    // Reported completed, a held job is past cancelling (RFC 8011): the person deletes it on
    // the Jobs page.
    let answer = ipp(
        &harness,
        send(body(&job_request(IppOperation::CancelJob, held_id), b"")),
    )
    .await;
    assert_eq!(status(&answer), 0x0404);
    let job = held::get(&harness.database, alice, i64::from(held_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.state, HeldState::Held);
    assert_eq!(held_files(&harness), 1);

    // A job still waiting for its document goes.
    let created = job_id(&ipp(&harness, send(body(&request(IppOperation::CreateJob), b""))).await);
    let answer = ipp(
        &harness,
        send(body(&job_request(IppOperation::CancelJob, created), b"")),
    )
    .await;
    assert_eq!(status(&answer), 0x0000);
    assert!(
        held::get(&harness.database, alice, i64::from(created))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn os_integration_off_removes_the_printer() {
    let harness = Harness::with(Setup {
        os_integration: false,
        ..Setup::default()
    })
    .await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let laptop = device(&harness, alice, "Laptop").await;
    for path in ["/ipp/print".to_owned(), personal(&laptop)] {
        let response = harness
            .send(post_ipp(
                &path,
                Some(&laptop.secret),
                Body::from(print_job("a.pdf", &sample_pdf(1))),
            ))
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    assert!(
        held::list(&harness.database, alice)
            .await
            .unwrap()
            .is_empty()
    );
}

/// Two pages of Apple raster and of PWG raster, from Ghostscript through CUPS's writer.
const URF: &[u8] =
    include_bytes!("../../../printing/tests/fixtures/raster/two-pages-srgb24-75dpi.urf");
const PWG: &[u8] =
    include_bytes!("../../../printing/tests/fixtures/raster/two-pages-sgray8-75dpi.pwg");

/// A Print-Job declaring `format`, asking for `copies`.
fn print_job_as(name: &str, format: &str, copies: Option<i32>, document: &[u8]) -> Vec<u8> {
    let mut request = request(IppOperation::PrintJob);
    operation_attribute(
        &mut request,
        "job-name",
        IppValue::new_name_without_language(name).unwrap(),
    );
    operation_attribute(
        &mut request,
        "document-format",
        IppValue::new_mime_media_type(format).unwrap(),
    );
    if let Some(copies) = copies {
        request.attributes_mut().add(
            DelimiterTag::JobAttributes,
            IppAttribute::with_name("copies", IppValue::Integer(copies)).unwrap(),
        );
    }
    body(&request, document)
}

async fn send_job(harness: &Harness, device: &Device, job: Vec<u8>) -> IppRequestResponse {
    ipp(
        harness,
        post_ipp("/ipp/print", Some(&device.secret), Body::from(job)),
    )
    .await
}

#[tokio::test]
async fn a_raster_job_is_held_as_a_pdf() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let phone = device(&harness, alice, "Phone").await;

    let answer = send_job(
        &harness,
        &phone,
        print_job_as("photo", "image/urf", None, URF),
    )
    .await;
    assert_eq!(status(&answer), 0x0000);
    assert_eq!(job_attribute(&answer, "job-state"), IppValue::Enum(9));
    // Asked for detection: the received format is what the content is.
    let answer = send_job(
        &harness,
        &phone,
        print_job_as("scan", "application/octet-stream", None, PWG),
    )
    .await;
    assert_eq!(status(&answer), 0x0000);

    let held = held::list(&harness.database, alice).await.unwrap();
    assert_eq!(held.len(), 2);
    for (job, received) in held.iter().zip(["image/pwg-raster", "image/urf"]) {
        assert_eq!(job.state, HeldState::Held);
        assert_eq!(job.stored_format.as_deref(), Some("application/pdf"));
        assert_eq!(job.received_format.as_deref(), Some(received));
        assert_eq!(job.document_pages, Some(2));
        // One format downstream: the file is the PDF, and its own count agrees.
        let stored = harness.held.read(job.file.as_ref().unwrap()).unwrap();
        assert!(stored.starts_with(b"%PDF-"));
        assert_eq!(recto_printing::count::pdf_pages(&stored), Some(2));
        assert_eq!(job.size, stored.len() as i64);
    }
    assert!(harness.print.submitted().is_empty());
}

#[tokio::test]
async fn a_bad_raster_is_refused() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let phone = device(&harness, alice, "Phone").await;

    // Cut inside a line: refused, never padded.
    let cut = &URF[..URF.len() - 10];
    let answer = send_job(
        &harness,
        &phone,
        print_job_as("cut", "image/urf", None, cut),
    )
    .await;
    assert_eq!(
        status(&answer),
        0x0411,
        "client-error-document-format-error"
    );

    // A page header claiming 100000 x 100000 pixels: refused from the header, before decoding.
    let mut huge = b"UNIRAST\0\0\0\0\x01".to_vec();
    let mut header = [0u8; 32];
    header[0] = 24;
    header[1] = 1;
    header[12..16].copy_from_slice(&100_000u32.to_be_bytes());
    header[16..20].copy_from_slice(&100_000u32.to_be_bytes());
    header[20..24].copy_from_slice(&300u32.to_be_bytes());
    huge.extend_from_slice(&header);
    huge.extend_from_slice(&[0xFF, 0x80]);
    let answer = send_job(
        &harness,
        &phone,
        print_job_as("huge", "image/urf", None, &huge),
    )
    .await;
    assert_eq!(
        status(&answer),
        0x0408,
        "client-error-request-entity-too-large"
    );

    assert!(
        held::list(&harness.database, alice)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(held_files(&harness), 0);
}

#[tokio::test]
async fn direct_printing_releases_at_once() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    users::set_direct_printer(&harness.database, alice, Some("office"))
        .await
        .unwrap();
    let laptop = device(&harness, alice, "Laptop").await;

    let answer = send_job(
        &harness,
        &laptop,
        print_job_as("photo", "image/urf", Some(2), URF),
    )
    .await;
    assert_eq!(status(&answer), 0x0000);
    assert_eq!(job_attribute(&answer, "job-state"), IppValue::Enum(9));
    assert_eq!(
        text_of(&job_attribute(&answer, "job-state-reasons")),
        "job-completed-successfully"
    );
    let id = job_id(&answer);

    // Through the print path: CUPS, the ledger, the held row released.
    let submitted = harness.print.submitted();
    assert_eq!(submitted.len(), 1);
    assert_eq!(submitted[0].printer, "office");
    assert_eq!(submitted[0].user, "alice");
    assert_eq!(submitted[0].options.copies, 2);
    assert!(submitted[0].document.starts_with(b"%PDF-"));
    let rows = print_jobs::Entity::find()
        .all(&harness.database)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pages, Some(4));
    let job = held::get(&harness.database, alice, i64::from(id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.state, HeldState::Released);
    assert_eq!(job.ledger_entry_id, Some(rows[0].id));
    assert_eq!(held_files(&harness), 0);

    // The device asking again sees it done.
    let ask = job_request(IppOperation::GetJobAttributes, id);
    let answer = ipp(
        &harness,
        post_ipp(
            "/ipp/print",
            Some(&laptop.secret),
            Body::from(ask.to_bytes().to_vec()),
        ),
    )
    .await;
    assert_eq!(
        text_of(&job_attribute(&answer, "job-state-reasons")),
        "job-completed-successfully"
    );
}

#[tokio::test]
async fn a_refused_direct_print_stays_held() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    users::set_direct_printer(&harness.database, alice, Some("office"))
        .await
        .unwrap();
    let laptop = device(&harness, alice, "Laptop").await;
    recto_domain::quotas::create(
        &harness.database,
        recto_domain::quotas::NewRule {
            name: "Monthly pages".into(),
            enabled: true,
            subject: recto_domain::quotas::Subject::Everyone,
            printers: vec![],
            metric: recto_domain::quotas::Metric::Pages,
            period: recto_domain::quotas::Period::Month,
            limit: 1,
            enforcement: recto_domain::quotas::Enforcement::Hard,
        },
    )
    .await
    .unwrap();

    let answer = send_job(
        &harness,
        &laptop,
        print_job_as("minutes.pdf", "application/pdf", None, &sample_pdf(2)),
    )
    .await;
    // The device's part is still done, so it does not retry or stop its queue; but it is told.
    assert_eq!(status(&answer), 0x0000);
    assert_eq!(job_attribute(&answer, "job-state"), IppValue::Enum(9));
    assert_eq!(
        text_of(&job_attribute(&answer, "job-state-reasons")),
        "job-completed-with-errors"
    );
    let message = text_of(&job_attribute(&answer, "job-state-message"));
    assert!(message.contains("quota"), "{message}");

    assert!(harness.print.submitted().is_empty());
    assert_eq!(
        print_jobs::Entity::find()
            .count(&harness.database)
            .await
            .unwrap(),
        0
    );
    let held = held::list(&harness.database, alice).await.unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].state, HeldState::Held);
    assert_eq!(held[0].refusal.as_deref(), Some("quota"));
    assert_eq!(held_files(&harness), 1);

    // Asked again, the device hears the same.
    let ask = job_request(IppOperation::GetJobAttributes, job_id(&answer));
    let again = ipp(
        &harness,
        post_ipp(
            "/ipp/print",
            Some(&laptop.secret),
            Body::from(ask.to_bytes().to_vec()),
        ),
    )
    .await;
    assert_eq!(
        text_of(&job_attribute(&again, "job-state-reasons")),
        "job-completed-with-errors"
    );

    // Its owner sees why on the Jobs page.
    let cookie = harness.sign_in_with_password("alice", ALICE_PASSWORD).await;
    let jobs = text(harness.send(page("/jobs", Some(&cookie))).await).await;
    assert!(jobs.contains("minutes.pdf"), "{jobs}");
    assert!(
        jobs.contains("Last attempt: This job goes over a print quota."),
        "{jobs}"
    );
}

#[tokio::test]
async fn only_usable_printers_are_direct() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let cookie = harness.sign_in_with_password("alice", ALICE_PASSWORD).await;
    let direct = |printer: &str| {
        form(
            "/account/direct-printer",
            Some(&cookie),
            &[("printer", printer)],
        )
    };
    let stored = || async {
        users::find_by_id(&harness.database, alice)
            .await
            .unwrap()
            .unwrap()
            .direct_printer
    };

    // The Account page offers holding and every printer alice may use.
    let account = text(harness.send(page("/account", Some(&cookie))).await).await;
    assert!(
        account.contains(r#"action="/account/direct-printer""#),
        "{account}"
    );
    assert!(account.contains(r#"<option value="office""#), "{account}");

    let response = harness.send(direct("office")).await;
    assert_redirect(&response, "/account?done=direct");
    assert_eq!(stored().await.as_deref(), Some("office"));
    let account = text(harness.send(page("/account", Some(&cookie))).await).await;
    assert!(
        account.contains(r#"<option value="office" selected"#),
        "{account}"
    );

    // Empty holds again.
    let response = harness.send(direct("")).await;
    assert_redirect(&response, "/account?done=direct");
    assert_eq!(stored().await, None);

    // A printer CUPS does not have, or one only administrators may use, is refused.
    let response = harness.send(direct("nowhere")).await;
    assert_redirect(&response, "/account?error=direct_printer");
    access::set(
        &harness.database,
        DeviceKind::Printer,
        "office",
        Access::Groups(vec![]),
    )
    .await
    .unwrap();
    let response = harness.send(direct("office")).await;
    assert_redirect(&response, "/account?error=direct_printer");
    assert_eq!(stored().await, None);

    // A printer that stops being usable after it was chosen leaves the next job held.
    users::set_direct_printer(&harness.database, alice, Some("office"))
        .await
        .unwrap();
    let laptop = device(&harness, alice, "Laptop").await;
    let answer = send_job(
        &harness,
        &laptop,
        print_job_as("minutes.pdf", "application/pdf", None, &sample_pdf(1)),
    )
    .await;
    assert_eq!(
        text_of(&job_attribute(&answer, "job-state-reasons")),
        "job-completed-with-errors"
    );
    assert!(harness.print.submitted().is_empty());
    let held = held::list(&harness.database, alice).await.unwrap();
    assert_eq!(held[0].refusal.as_deref(), Some("printer_not_found"));
}

#[tokio::test]
async fn direct_printing_needs_os_integration() {
    let harness = Harness::with(Setup {
        os_integration: false,
        ..Setup::default()
    })
    .await;
    harness.local_user("alice", ALICE_PASSWORD).await;
    let cookie = harness.sign_in_with_password("alice", ALICE_PASSWORD).await;
    let response = harness
        .send(form(
            "/account/direct-printer",
            Some(&cookie),
            &[("printer", "office")],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// A body that sends `chunks`, then nothing, ever: a device that stalls.
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

fn stalled(chunks: &[&[u8]]) -> Body {
    Body::from_stream(Stalled {
        chunks: chunks.iter().map(|c| Bytes::copy_from_slice(c)).collect(),
    })
}

#[tokio::test]
async fn an_unauthenticated_body_stays_small() {
    let harness = Harness::new().await;
    // Get-Printer-Attributes needs no credentials and carries no document: past 64 KiB, the
    // body is refused rather than kept in memory.
    let attributes = request(IppOperation::GetPrinterAttributes).to_bytes();
    let response = harness
        .send(post_ipp(
            "/ipp/print",
            None,
            Body::from(body(
                &request(IppOperation::GetPrinterAttributes),
                &vec![0; 100 * 1024],
            )),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let answer = ipp(
        &harness,
        post_ipp("/ipp/print", None, Body::from(attributes.to_vec())),
    )
    .await;
    assert_eq!(status(&answer), 0x0000);
}

#[tokio::test]
async fn only_documents_make_a_body_large() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let laptop = device(&harness, alice, "Laptop").await;
    // With credentials too, an operation without a document is read up to 64 KiB only: many of
    // them at once would otherwise each keep RECTO_MAX_UPLOAD_SIZE in memory.
    for operation in [
        IppOperation::GetJobs,
        IppOperation::ValidateJob,
        IppOperation::CreateJob,
    ] {
        let padded = body(&request(operation), &vec![0; 100 * 1024]);
        let response = harness
            .send(post_ipp(
                "/ipp/print",
                Some(&laptop.secret),
                Body::from(padded.clone()),
            ))
            .await;
        assert_eq!(
            response.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "{operation:?}"
        );
        let (sent, _) = chunked(&padded);
        let response = harness.send(post_ipp(&personal(&laptop), None, sent)).await;
        assert_eq!(
            response.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "{operation:?}"
        );
    }
    // A document past 64 KiB is read.
    let large = sample_pdf(1000);
    assert!(large.len() > 64 * 1024);
    let answer = ipp(
        &harness,
        post_ipp(
            "/ipp/print",
            Some(&laptop.secret),
            Body::from(print_job("large.pdf", &large)),
        ),
    )
    .await;
    assert_eq!(status(&answer), 0x0000);
}

#[tokio::test]
async fn deeply_nested_collections_are_refused_without_credentials() {
    let harness = Harness::new().await;
    // About 4,000 collections, each the only member of the one around it, within the 64 KiB
    // read without credentials. Parsed as is, they would stall the worker or overflow its stack.
    fn entry(tag: u8, name: &[u8]) -> Vec<u8> {
        let mut bytes = vec![tag];
        bytes.extend_from_slice(&u16::try_from(name.len()).unwrap().to_be_bytes());
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(&[0, 0]);
        bytes
    }
    let get = request(IppOperation::GetPrinterAttributes).to_bytes();
    let mut nested = get[..get.len() - 1].to_vec();
    nested.extend(entry(0x34, b"media-col"));
    for _ in 1..4000 {
        nested.extend([0x4A, 0, 0, 0, 1, b'm']);
        nested.extend(entry(0x34, b""));
    }
    for _ in 0..4000 {
        nested.extend(entry(0x37, b""));
    }
    nested.push(0x03);
    assert!(nested.len() <= 64 * 1024);

    let answer = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        ipp(&harness, post_ipp("/ipp/print", None, Body::from(nested))),
    )
    .await
    .expect("refused at once");
    // client-error-bad-request.
    assert_eq!(status(&answer), 0x0400);
}

#[tokio::test]
async fn a_stalled_body_times_out() {
    let harness = Harness::with(Setup {
        device_read_timeout: std::time::Duration::from_millis(200),
        ..Setup::default()
    })
    .await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let laptop = device(&harness, alice, "Laptop").await;
    let job = print_job("minutes.pdf", &sample_pdf(2));

    // Nothing at all, and the header but no document, with or without credentials.
    for (secret, body) in [
        (None, stalled(&[])),
        (None, stalled(&[&job[..4]])),
        (Some(laptop.secret.as_str()), stalled(&[&job[..8]])),
        (
            Some(laptop.secret.as_str()),
            stalled(&[&job[..8], &job[8..200]]),
        ),
    ] {
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            harness.send(post_ipp("/ipp/print", secret, body)),
        )
        .await
        .expect("recto stops waiting");
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    }
    // A personal address alike.
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        harness.send(post_ipp(&personal(&laptop), None, stalled(&[&job[..8]]))),
    )
    .await
    .expect("recto stops waiting");
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    assert!(
        held::list(&harness.database, alice)
            .await
            .unwrap()
            .is_empty()
    );
}

/// A body whose 8-byte header comes at once, then the rest one byte at a time, each well
/// within the read timeout of the piece before.
struct Trickle {
    bytes: VecDeque<Bytes>,
    every: std::time::Duration,
    next: Pin<Box<tokio::time::Sleep>>,
}

impl futures_core::Stream for Trickle {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.next.as_mut().poll(context).is_pending() {
            return Poll::Pending;
        }
        let every = self.every;
        let deadline = tokio::time::Instant::now() + every;
        self.next.as_mut().reset(deadline);
        Poll::Ready(self.bytes.pop_front().map(Ok))
    }
}

fn trickle(bytes: &[u8], every: std::time::Duration) -> Body {
    let mut pieces = VecDeque::from([Bytes::copy_from_slice(&bytes[..8])]);
    pieces.extend(bytes[8..].iter().map(|&byte| Bytes::from(vec![byte])));
    Body::from_stream(Trickle {
        bytes: pieces,
        every,
        next: Box::pin(tokio::time::sleep(std::time::Duration::ZERO)),
    })
}

#[tokio::test]
async fn a_trickling_body_runs_out_of_time() {
    let harness = Harness::with(Setup {
        device_read_timeout: std::time::Duration::from_millis(300),
        ..Setup::default()
    })
    .await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let laptop = device(&harness, alice, "Laptop").await;
    // Each byte comes 100 ms after the last, so no single wait is too long: the whole request
    // must still arrive within the read timeout, here 300 ms instead of 30 seconds.
    for (secret, operation) in [
        (None, IppOperation::GetPrinterAttributes),
        (Some(laptop.secret.as_str()), IppOperation::GetJobs),
    ] {
        let sent = request(operation).to_bytes();
        assert!(sent.len() > 20);
        let started = std::time::Instant::now();
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            harness.send(post_ipp(
                "/ipp/print",
                secret,
                trickle(&sent, std::time::Duration::from_millis(100)),
            )),
        )
        .await
        .expect("recto stops waiting");
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }
}

#[tokio::test]
async fn one_user_sends_two_documents_at_a_time() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let bob = harness.local_user("bob", ALICE_PASSWORD).await;
    let laptop = device(&harness, alice, "Laptop").await;
    let phone = device(&harness, alice, "Phone").await;
    let bobs = device(&harness, bob, "Laptop").await;
    let job = print_job("minutes.pdf", &sample_pdf(2));

    // Two documents of alice's still arriving, from two devices.
    let mut sending = Vec::new();
    for device in [&laptop, &phone] {
        let request = post_ipp(
            "/ipp/print",
            Some(&device.secret),
            stalled(&[&job[..8], &job[8..100]]),
        );
        let router = harness.router.clone();
        sending.push(tokio::spawn(async move {
            router.oneshot(from_peer(request)).await.unwrap()
        }));
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // A third waits for neither: it is told to come back, before its document is read.
    let answer = ipp(
        &harness,
        post_ipp("/ipp/print", Some(&laptop.secret), Body::from(job.clone())),
    )
    .await;
    assert_eq!(status(&answer), 0x0507, "server-error-busy");
    // Other users are not held up, nor are alice's other operations.
    let answer = ipp(
        &harness,
        post_ipp("/ipp/print", Some(&bobs.secret), Body::from(job.clone())),
    )
    .await;
    assert_eq!(status(&answer), 0x0000);
    let jobs = request(IppOperation::GetJobs);
    let answer = ipp(
        &harness,
        post_ipp(
            "/ipp/print",
            Some(&laptop.secret),
            Body::from(jobs.to_bytes().to_vec()),
        ),
    )
    .await;
    assert_eq!(status(&answer), 0x0000);

    // Once one ends, alice sends again.
    sending.remove(0).abort();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let answer = ipp(
        &harness,
        post_ipp("/ipp/print", Some(&laptop.secret), Body::from(job)),
    )
    .await;
    assert_eq!(status(&answer), 0x0000);
}

#[tokio::test]
async fn an_unknown_which_jobs_is_named_in_the_refusal() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let laptop = device(&harness, alice, "Laptop").await;
    let mut jobs = request(IppOperation::GetJobs);
    operation_attribute(
        &mut jobs,
        "which-jobs",
        IppValue::new_keyword("fetchable").unwrap(),
    );
    let answer = ipp(
        &harness,
        post_ipp(
            "/ipp/print",
            Some(&laptop.secret),
            Body::from(jobs.to_bytes().to_vec()),
        ),
    )
    .await;
    assert_eq!(status(&answer), 0x040B);
    assert_eq!(
        attribute(&answer, DelimiterTag::UnsupportedAttributes, "which-jobs")
            .map(|value| text_of(&value)),
        Some("fetchable".to_owned())
    );
}

#[tokio::test]
async fn a_printer_that_cannot_keep_documents_refuses_jobs() {
    let harness = Harness::with(Setup {
        held_usable: false,
        ..Setup::default()
    })
    .await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let laptop = device(&harness, alice, "Laptop").await;
    let send = |body: Vec<u8>| post_ipp("/ipp/print", Some(&laptop.secret), Body::from(body));

    let answer = ipp(&harness, send(print_job("minutes.pdf", &sample_pdf(1)))).await;
    assert_eq!(status(&answer), 0x0506, "Print-Job");
    let mut create = request(IppOperation::CreateJob);
    operation_attribute(
        &mut create,
        "job-name",
        IppValue::new_name_without_language("minutes.pdf").unwrap(),
    );
    let answer = ipp(&harness, send(create.to_bytes().to_vec())).await;
    assert_eq!(status(&answer), 0x0506, "Create-Job");
    let validate = request(IppOperation::ValidateJob);
    let answer = ipp(&harness, send(validate.to_bytes().to_vec())).await;
    assert_eq!(status(&answer), 0x0506, "Validate-Job");

    // Devices are told before they send.
    let answer = ipp(
        &harness,
        post_ipp(
            "/ipp/print",
            None,
            Body::from(
                request(IppOperation::GetPrinterAttributes)
                    .to_bytes()
                    .to_vec(),
            ),
        ),
    )
    .await;
    assert_eq!(
        printer_attribute(&answer, "printer-is-accepting-jobs"),
        IppValue::Boolean(false)
    );
    assert!(
        held::list(&harness.database, alice)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn guessing_device_passwords_never_locks_out_sign_in() {
    let harness = Harness::new().await;
    harness.local_user("alice", ALICE_PASSWORD).await;
    let job = print_job("minutes.pdf", &sample_pdf(1));
    // Every guess a different secret, so none is refused from memory without being counted.
    let mut refused = None;
    for guess in 0..40 {
        let secret = format!("{guess:032}");
        let response = harness
            .send(post_ipp(
                "/ipp/print",
                Some(&secret),
                Body::from(job.clone()),
            ))
            .await;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            refused = Some(guess);
            break;
        }
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "guess {guess}");
    }
    assert_eq!(refused, Some(30), "the 31st guess is refused");

    // Password sign-in from the same address has its own limit, untouched.
    let response = harness
        .send(from_address(
            form(
                "/sign-in",
                None,
                &[("login", "alice"), ("password", ALICE_PASSWORD)],
            ),
            LOOPBACK,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(session_cookie(response.headers()).is_some());
}
