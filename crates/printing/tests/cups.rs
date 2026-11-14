//! The CUPS client against a fake CUPS: wiremock answers with real IPP bodies.

use std::io::{Cursor, Read};
use std::time::{Duration, Instant};

use bytes::Bytes;
use ipp::model::{DelimiterTag, Operation, StatusCode};
use ipp::parser::IppParser;
use ipp::reader::IppReader;
use ipp::request::IppRequestResponse;
use ipp::value::IppValue;
use recto_printing::testing::{attribute, ipp_response};
use recto_printing::{
    ColorMode, Cups, DocumentFormat, JobState, PageRanges, PrintError, PrintOptions, PrintRequest,
    PrintService, PrinterState, Sides,
};
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

fn parse(body: &[u8]) -> IppRequestResponse {
    IppParser::new(IppReader::new(Cursor::new(body.to_vec())))
        .parse()
        .expect("valid IPP body")
}

/// The first value of an operation or job attribute, as text.
fn attr(request: &IppRequestResponse, name: &str) -> Option<String> {
    request
        .attributes()
        .groups()
        .iter()
        .flat_map(|g| g.attributes())
        .find(|a| a.name().as_str() == name)
        .map(|a| a.value().to_string())
}

/// Matches the IPP operation id, and optionally one attribute value.
struct Ipp(Operation, Option<(&'static str, &'static str)>);

impl Match for Ipp {
    fn matches(&self, request: &Request) -> bool {
        let parsed = parse(&request.body);
        if parsed.header().operation_or_status != self.0 as i16 {
            return false;
        }
        match self.1 {
            Some((name, value)) => attr(&parsed, name).as_deref() == Some(value),
            None => true,
        }
    }
}

fn ipp(
    status: StatusCode,
    groups: Vec<(DelimiterTag, Vec<ipp::attribute::IppAttribute>)>,
) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "application/ipp")
        .set_body_bytes(ipp_response(status, groups))
}

fn keyword(value: &str) -> IppValue {
    IppValue::new_keyword(value).unwrap()
}

fn text(value: &str) -> IppValue {
    IppValue::new_text_without_language(value).unwrap()
}

fn name(value: &str) -> IppValue {
    IppValue::new_name_without_language(value).unwrap()
}

fn printer_group(printer: &str, state: i32) -> (DelimiterTag, Vec<ipp::attribute::IppAttribute>) {
    (
        DelimiterTag::PrinterAttributes,
        vec![
            attribute("printer-name", name(printer)),
            attribute("printer-info", text("Front office")),
            attribute("printer-location", text("Ground floor")),
            attribute("printer-make-and-model", text("Acme Laser 3000")),
            attribute("printer-state", IppValue::Enum(state)),
            attribute(
                "printer-state-reasons",
                IppValue::Array(vec![
                    keyword("toner-low-warning"),
                    keyword("media-empty-error"),
                ]),
            ),
            attribute("printer-is-accepting-jobs", IppValue::Boolean(true)),
            attribute(
                "marker-names",
                IppValue::Array(vec![text("Black"), text("Cyan")]),
            ),
            attribute(
                "marker-levels",
                IppValue::Array(vec![IppValue::Integer(80), IppValue::Integer(-1)]),
            ),
            attribute(
                "marker-colors",
                IppValue::Array(vec![text("#000000"), text("#00FFFF")]),
            ),
            attribute(
                "marker-types",
                IppValue::Array(vec![keyword("toner"), keyword("toner")]),
            ),
            attribute("copies-supported", IppValue::new_range_of_integer(1, 99)),
            attribute(
                "sides-supported",
                IppValue::Array(vec![keyword("one-sided"), keyword("two-sided-long-edge")]),
            ),
            attribute(
                "print-color-mode-supported",
                IppValue::Array(vec![keyword("color"), keyword("monochrome")]),
            ),
        ],
    )
}

fn job_group(
    id: i32,
    state: i32,
    owner: &str,
) -> (DelimiterTag, Vec<ipp::attribute::IppAttribute>) {
    (
        DelimiterTag::JobAttributes,
        vec![
            attribute("job-id", IppValue::Integer(id)),
            attribute(
                "job-printer-uri",
                IppValue::new_uri("ipp://localhost:631/printers/front%20desk").unwrap(),
            ),
            attribute("job-name", name(&format!("doc {id}"))),
            attribute("job-originating-user-name", name(owner)),
            attribute("job-state", IppValue::Enum(state)),
            attribute("job-media-sheets-completed", IppValue::Integer(2)),
            attribute("time-at-creation", IppValue::Integer(1_700_000_000 + id)),
        ],
    )
}

fn cups(server: &MockServer) -> Cups {
    Cups::new(Url::parse(&server.uri()).unwrap())
}

#[tokio::test]
async fn lists_printers_with_status_and_default() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .and(Ipp(Operation::CupsGetPrinters, None))
        .respond_with(ipp(
            StatusCode::SuccessfulOk,
            vec![printer_group("office", 3), printer_group("lab", 4)],
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/"))
        .and(Ipp(Operation::CupsGetDefault, None))
        .respond_with(ipp(StatusCode::SuccessfulOk, vec![printer_group("lab", 4)]))
        .mount(&server)
        .await;

    let printers = cups(&server).printers().await.unwrap();

    assert_eq!(printers.len(), 2);
    let office = &printers[0];
    assert_eq!(office.name, "office");
    assert_eq!(office.description.as_deref(), Some("Front office"));
    assert_eq!(office.location.as_deref(), Some("Ground floor"));
    assert_eq!(office.make_and_model.as_deref(), Some("Acme Laser 3000"));
    assert_eq!(office.state, PrinterState::Idle);
    assert!(!office.is_default);
    assert!(office.accepting_jobs);
    assert_eq!(office.reasons.len(), 2);
    assert_eq!(office.supplies.len(), 2);
    assert_eq!(office.supplies[0].level, Some(80));
    assert_eq!(office.supplies[1].level, None);
    assert_eq!(office.capabilities.max_copies, 99);
    assert_eq!(
        office.capabilities.sides,
        vec![Sides::OneSided, Sides::TwoSidedLongEdge]
    );
    assert_eq!(
        office.capabilities.color_modes,
        vec![ColorMode::Color, ColorMode::Monochrome]
    );
    assert_eq!(printers[1].state, PrinterState::Processing);
    assert!(printers[1].is_default);
}

#[tokio::test]
async fn lists_printers_when_cups_has_no_default() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(Ipp(Operation::CupsGetPrinters, None))
        .respond_with(ipp(
            StatusCode::SuccessfulOk,
            vec![printer_group("office", 3)],
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(Ipp(Operation::CupsGetDefault, None))
        .respond_with(ipp(StatusCode::ClientErrorNotFound, vec![]))
        .mount(&server)
        .await;

    let printers = cups(&server).printers().await.unwrap();

    assert_eq!(printers.len(), 1);
    assert!(!printers[0].is_default);
}

#[tokio::test]
async fn submits_pdf_with_options() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/printers/front%20desk"))
        .and(Ipp(Operation::PrintJob, None))
        .respond_with(ipp(
            StatusCode::SuccessfulOk,
            vec![(
                DelimiterTag::JobAttributes,
                vec![attribute("job-id", IppValue::Integer(42))],
            )],
        ))
        .mount(&server)
        .await;

    let document = b"%PDF-1.7\nhello".to_vec();
    let id = cups(&server)
        .submit(PrintRequest {
            printer: "front desk".into(),
            document: Bytes::from(document.clone()),
            format: DocumentFormat::Pdf,
            job_name: "report.pdf".into(),
            user: "alice".into(),
            options: PrintOptions {
                copies: 2,
                sides: Sides::TwoSidedLongEdge,
                color_mode: ColorMode::Monochrome,
                page_ranges: Some("1-3,5".parse::<PageRanges>().unwrap()),
            },
        })
        .await
        .unwrap();
    assert_eq!(id, 42);

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let sent = parse(&requests[0].body);
    assert_eq!(
        sent.header().operation_or_status,
        Operation::PrintJob as i16
    );
    assert_eq!(
        attr(&sent, "requesting-user-name").as_deref(),
        Some("alice")
    );
    assert_eq!(attr(&sent, "job-name").as_deref(), Some("report.pdf"));
    assert_eq!(
        attr(&sent, "document-format").as_deref(),
        Some("application/pdf")
    );
    assert_eq!(attr(&sent, "sides").as_deref(), Some("two-sided-long-edge"));
    assert_eq!(
        attr(&sent, "print-color-mode").as_deref(),
        Some("monochrome")
    );
    assert_eq!(attr(&sent, "copies").as_deref(), Some("2"));
    assert!(attr(&sent, "page-ranges").is_some());
    // CUPS wants the ipp scheme in the attribute even though the request goes over HTTP.
    let uri = attr(&sent, "printer-uri").unwrap();
    assert!(uri.starts_with("ipp://127.0.0.1:"), "{uri}");
    assert!(uri.ends_with("/printers/front%20desk"), "{uri}");

    let mut payload = Vec::new();
    sent.into_payload().read_to_end(&mut payload).unwrap();
    assert_eq!(payload, document);
}

#[tokio::test]
async fn lists_jobs_newest_first() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .and(Ipp(
            Operation::GetJobs,
            Some(("which-jobs", "not-completed")),
        ))
        .respond_with(ipp(
            StatusCode::SuccessfulOk,
            vec![job_group(7, 5, "alice"), job_group(6, 3, "bob")],
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/"))
        .and(Ipp(Operation::GetJobs, Some(("which-jobs", "completed"))))
        .and(Ipp(Operation::GetJobs, Some(("limit", "20"))))
        .respond_with(ipp(
            StatusCode::SuccessfulOk,
            vec![job_group(4, 9, "alice"), job_group(5, 7, "bob")],
        ))
        .mount(&server)
        .await;

    let jobs = cups(&server).jobs().await.unwrap();

    let ids: Vec<i32> = jobs.iter().map(|j| j.id).collect();
    assert_eq!(ids, vec![7, 6, 5, 4]);
    assert_eq!(jobs[0].state, JobState::Processing);
    assert_eq!(jobs[0].printer, "front desk");
    assert_eq!(jobs[0].owner.as_deref(), Some("alice"));
    assert_eq!(jobs[0].name.as_deref(), Some("doc 7"));
    assert_eq!(jobs[0].pages, Some(2));
    assert_eq!(jobs[0].created, Some(1_700_000_007));
    assert_eq!(jobs[3].state, JobState::Completed);
}

async fn cancel_scenario(owner: Option<IppValue>) -> (Result<(), PrintError>, Option<String>) {
    let server = MockServer::start().await;
    let mut attributes = vec![attribute("job-id", IppValue::Integer(9))];
    if let Some(owner) = owner {
        attributes.push(attribute("job-originating-user-name", owner));
    }
    Mock::given(method("POST"))
        .and(Ipp(Operation::GetJobAttributes, None))
        .respond_with(ipp(
            StatusCode::SuccessfulOk,
            vec![(DelimiterTag::JobAttributes, attributes)],
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(Ipp(Operation::CancelJob, None))
        .respond_with(ipp(StatusCode::SuccessfulOk, vec![]))
        .mount(&server)
        .await;

    let result = cups(&server).cancel(9, "alice").await;

    let sent = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| parse(&r.body))
        .find(|r| r.header().operation_or_status == Operation::CancelJob as i16);
    let user = sent.and_then(|s| attr(&s, "requesting-user-name"));
    (result, user)
}

#[tokio::test]
async fn cancels_as_job_owner() {
    let (result, user) = cancel_scenario(Some(name("bob"))).await;
    result.unwrap();
    assert_eq!(user.as_deref(), Some("bob"));
}

#[tokio::test]
async fn cancels_as_requester_when_owner_hidden() {
    let (result, user) = cancel_scenario(None).await;
    result.unwrap();
    assert_eq!(user.as_deref(), Some("alice"));
}

#[tokio::test]
async fn cancels_as_requester_when_owner_empty() {
    let (result, user) = cancel_scenario(Some(name(""))).await;
    result.unwrap();
    assert_eq!(user.as_deref(), Some("alice"));
}

#[tokio::test]
async fn cancels_as_requester_when_owner_out_of_band() {
    // CUPS hides attributes with the out-of-band tags `unknown` (0x12) and `no-value` (0x13).
    for tag in [0x12, 0x13] {
        let (result, user) = cancel_scenario(Some(IppValue::new_other(tag, Bytes::new()))).await;
        result.unwrap();
        assert_eq!(user.as_deref(), Some("alice"), "tag {tag:#x}");
    }
}

#[tokio::test]
async fn cancelling_a_missing_job_is_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ipp(StatusCode::ClientErrorNotFound, vec![]))
        .mount(&server)
        .await;

    assert_eq!(
        cups(&server).cancel(3, "alice").await,
        Err(PrintError::JobNotFound(3))
    );
}

#[tokio::test]
async fn unknown_printer_is_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/printers/ghost"))
        .respond_with(ipp(StatusCode::ClientErrorNotFound, vec![]))
        .mount(&server)
        .await;

    assert_eq!(
        cups(&server).printer("ghost").await,
        Err(PrintError::PrinterNotFound("ghost".into()))
    );
}

#[tokio::test]
async fn refused_requests_are_reported_as_refused() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ipp(StatusCode::ClientErrorNotAuthorized, vec![]))
        .mount(&server)
        .await;

    assert!(matches!(
        cups(&server).printers().await,
        Err(PrintError::Refused(_))
    ));
}

#[tokio::test]
async fn http_errors_are_unavailable() {
    let server = MockServer::start().await;
    // CUPS answers a bad Host header with a plain HTTP 400.
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400))
        .mount(&server)
        .await;

    assert!(matches!(
        cups(&server).printers().await,
        Err(PrintError::Unavailable(_))
    ));
}

#[tokio::test]
async fn unreachable_cups_is_unavailable() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let cups = Cups::new(Url::parse(&format!("http://127.0.0.1:{port}")).unwrap());

    assert!(matches!(
        cups.printers().await,
        Err(PrintError::Unavailable(_))
    ));
}

#[tokio::test]
async fn slow_cups_times_out() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ipp(StatusCode::SuccessfulOk, vec![]).set_delay(Duration::from_secs(15)))
        .mount(&server)
        .await;
    let cups = Cups::with_timeout(
        Url::parse(&server.uri()).unwrap(),
        Duration::from_millis(500),
    );

    let started = Instant::now();
    let result = cups.printers().await;

    assert!(
        matches!(result, Err(PrintError::Unavailable(_))),
        "{result:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(12));
}
