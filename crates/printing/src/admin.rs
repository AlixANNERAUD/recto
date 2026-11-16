//! Administering CUPS: adding, editing and removing printers over IPP.
//!
//! Every request names recto to CUPS with `Authorization: PeerCred recto`, which CUPS accepts
//! only on its local socket, from a process whose uid is the one of the `recto` account and
//! whose group is a `SystemGroup`. Over TCP, CUPS refuses every administration request.

use std::io::Cursor;
use std::path::Path;
use std::time::Duration;

use ipp::attribute::IppAttribute;
use ipp::model::{DelimiterTag, IppVersion, Operation, StatusCode};
use ipp::parser::IppParser;
use ipp::reader::IppReader;
use ipp::request::IppRequestResponse;
use ipp::value::IppValue;
use url::Url;

use crate::cups::{error_chain, printer_groups, string};
use crate::{AdminError, Driver, DriverInfo, NewPrinter};

/// Long enough for a busy CUPS, short enough that a page never hangs on a dead one.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Adding a printer with the `everywhere` driver makes CUPS query it, and CUPS waits about 31
/// seconds for an address that does not answer before it gives up.
const ADD_TIMEOUT: Duration = Duration::from_secs(60);

/// Changes the printers of CUPS.
#[async_trait::async_trait]
pub trait PrinterAdmin: Send + Sync + 'static {
    async fn add(&self, printer: NewPrinter) -> Result<(), AdminError>;
    /// Changes the description and the location; `None` leaves a text as it is.
    async fn describe(
        &self,
        name: &str,
        description: Option<String>,
        location: Option<String>,
    ) -> Result<(), AdminError>;
    async fn set_default(&self, name: &str) -> Result<(), AdminError>;
    async fn pause(&self, name: &str) -> Result<(), AdminError>;
    async fn resume(&self, name: &str) -> Result<(), AdminError>;
    /// Lets the queue take jobs again.
    async fn accept(&self, name: &str) -> Result<(), AdminError>;
    /// Makes the queue turn new jobs away.
    async fn reject(&self, name: &str) -> Result<(), AdminError>;
    async fn delete(&self, name: &str) -> Result<(), AdminError>;
    /// The drivers whose make and model contain every word of `search`, at most `limit`.
    async fn drivers(&self, search: &str, limit: u32) -> Result<Vec<DriverInfo>, AdminError>;
}

/// Talks to the administration endpoint of CUPS.
pub struct CupsAdmin {
    /// Where `/admin/` is: `http://localhost/` over the socket, whatever the host is.
    base: Url,
    client: reqwest::Client,
}

impl CupsAdmin {
    /// Reaches CUPS through its Unix socket. A socket that does not exist yet is not an error
    /// here: CUPS may start after recto, and each request reports it then.
    pub fn over_socket(path: &Path) -> Result<Self, AdminError> {
        let client = Self::builder()
            .unix_socket(path)
            .build()
            .map_err(|error| AdminError::Unavailable(error_chain(&error)))?;
        Ok(CupsAdmin {
            base: Url::parse("http://localhost/").expect("a valid address"),
            client,
        })
    }

    /// Sends the same requests over TCP. For tests: CUPS refuses administration over TCP.
    pub fn over_tcp(base: Url) -> Self {
        let client = Self::builder()
            .build()
            .expect("the TLS backend initializes");
        CupsAdmin { base, client }
    }

    fn builder() -> reqwest::ClientBuilder {
        // The workspace reqwest (rustls with ring) does the HTTP instead of the client bundled
        // with `ipp`, as in `Cups`.
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(REQUEST_TIMEOUT)
    }

    async fn send(
        &self,
        request: IppRequestResponse,
        timeout: Duration,
    ) -> Result<IppRequestResponse, AdminError> {
        let mut url = self.base.clone();
        url.set_path("/admin/");
        url.set_query(None);
        let response = self
            .client
            .post(url)
            // CUPS answers any other Host with a 400, and the socket has no host of its own.
            .header("host", "localhost")
            .header("content-type", "application/ipp")
            .header("authorization", format!("PeerCred {USER}"))
            .timeout(timeout)
            .body(request.to_bytes().to_vec())
            .send()
            .await
            .map_err(|error| AdminError::Unavailable(error_chain(&error)))?;
        let status = response.status();
        match status.as_u16() {
            401 | 403 | 426 => return Err(AdminError::Refused),
            _ if !status.is_success() => {
                return Err(AdminError::Unavailable(format!(
                    "CUPS answered with HTTP status {status}"
                )));
            }
            _ => {}
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| AdminError::Unavailable(error_chain(&error)))?;
        IppParser::new(IppReader::new(Cursor::new(bytes.to_vec())))
            .parse()
            .map_err(|error| AdminError::Unavailable(format!("unreadable CUPS answer: {error}")))
    }

    /// Sends an operation that answers with nothing but its status.
    async fn run(&self, request: IppRequestResponse, timeout: Duration) -> Result<(), AdminError> {
        check(&self.send(request, timeout).await?)
    }

    async fn on_printer(&self, operation: Operation, name: &str) -> Result<(), AdminError> {
        self.run(printer_request(operation, name)?, REQUEST_TIMEOUT)
            .await
    }

    async fn ppds(
        &self,
        make_and_model: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<DriverInfo>, AdminError> {
        let mut request = new_request(Operation::CupsGetPPDs)?;
        if let Some(search) = make_and_model {
            add(
                &mut request,
                DelimiterTag::OperationAttributes,
                "ppd-make-and-model",
                IppValue::new_text_without_language(search).map_err(invalid)?,
            )?;
        }
        if let Some(limit) = limit {
            let limit = i32::try_from(limit).unwrap_or(i32::MAX);
            add(
                &mut request,
                DelimiterTag::OperationAttributes,
                "limit",
                IppValue::Integer(limit),
            )?;
        }
        add(
            &mut request,
            DelimiterTag::OperationAttributes,
            "requested-attributes",
            IppValue::Array(vec![
                IppValue::new_keyword("ppd-name").map_err(invalid)?,
                IppValue::new_keyword("ppd-make-and-model").map_err(invalid)?,
            ]),
        )?;
        let response = self.send(request, REQUEST_TIMEOUT).await?;
        // No driver matches: CUPS may say so with "not found" instead of an empty list.
        if response.header().status_code() == StatusCode::ClientErrorNotFound {
            return Ok(Vec::new());
        }
        check(&response)?;
        Ok(printer_groups(&response)
            .filter_map(|group| {
                Some(DriverInfo {
                    name: string(group, "ppd-name")?,
                    make_and_model: string(group, "ppd-make-and-model")?,
                })
            })
            .collect())
    }
}

#[async_trait::async_trait]
impl PrinterAdmin for CupsAdmin {
    async fn add(&self, printer: NewPrinter) -> Result<(), AdminError> {
        let mut request = printer_request(Operation::CupsAddModifyPrinter, &printer.name)?;
        let group = DelimiterTag::PrinterAttributes;
        add(
            &mut request,
            group,
            "device-uri",
            IppValue::new_uri(printer.device_uri).map_err(invalid)?,
        )?;
        let ppd = match printer.driver {
            Driver::Everywhere => "everywhere".to_owned(),
            Driver::Ppd(name) => name,
        };
        add(
            &mut request,
            group,
            "ppd-name",
            IppValue::new_name_without_language(ppd).map_err(invalid)?,
        )?;
        add(
            &mut request,
            group,
            "printer-is-accepting-jobs",
            IppValue::Boolean(true),
        )?;
        // Idle: CUPS creates a queue stopped otherwise.
        add(&mut request, group, "printer-state", IppValue::Enum(3))?;
        add_texts(&mut request, printer.description, printer.location)?;
        self.run(request, ADD_TIMEOUT).await
    }

    async fn describe(
        &self,
        name: &str,
        description: Option<String>,
        location: Option<String>,
    ) -> Result<(), AdminError> {
        if description.is_none() && location.is_none() {
            return Ok(());
        }
        let mut request = printer_request(Operation::CupsAddModifyPrinter, name)?;
        add_texts(&mut request, description, location)?;
        self.run(request, REQUEST_TIMEOUT).await
    }

    async fn set_default(&self, name: &str) -> Result<(), AdminError> {
        self.on_printer(Operation::CupsSetDefault, name).await
    }

    async fn pause(&self, name: &str) -> Result<(), AdminError> {
        self.on_printer(Operation::PausePrinter, name).await
    }

    async fn resume(&self, name: &str) -> Result<(), AdminError> {
        self.on_printer(Operation::ResumePrinter, name).await
    }

    async fn accept(&self, name: &str) -> Result<(), AdminError> {
        self.on_printer(Operation::CupsAcceptJobs, name).await
    }

    async fn reject(&self, name: &str) -> Result<(), AdminError> {
        self.on_printer(Operation::CupsRejectJobs, name).await
    }

    async fn delete(&self, name: &str) -> Result<(), AdminError> {
        self.on_printer(Operation::CupsDeletePrinter, name).await
    }

    async fn drivers(&self, search: &str, limit: u32) -> Result<Vec<DriverInfo>, AdminError> {
        let words: Vec<String> = search.split_whitespace().map(str::to_lowercase).collect();
        let matching = |drivers: Vec<DriverInfo>| -> Vec<DriverInfo> {
            drivers
                .into_iter()
                .filter(|driver| {
                    let model = driver.make_and_model.to_lowercase();
                    words.iter().all(|word| model.contains(word))
                })
                .take(usize::try_from(limit).unwrap_or(usize::MAX))
                .collect()
        };
        if words.is_empty() {
            return Ok(matching(self.ppds(None, Some(limit)).await?));
        }
        // CUPS may match the filter as a prefix of the make, which a search for a model alone
        // never satisfies. Its limit applies before recto's word filter, so an answer that
        // loses everything to the filter is retried with the whole list.
        let found = matching(self.ppds(Some(&words.join(" ")), Some(limit)).await?);
        if !found.is_empty() {
            return Ok(found);
        }
        Ok(matching(self.ppds(None, None).await?))
    }
}

/// The account CUPS knows recto as (`/etc/passwd` of the CUPS container).
const USER: &str = "recto";

/// The queue address CUPS expects, on its own host name: the host of the request is always
/// `localhost`, and the name is one path segment.
fn printer_uri(name: &str) -> Result<IppValue, AdminError> {
    let mut url = Url::parse("ipp://localhost/printers/").expect("a valid address");
    url.path_segments_mut()
        .map_err(|()| AdminError::Rejected("the printer address cannot hold a name".into()))?
        .pop_if_empty()
        .push(name);
    IppValue::new_uri(url.as_str()).map_err(invalid)
}

/// A name or text CUPS cannot take (too long, say), caught before asking it.
fn invalid(error: impl std::fmt::Display) -> AdminError {
    AdminError::Rejected(error.to_string())
}

fn new_request(operation: Operation) -> Result<IppRequestResponse, AdminError> {
    IppRequestResponse::new(IppVersion::v1_1(), operation, None).map_err(invalid)
}

/// A request aimed at one queue.
fn printer_request(operation: Operation, name: &str) -> Result<IppRequestResponse, AdminError> {
    let mut request = new_request(operation)?;
    add(
        &mut request,
        DelimiterTag::OperationAttributes,
        "printer-uri",
        printer_uri(name)?,
    )?;
    Ok(request)
}

fn add(
    request: &mut IppRequestResponse,
    group: DelimiterTag,
    name: &str,
    value: IppValue,
) -> Result<(), AdminError> {
    request.attributes_mut().add(
        group,
        IppAttribute::with_name(name, value).map_err(invalid)?,
    );
    Ok(())
}

fn add_texts(
    request: &mut IppRequestResponse,
    description: Option<String>,
    location: Option<String>,
) -> Result<(), AdminError> {
    for (name, text) in [
        ("printer-info", description),
        ("printer-location", location),
    ] {
        if let Some(text) = text {
            add(
                request,
                DelimiterTag::PrinterAttributes,
                name,
                IppValue::new_text_without_language(text).map_err(invalid)?,
            )?;
        }
    }
    Ok(())
}

/// Turns the IPP status of an answer into the error the interface can explain.
fn check(response: &IppRequestResponse) -> Result<(), AdminError> {
    let status = response.header().status_code();
    if status.is_success() {
        return Ok(());
    }
    let message = response
        .attributes()
        .groups_of(DelimiterTag::OperationAttributes)
        .find_map(|group| string(group, "status-message"))
        // CUPS sends no message for some errors: the standard text of the status reads better
        // than its name.
        .unwrap_or_else(|| status.to_string());
    Err(match status {
        // What CUPS answers when it cannot talk to the printer at the address.
        StatusCode::ServerErrorDeviceError => AdminError::Unreachable(message),
        _ => AdminError::Rejected(message),
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use ipp::attribute::IppAttribute;
    use ipp::model::{DelimiterTag, IppVersion, Operation, StatusCode};
    use ipp::parser::IppParser;
    use ipp::reader::IppReader;
    use ipp::request::IppRequestResponse;
    use ipp::value::IppValue;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::testing::{attribute, ipp_response};

    /// A new directory under `target/test-tmp` of the workspace, never the system temporary
    /// directory, removed when dropped. The path stays short enough for a Unix socket.
    fn temporary_directory() -> tempfile::TempDir {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp");
        std::fs::create_dir_all(&base).expect("create target/test-tmp");
        tempfile::TempDir::new_in(base).expect("create a temporary directory")
    }

    fn ok() -> Vec<u8> {
        ipp_response(StatusCode::SuccessfulOk, vec![])
    }

    async fn server_answering(response: ResponseTemplate) -> (MockServer, CupsAdmin) {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/admin/"))
            .respond_with(response)
            .mount(&server)
            .await;
        let admin = CupsAdmin::over_tcp(Url::parse(&server.uri()).unwrap());
        (server, admin)
    }

    async fn server_with_ipp(body: Vec<u8>) -> (MockServer, CupsAdmin) {
        server_answering(ResponseTemplate::new(200).set_body_bytes(body)).await
    }

    fn parse(body: &[u8]) -> IppRequestResponse {
        IppParser::new(IppReader::new(Cursor::new(body.to_vec())))
            .parse()
            .expect("an IPP request")
    }

    /// The only request the server saw, with its IPP body parsed.
    async fn the_request(server: &MockServer) -> (wiremock::Request, IppRequestResponse) {
        let mut requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "one request");
        let request = requests.remove(0);
        let ipp = parse(&request.body);
        (request, ipp)
    }

    fn operation(request: &IppRequestResponse) -> i16 {
        request.header().operation_or_status
    }

    fn find<'a>(
        request: &'a IppRequestResponse,
        tag: DelimiterTag,
        name: &str,
    ) -> Option<&'a IppValue> {
        request
            .attributes()
            .groups_of(tag)
            .find_map(|group| group.get(name))
            .map(IppAttribute::value)
    }

    fn text(request: &IppRequestResponse, tag: DelimiterTag, name: &str) -> Option<String> {
        find(request, tag, name).map(ToString::to_string)
    }

    #[tokio::test]
    async fn every_request_names_recto_by_peer_credentials() {
        let (server, admin) = server_with_ipp(ok()).await;
        admin.pause("office").await.unwrap();

        let (request, ipp) = the_request(&server).await;
        assert_eq!(request.method.as_str(), "POST");
        assert_eq!(request.url.path(), "/admin/");
        assert_eq!(request.headers["host"], "localhost");
        assert_eq!(request.headers["content-type"], "application/ipp");
        assert_eq!(request.headers["authorization"], "PeerCred recto");
        assert_eq!(operation(&ipp), Operation::PausePrinter as i16);
        assert_eq!(
            text(&ipp, DelimiterTag::OperationAttributes, "printer-uri").as_deref(),
            Some("ipp://localhost/printers/office")
        );
    }

    #[tokio::test]
    async fn the_other_operations_target_the_queue() {
        for (expected, call) in [
            (Operation::ResumePrinter, 0),
            (Operation::CupsAcceptJobs, 1),
            (Operation::CupsRejectJobs, 2),
            (Operation::CupsSetDefault, 3),
            (Operation::CupsDeletePrinter, 4),
        ] {
            let (server, admin) = server_with_ipp(ok()).await;
            match call {
                0 => admin.resume("my printer").await,
                1 => admin.accept("my printer").await,
                2 => admin.reject("my printer").await,
                3 => admin.set_default("my printer").await,
                _ => admin.delete("my printer").await,
            }
            .unwrap();
            let (_, ipp) = the_request(&server).await;
            assert_eq!(operation(&ipp), expected as i16);
            // The name is one path segment, so it cannot reach another queue or operation.
            assert_eq!(
                text(&ipp, DelimiterTag::OperationAttributes, "printer-uri").as_deref(),
                Some("ipp://localhost/printers/my%20printer")
            );
        }
    }

    #[tokio::test]
    async fn adding_sends_the_address_and_driver() {
        let (server, admin) = server_with_ipp(ok()).await;
        admin
            .add(NewPrinter {
                name: "office".into(),
                device_uri: "ipp://192.168.1.20/ipp/print".into(),
                driver: Driver::Everywhere,
                description: Some("Second floor".into()),
                location: Some("Hall".into()),
            })
            .await
            .unwrap();

        let (_, ipp) = the_request(&server).await;
        assert_eq!(operation(&ipp), Operation::CupsAddModifyPrinter as i16);
        let printer = DelimiterTag::PrinterAttributes;
        assert_eq!(
            text(&ipp, DelimiterTag::OperationAttributes, "printer-uri").as_deref(),
            Some("ipp://localhost/printers/office")
        );
        assert_eq!(
            text(&ipp, printer, "device-uri").as_deref(),
            Some("ipp://192.168.1.20/ipp/print")
        );
        assert_eq!(
            text(&ipp, printer, "ppd-name").as_deref(),
            Some("everywhere")
        );
        assert_eq!(
            find(&ipp, printer, "printer-is-accepting-jobs"),
            Some(&IppValue::Boolean(true))
        );
        assert_eq!(
            find(&ipp, printer, "printer-state"),
            Some(&IppValue::Enum(3))
        );
        assert_eq!(
            text(&ipp, printer, "printer-info").as_deref(),
            Some("Second floor")
        );
        assert_eq!(
            text(&ipp, printer, "printer-location").as_deref(),
            Some("Hall")
        );
    }

    #[tokio::test]
    async fn adding_with_a_ppd_names_it_and_skips_empty_texts() {
        let (server, admin) = server_with_ipp(ok()).await;
        admin
            .add(NewPrinter {
                name: "office".into(),
                device_uri: "socket://192.168.1.20".into(),
                driver: Driver::Ppd("drv:///hp.drv/laserjet.ppd".into()),
                description: None,
                location: None,
            })
            .await
            .unwrap();

        let (_, ipp) = the_request(&server).await;
        let printer = DelimiterTag::PrinterAttributes;
        assert_eq!(
            text(&ipp, printer, "ppd-name").as_deref(),
            Some("drv:///hp.drv/laserjet.ppd")
        );
        assert!(find(&ipp, printer, "printer-info").is_none());
        assert!(find(&ipp, printer, "printer-location").is_none());
    }

    #[tokio::test]
    async fn describing_sends_only_the_text() {
        let (server, admin) = server_with_ipp(ok()).await;
        admin
            .describe("office", Some("Second floor".into()), Some("Hall".into()))
            .await
            .unwrap();

        let (_, ipp) = the_request(&server).await;
        assert_eq!(operation(&ipp), Operation::CupsAddModifyPrinter as i16);
        let printer = DelimiterTag::PrinterAttributes;
        assert_eq!(
            text(&ipp, printer, "printer-info").as_deref(),
            Some("Second floor")
        );
        assert_eq!(
            text(&ipp, printer, "printer-location").as_deref(),
            Some("Hall")
        );
        assert!(find(&ipp, printer, "device-uri").is_none());
        assert!(find(&ipp, printer, "ppd-name").is_none());
        assert!(find(&ipp, printer, "printer-state").is_none());
        assert!(find(&ipp, printer, "printer-is-accepting-jobs").is_none());
    }

    #[tokio::test]
    async fn describing_nothing_asks_nothing() {
        let (server, admin) = server_with_ipp(ok()).await;
        admin.describe("office", None, None).await.unwrap();
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn refusals_are_told_apart() {
        for code in [401, 403, 426] {
            let (_server, admin) = server_answering(ResponseTemplate::new(code)).await;
            assert_eq!(
                admin.pause("office").await,
                Err(AdminError::Refused),
                "{code}"
            );
        }

        let message = |status, text: &str| {
            let mut response =
                IppRequestResponse::new_response(IppVersion::v1_1(), status, 1).unwrap();
            response.attributes_mut().add(
                DelimiterTag::OperationAttributes,
                attribute(
                    "status-message",
                    IppValue::new_text_without_language(text).unwrap(),
                ),
            );
            response.to_bytes().to_vec()
        };
        let (_server, admin) = server_with_ipp(message(
            StatusCode::ServerErrorDeviceError,
            "Unable to connect to printer",
        ))
        .await;
        assert_eq!(
            admin.pause("office").await,
            Err(AdminError::Unreachable(
                "Unable to connect to printer".into()
            ))
        );

        let (_server, admin) =
            server_with_ipp(message(StatusCode::ClientErrorBadRequest, "Bad name")).await;
        assert_eq!(
            admin.pause("office").await,
            Err(AdminError::Rejected("Bad name".into()))
        );

        let (_server, admin) = server_answering(ResponseTemplate::new(400)).await;
        assert!(matches!(
            admin.pause("office").await,
            Err(AdminError::Unavailable(_))
        ));
    }

    #[tokio::test]
    async fn an_answer_without_a_message_is_still_readable() {
        let bare = |status| {
            IppRequestResponse::new_response(IppVersion::v1_1(), status, 1)
                .unwrap()
                .to_bytes()
                .to_vec()
        };
        let (_server, admin) = server_with_ipp(bare(StatusCode::ServerErrorDeviceError)).await;
        let Err(AdminError::Unreachable(text)) = admin.pause("office").await else {
            panic!("an unreachable printer");
        };
        assert!(!text.contains("ServerError"), "{text}");
        assert!(!text.is_empty());

        let (_server, admin) = server_with_ipp(bare(StatusCode::ClientErrorNotPossible)).await;
        let Err(AdminError::Rejected(text)) = admin.pause("office").await else {
            panic!("a rejection");
        };
        assert_eq!(text, "Not possible");
    }

    fn ppd(name: &str, make_and_model: &str) -> (DelimiterTag, Vec<IppAttribute>) {
        (
            DelimiterTag::PrinterAttributes,
            vec![
                attribute(
                    "ppd-name",
                    IppValue::new_name_without_language(name).unwrap(),
                ),
                attribute(
                    "ppd-make-and-model",
                    IppValue::new_text_without_language(make_and_model).unwrap(),
                ),
            ],
        )
    }

    #[tokio::test]
    async fn drivers_are_searched_by_make_and_model() {
        let (server, admin) = server_with_ipp(ipp_response(
            StatusCode::SuccessfulOk,
            vec![
                ppd("hp-1.ppd", "HP LaserJet 4"),
                ppd("brother.ppd", "Brother HL-L2350"),
                ppd("hp-2.ppd", "HP LaserJet 5"),
                ppd("hp-3.ppd", "HP Laserjet 6"),
            ],
        ))
        .await;

        let drivers = admin.drivers("hp laserjet", 2).await.unwrap();
        assert_eq!(
            drivers,
            [
                DriverInfo {
                    name: "hp-1.ppd".into(),
                    make_and_model: "HP LaserJet 4".into()
                },
                DriverInfo {
                    name: "hp-2.ppd".into(),
                    make_and_model: "HP LaserJet 5".into()
                },
            ]
        );

        let (_, ipp) = the_request(&server).await;
        assert_eq!(operation(&ipp), Operation::CupsGetPPDs as i16);
        let operation_attributes = DelimiterTag::OperationAttributes;
        assert_eq!(
            text(&ipp, operation_attributes, "ppd-make-and-model").as_deref(),
            Some("hp laserjet")
        );
        assert_eq!(
            find(&ipp, operation_attributes, "limit"),
            Some(&IppValue::Integer(2))
        );
        let requested = find(&ipp, operation_attributes, "requested-attributes").unwrap();
        assert!(requested.to_string().contains("ppd-make-and-model"));
    }

    #[tokio::test]
    async fn a_search_the_server_cannot_match_falls_back_to_all_drivers() {
        // The first answer is empty (CUPS matched the search as a prefix of the make), the
        // second lists the PPDs.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/admin/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(ipp_response(StatusCode::ClientErrorNotFound, vec![])),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/admin/"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(ipp_response(
                StatusCode::SuccessfulOk,
                vec![ppd("hp.ppd", "HP LaserJet 4"), ppd("b.ppd", "Brother")],
            )))
            .mount(&server)
            .await;
        let admin = CupsAdmin::over_tcp(Url::parse(&server.uri()).unwrap());

        let drivers = admin.drivers("laserjet", 50).await.unwrap();
        assert_eq!(drivers.len(), 1);
        assert_eq!(drivers[0].name, "hp.ppd");
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn the_socket_is_used() {
        let directory = temporary_directory();
        let socket = directory.path().join("cups.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let fake = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut seen = Vec::new();
            let mut buffer = [0_u8; 4096];
            // Read the headers, then the body they announce.
            loop {
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0, "the client closed early");
                seen.extend_from_slice(&buffer[..read]);
                let Some(end) = seen.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let head = String::from_utf8_lossy(&seen[..end]).to_ascii_lowercase();
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .and_then(|value| value.trim().parse().ok())
                    .unwrap_or(0);
                if seen.len() >= end + 4 + length {
                    break;
                }
            }
            let body = ok();
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/ipp\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
            String::from_utf8_lossy(&seen).into_owned()
        });

        let admin = CupsAdmin::over_socket(&socket).unwrap();
        admin.pause("x").await.unwrap();

        let seen = fake.await.unwrap();
        let lowercase = seen.to_ascii_lowercase();
        assert!(seen.starts_with("POST /admin/ HTTP/1.1"), "{seen}");
        assert!(
            lowercase.contains("authorization: peercred recto"),
            "{seen}"
        );
        assert!(lowercase.contains("host: localhost\r\n"), "{seen}");
    }

    #[tokio::test]
    async fn a_missing_socket_is_unavailable() {
        let directory = temporary_directory();
        let admin = CupsAdmin::over_socket(&directory.path().join("absent.sock")).unwrap();
        assert!(matches!(
            admin.pause("x").await,
            Err(AdminError::Unavailable(_))
        ));
    }
}
