//! The LAN listener: TLS straight from the devices that found recto through DNS-SD, serving the
//! printer's and the scanners' addresses only, with URIs built on the address the device used.

use std::{
    net::{IpAddr, Ipv4Addr},
    path::Path,
};

use chrono::Utc;
use ipp::model::Operation as IppOperation;
use ipp::prelude::{DelimiterTag, IppAttribute, IppRequestResponse, IppValue, IppVersion, Uri};
use ipp::{parser::IppParser, reader::IppReader};
use recto::lan::{self, TlsListener};
use recto_domain::device_passwords;
use recto_printing::testing::sample_pdf;
use recto_scanning::escl;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use super::*;

/// A phone on the LAN: device passwords are accepted from it, the LAN listener being TLS.
const PHONE: [u8; 4] = [192, 168, 1, 20];
const LAN_HOST: &str = "recto-nas.local:8631";

fn ipp_request(operation: IppOperation) -> IppRequestResponse {
    let uri: Uri = "ipps://recto-nas.local:8631/ipp/print".parse().unwrap();
    IppRequestResponse::new(IppVersion::v2_0(), operation, Some(uri)).unwrap()
}

fn ipp_body(request: &IppRequestResponse, document: &[u8]) -> Vec<u8> {
    let mut body = request.to_bytes().to_vec();
    body.extend_from_slice(document);
    body
}

fn print_job(document: &[u8]) -> Vec<u8> {
    let mut request = ipp_request(IppOperation::PrintJob);
    request.attributes_mut().add(
        DelimiterTag::OperationAttributes,
        IppAttribute::with_name(
            "job-name",
            IppValue::new_name_without_language("lan.pdf").unwrap(),
        )
        .unwrap(),
    );
    ipp_body(&request, document)
}

fn parse_ipp(bytes: &[u8]) -> IppRequestResponse {
    IppParser::new(IppReader::new(std::io::Cursor::new(bytes.to_vec())))
        .parse()
        .unwrap()
}

/// The URI-valued attribute `name` of the first group tagged `tag`.
fn uri_of(answer: &IppRequestResponse, tag: DelimiterTag, name: &str) -> String {
    match answer
        .attributes()
        .groups_of(tag)
        .next()
        .and_then(|group| group.get(name))
        .map(|attribute| attribute.value().clone())
    {
        Some(IppValue::Uri(uri)) => uri.as_str().to_owned(),
        other => panic!("{name}: {other:?}"),
    }
}

/// An IPP post from the phone to the LAN listener, with Basic credentials holding `secret`.
fn lan_ipp(path: &str, host: Option<&str>, secret: Option<&str>, body: Vec<u8>) -> Request<Body> {
    let mut request = Request::post(path).header(header::CONTENT_TYPE, "application/ipp");
    if let Some(host) = host {
        request = request.header(header::HOST, host);
    }
    if let Some(secret) = secret {
        let credentials = data_encoding::BASE64.encode(format!("iphone:{secret}").as_bytes());
        request = request.header(header::AUTHORIZATION, format!("Basic {credentials}"));
    }
    from_address(request.body(Body::from(body)).unwrap(), PHONE)
}

async fn answer(harness: &Harness, request: Request<Body>) -> IppRequestResponse {
    let response = harness.send_lan(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    parse_ipp(&bytes)
}

#[tokio::test]
async fn lan_uris_are_built_on_the_host_the_device_used() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let secret = device_passwords::create(&harness.database, alice, "iPhone", Utc::now())
        .await
        .unwrap()
        .secret;

    let attributes = ipp_body(&ipp_request(IppOperation::GetPrinterAttributes), b"");
    let answer_ = answer(
        &harness,
        lan_ipp("/ipp/print", Some(LAN_HOST), None, attributes.clone()),
    )
    .await;
    assert_eq!(
        uri_of(
            &answer_,
            DelimiterTag::PrinterAttributes,
            "printer-uri-supported"
        ),
        "ipps://recto-nas.local:8631/ipp/print"
    );

    // The public URL is plain HTTP, yet a device on the LAN sends its password: the LAN
    // listener is TLS from the device to recto.
    let answer_ = answer(
        &harness,
        lan_ipp(
            "/ipp/print",
            Some(LAN_HOST),
            Some(&secret),
            print_job(&sample_pdf(1)),
        ),
    )
    .await;
    let job_uri = uri_of(&answer_, DelimiterTag::JobAttributes, "job-uri");
    assert!(
        job_uri.starts_with("ipps://recto-nas.local:8631/ipp/print/"),
        "{job_uri}"
    );
    // The same password is refused from that address on the main listener, over plain HTTP.
    let response = harness
        .send(lan_ipp(
            "/ipp/print",
            Some(LAN_HOST),
            Some(&secret),
            print_job(&sample_pdf(1)),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // A personal address keeps its secret, on the host the device used.
    let personal = format!("/device/{secret}/ipp/print");
    let answer_ = answer(
        &harness,
        lan_ipp(
            &personal,
            Some("192.168.1.10:8631"),
            None,
            attributes.clone(),
        ),
    )
    .await;
    assert_eq!(
        uri_of(
            &answer_,
            DelimiterTag::PrinterAttributes,
            "printer-uri-supported"
        ),
        format!("ipps://192.168.1.10:8631{personal}")
    );

    // An IPv6 address, as Android connects by address, keeps its brackets.
    let answer_ = answer(
        &harness,
        lan_ipp(
            "/ipp/print",
            Some("[fd00::10]:8631"),
            None,
            attributes.clone(),
        ),
    )
    .await;
    assert_eq!(
        uri_of(
            &answer_,
            DelimiterTag::PrinterAttributes,
            "printer-uri-supported"
        ),
        "ipps://[fd00::10]:8631/ipp/print"
    );

    // Without a usable Host, there is no address to answer with.
    for host in [
        None,
        Some("user@evil:8631"),
        Some("a b"),
        Some("nas/../x"),
        Some("[fd00::10"),
    ] {
        let response = harness
            .send_lan(lan_ipp("/ipp/print", host, None, attributes.clone()))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{host:?}");
    }
}

/// A TLS client trusting only the LAN listener's certificate.
fn trusting(certificate: &Path) -> reqwest::Client {
    let pem = std::fs::read(certificate).unwrap();
    reqwest::Client::builder()
        .tls_built_in_root_certs(false)
        .add_root_certificate(reqwest::Certificate::from_pem(&pem).unwrap())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

#[tokio::test]
async fn the_lan_listener_serves_only_device_routes() {
    let harness = Harness::new().await;
    let data = temporary_directory();
    let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let tls = lan::load_or_create(None, None, data.path(), "nas", &[loopback]).unwrap();
    let tcp = tokio::net::TcpListener::bind((loopback, 0)).await.unwrap();
    let address = tcp.local_addr().unwrap();
    let listener = TlsListener::new(tcp, tls.acceptor(), lan::HANDSHAKE_TIMEOUT);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(lan::serve(listener, harness.lan.clone(), async {
        let _ = stopped.await;
    }));

    let client = trusting(&data.path().join(lan::DIRECTORY).join("certificate.pem"));
    let base = format!("https://{address}");
    let printer = client
        .post(format!("{base}/ipp/print"))
        .header(header::CONTENT_TYPE, "application/ipp")
        .body(ipp_body(
            &ipp_request(IppOperation::GetPrinterAttributes),
            b"",
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(printer.status(), StatusCode::OK);
    let answer = parse_ipp(&printer.bytes().await.unwrap());
    assert_eq!(
        uri_of(
            &answer,
            DelimiterTag::PrinterAttributes,
            "printer-uri-supported"
        ),
        format!("ipps://{address}/ipp/print")
    );
    // The scanners are there too, asking for a device password.
    let scanner = client
        .get(format!(
            "{base}/eSCL/{}/ScannerCapabilities",
            escl::scanner_key("mock:0")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(scanner.status(), StatusCode::UNAUTHORIZED);

    // No page, no sign-in, no asset, no health check: a bare 404 for everything else.
    for path in [
        "/",
        "/health",
        "/sign-in",
        "/account",
        "/jobs",
        "/auth/login",
        "/pkg/recto.css",
        "/manifest.webmanifest",
        "/brand/icon-192.png",
    ] {
        let response = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(!response.headers().contains_key(header::LOCATION), "{path}");
        assert!(
            !response.headers().contains_key(header::SET_COOKIE),
            "{path}"
        );
    }
    let sign_in = client
        .post(format!("{base}/sign-in"))
        .form(&[("login", "alice"), ("password", ALICE_PASSWORD)])
        .send()
        .await
        .unwrap();
    assert_eq!(sign_in.status(), StatusCode::NOT_FOUND);

    // Plain HTTP gets no answer at all.
    let mut plain = tokio::net::TcpStream::connect(address).await.unwrap();
    plain
        .write_all(b"GET /health HTTP/1.1\r\nHost: nas\r\n\r\n")
        .await
        .unwrap();
    let mut received = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), plain.read_to_end(&mut received)).await;
    assert!(!received.starts_with(b"HTTP/"), "{received:?}");

    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("the listener stops")
        .unwrap()
        .unwrap();
}
