use std::collections::HashSet;

use bytes::{Bytes, BytesMut};
use chrono::{DateTime, TimeZone, Utc};
use ipp::model::Operation as IppOperation;
use ipp::prelude::{
    DelimiterTag, IppAttribute, IppRequestResponse, IppValue, IppVersion, StatusCode, Uri,
};
use ipp::{parser::IppParser, reader::IppReader};

use super::*;
use crate::{ColorMode, PageRanges, Sides};

const PLAIN: &str = "ipps://print.example.org:443/ipp/print";
const PERSONAL: &str = "ipps://print.example.org:443/device/s3cr3t/ipp/print";

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 23, 12, 0, 0).unwrap()
}

fn printer() -> VirtualPrinter {
    VirtualPrinter {
        name: "Home".to_owned(),
        make_and_model: "recto".to_owned(),
        uuid: VirtualPrinter::uuid_for("https://print.example.org"),
        more_info: "https://print.example.org/jobs".to_owned(),
        icons: vec![
            "https://print.example.org/icon-48.png".to_owned(),
            "https://print.example.org/icon-128.png".to_owned(),
        ],
        default_media: Media::A4,
        formats: DOCUMENT_FORMATS.to_vec(),
        started_at: Utc.with_ymd_and_hms(2026, 11, 23, 8, 0, 0).unwrap(),
        accepting_jobs: true,
        languages: vec!["en", "fr"],
    }
}

fn plain() -> PrinterUri {
    PrinterUri {
        uri: PLAIN.to_owned(),
        tls: true,
        basic: true,
    }
}

fn personal() -> PrinterUri {
    PrinterUri {
        uri: PERSONAL.to_owned(),
        tls: true,
        basic: false,
    }
}

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn find<'a>(attributes: &'a [IppAttribute], name: &str) -> Option<&'a IppValue> {
    attributes
        .iter()
        .find(|attribute| attribute.name().as_str() == name)
        .map(IppAttribute::value)
}

fn values(value: &IppValue) -> Vec<IppValue> {
    match value {
        IppValue::Array(values) => values.clone(),
        one => vec![one.clone()],
    }
}

fn keyword(value: &str) -> IppValue {
    IppValue::new_keyword(value).unwrap()
}

fn attribute(name: &str, value: IppValue) -> IppAttribute {
    IppAttribute::with_name(name, value).unwrap()
}

fn request(operation: IppOperation) -> IppRequestResponse {
    let uri: Uri = "ipp://print.example.org/ipp/print".parse().unwrap();
    IppRequestResponse::new(IppVersion::v2_0(), operation, Some(uri)).unwrap()
}

fn with_document(request: &IppRequestResponse, document: &[u8]) -> Bytes {
    let mut body = BytesMut::from(&request.to_bytes()[..]);
    body.extend_from_slice(document);
    body.freeze()
}

fn add(request: &mut IppRequestResponse, tag: DelimiterTag, name: &str, value: IppValue) {
    request.attributes_mut().add(tag, attribute(name, value));
}

fn parse(bytes: &Bytes) -> IppRequestResponse {
    IppParser::new(IppReader::new(std::io::Cursor::new(bytes.to_vec())))
        .parse()
        .unwrap()
}

fn names_of(incoming: &Incoming) -> Vec<&str> {
    incoming
        .unsupported
        .iter()
        .map(|a| a.name().as_str())
        .collect()
}

fn job(state: JobViewState) -> JobView {
    JobView {
        id: 7,
        name: "report.pdf".to_owned(),
        device: "Laptop".to_owned(),
        state,
        received_at: Utc.with_ymd_and_hms(2026, 11, 23, 11, 0, 0).unwrap(),
        pages: Some(2),
    }
}

#[test]
fn every_required_attribute_is_answered() {
    let printer = printer();
    let attributes =
        printer.printer_attributes(&plain(), &strings(&["all", "media-col-database"]), 0, now());

    for name in REQUIRED_PRINTER_ATTRIBUTES {
        assert!(find(&attributes, name).is_some(), "{name} is missing");
    }
    let mut seen = HashSet::new();
    for attribute in &attributes {
        assert!(
            seen.insert(attribute.name().as_str().to_owned()),
            "{} is answered twice",
            attribute.name()
        );
        // An empty array encodes without a value length and desyncs the client's parser.
        assert_ne!(
            attribute.value(),
            &IppValue::Array(Vec::new()),
            "{}",
            attribute.name()
        );
    }

    assert_eq!(
        values(find(&attributes, "ipp-versions-supported").unwrap()),
        vec![keyword("1.0"), keyword("1.1"), keyword("2.0")]
    );
    let operations: Vec<IppValue> = [
        0x02, 0x04, 0x05, 0x06, 0x08, 0x09, 0x0A, 0x0B, 0x39, 0x3B, 0x3C,
    ]
    .into_iter()
    .map(IppValue::Enum)
    .collect();
    assert_eq!(
        values(find(&attributes, "operations-supported").unwrap()),
        operations
    );
    let formats: Vec<IppValue> = [
        "application/pdf",
        "image/pwg-raster",
        "image/urf",
        "image/jpeg",
        "image/png",
        "application/octet-stream",
    ]
    .into_iter()
    .map(|format| IppValue::new_mime_media_type(format).unwrap())
    .collect();
    assert_eq!(
        values(find(&attributes, "document-format-supported").unwrap()),
        formats
    );
    assert_eq!(
        find(&attributes, "media-default"),
        Some(&keyword("iso_a4_210x297mm"))
    );
    assert_eq!(
        values(find(&attributes, "media-col-database").unwrap()).len(),
        Media::ALL.len()
    );
    assert_eq!(
        find(&attributes, "printer-device-id"),
        Some(
            &IppValue::new_text_without_language("MFG:recto;MDL:recto;CMD:PDF,PWG,URF,JPEG,PNG;")
                .unwrap()
        )
    );
    assert_eq!(
        find(&attributes, "printer-uuid"),
        Some(&IppValue::new_uri(format!("urn:uuid:{}", printer.uuid)).unwrap())
    );

    // The rasters recto converts: 8-bit grey and sRGB at 300 dpi, back sides upright.
    let dpi = IppValue::Resolution {
        cross_feed: 300,
        feed: 300,
        units: 3,
    };
    assert_eq!(
        find(&attributes, "pwg-raster-document-resolution-supported"),
        Some(&dpi)
    );
    assert_eq!(
        values(find(&attributes, "pwg-raster-document-type-supported").unwrap()),
        vec![keyword("sgray_8"), keyword("srgb_8")]
    );
    assert_eq!(
        find(&attributes, "pwg-raster-document-sheet-back"),
        Some(&keyword("normal"))
    );
    assert_eq!(
        values(find(&attributes, "urf-supported").unwrap()),
        ["V1.4", "CP1", "W8", "SRGB24", "RS300", "DM1", "IS1"]
            .map(keyword)
            .to_vec()
    );
    assert_eq!(
        find(&attributes, "print-quality-supported"),
        Some(&IppValue::Enum(4))
    );

    // Sent only when named, as CUPS does: it is large and most clients never read it.
    let all = printer.printer_attributes(&plain(), &strings(&["all"]), 0, now());
    assert!(find(&all, "media-col-database").is_none());
    assert_eq!(all.len() + 1, attributes.len());
    // No made-up ink levels (R-P5-5).
    assert!(
        all.iter()
            .all(|a| !a.name().as_str().starts_with("printer-supply"))
    );
}

#[test]
fn requested_attributes_are_filtered() {
    let attributes = printer().printer_attributes(
        &plain(),
        &strings(&["printer-name", "media-default"]),
        0,
        now(),
    );
    let names: Vec<&str> = attributes.iter().map(|a| a.name().as_str()).collect();
    assert_eq!(names, ["media-default", "printer-name"]);

    let jobs = printer().job_attributes(
        &job(JobViewState::Completed),
        &plain(),
        "Held",
        &strings(&["job-id", "job-uri"]),
        now(),
    );
    let names: Vec<&str> = jobs.iter().map(|a| a.name().as_str()).collect();
    assert_eq!(names, ["job-id", "job-uri"]);
}

#[test]
fn returned_uris_echo_the_personal_address() {
    let printer = printer();
    let attributes = printer.printer_attributes(&personal(), &strings(&["all"]), 0, now());
    let job = printer.job_attributes(
        &job(JobViewState::Completed),
        &personal(),
        "Held",
        &strings(&["all"]),
        now(),
    );
    assert_eq!(
        find(&attributes, "printer-uri-supported"),
        Some(&IppValue::new_uri(PERSONAL).unwrap())
    );
    assert_eq!(
        find(&job, "job-uri"),
        Some(&IppValue::new_uri(format!("{PERSONAL}/7")).unwrap())
    );
    assert_eq!(
        find(&job, "job-printer-uri"),
        Some(&IppValue::new_uri(PERSONAL).unwrap())
    );
    // Android refuses an address that asks for Basic (R-P5-2).
    assert_eq!(
        find(&attributes, "uri-authentication-supported"),
        Some(&keyword("none"))
    );
    assert_eq!(
        find(&attributes, "uri-security-supported"),
        Some(&keyword("tls"))
    );

    let attributes = printer.printer_attributes(&plain(), &strings(&["all"]), 0, now());
    let job = printer.job_attributes(
        &self::job(JobViewState::Completed),
        &plain(),
        "Held",
        &strings(&["all"]),
        now(),
    );
    for value in attributes.iter().chain(&job).map(IppAttribute::value) {
        assert!(!value.to_string().contains("/device/"), "{value}");
    }
    assert_eq!(
        find(&attributes, "uri-authentication-supported"),
        Some(&keyword("basic"))
    );
}

#[test]
fn the_header_is_read_from_eight_bytes() {
    assert_eq!(
        header(&[0x02, 0x00, 0x00, 0x0B, 0x00, 0x00, 0x00, 0x2A]),
        Header {
            version: (2, 0),
            operation: GET_PRINTER_ATTRIBUTES,
            request_id: 42,
        }
    );
}

#[test]
fn job_template_attributes_are_read() {
    let mut print = request(IppOperation::PrintJob);
    let job = DelimiterTag::JobAttributes;
    let operation = DelimiterTag::OperationAttributes;
    add(
        &mut print,
        operation,
        "job-name",
        IppValue::new_name_without_language("report").unwrap(),
    );
    add(
        &mut print,
        operation,
        "document-format",
        IppValue::new_mime_media_type("application/pdf").unwrap(),
    );
    add(&mut print, job, "copies", IppValue::Integer(3));
    add(&mut print, job, "sides", keyword("two-sided-long-edge"));
    add(&mut print, job, "print-color-mode", keyword("monochrome"));
    add(
        &mut print,
        job,
        "page-ranges",
        IppValue::RangeOfInteger { min: 1, max: 2 },
    );
    add(&mut print, job, "media", keyword("iso_a5_148x210mm"));
    let incoming = decode(with_document(&print, b"%PDF-1.7\n")).unwrap();

    let Operation::PrintJob {
        name,
        format,
        requested,
        document,
    } = &incoming.operation
    else {
        panic!("not a Print-Job: {:?}", incoming.operation);
    };
    assert_eq!(name.as_deref(), Some("report"));
    assert_eq!(format.as_deref(), Some("application/pdf"));
    assert_eq!(&document[..], b"%PDF-1.7\n");
    assert_eq!(
        requested,
        &Requested {
            copies: Some(3),
            sides: Some(Sides::TwoSidedLongEdge),
            color_mode: Some(ColorMode::Monochrome),
            page_ranges: Some(PageRanges(vec![(1, 2)])),
            media: Some(Media::A5),
        }
    );
    assert!(incoming.unsupported.is_empty());

    let mut odd = request(IppOperation::CreateJob);
    add(&mut odd, job, "copies", IppValue::Integer(0));
    add(&mut odd, job, "media", keyword("na_foo_1x1in"));
    let incoming = decode(odd.to_bytes()).unwrap();
    let Operation::CreateJob { requested, .. } = &incoming.operation else {
        panic!("not a Create-Job: {:?}", incoming.operation);
    };
    assert_eq!(requested, &Requested::default());
    let dropped: Vec<&str> = incoming
        .unsupported
        .iter()
        .map(|a| a.name().as_str())
        .collect();
    assert_eq!(dropped, ["copies", "media"]);
    // Dropped values are reported back in the unsupported group with status 0x0001.
    let answer = parse(&response(&incoming, "en", 0, Vec::new()));
    assert_eq!(answer.header().operation_or_status, 0x0001);
    let group = answer
        .attributes()
        .first_of(DelimiterTag::UnsupportedAttributes)
        .unwrap();
    assert_eq!(group.attributes().len(), 2);

    add(
        &mut odd,
        DelimiterTag::OperationAttributes,
        "ipp-attribute-fidelity",
        IppValue::Boolean(true),
    );
    // With fidelity, the job is refused, and the refusal names what was not supported
    // (RFC 8011 section 4.1.7).
    let refused = decode(odd.to_bytes()).unwrap();
    assert!(matches!(
        refused.operation,
        Operation::Refused(StatusCode::ClientErrorAttributesOrValuesNotSupported)
    ));
    let answer = parse(&response(
        &refused,
        "en",
        StatusCode::ClientErrorAttributesOrValuesNotSupported as u16,
        Vec::new(),
    ));
    assert_eq!(answer.header().operation_or_status, 0x040B);
    let group = answer
        .attributes()
        .first_of(DelimiterTag::UnsupportedAttributes)
        .unwrap();
    assert_eq!(group.attributes().len(), 2);

    // What the printer advertises for job creation is read, and checked against what it
    // supports, even though the document is already laid out.
    let mut advertised = request(IppOperation::CreateJob);
    add(&mut advertised, job, "print-quality", IppValue::Enum(4));
    add(
        &mut advertised,
        job,
        "orientation-requested",
        IppValue::Enum(4),
    );
    add(
        &mut advertised,
        job,
        "printer-resolution",
        IppValue::Resolution {
            cross_feed: 300,
            feed: 300,
            units: 3,
        },
    );
    assert!(
        decode(advertised.to_bytes())
            .unwrap()
            .unsupported
            .is_empty()
    );
    let mut unoffered = request(IppOperation::CreateJob);
    add(&mut unoffered, job, "print-quality", IppValue::Enum(5));
    add(
        &mut unoffered,
        job,
        "orientation-requested",
        IppValue::Enum(7),
    );
    add(
        &mut unoffered,
        job,
        "printer-resolution",
        IppValue::Resolution {
            cross_feed: 600,
            feed: 600,
            units: 3,
        },
    );
    let names = |incoming: &Incoming| -> Vec<String> {
        incoming
            .unsupported
            .iter()
            .map(|a| a.name().as_str().to_owned())
            .collect()
    };
    assert_eq!(
        names(&decode(unoffered.to_bytes()).unwrap()),
        [
            "orientation-requested",
            "print-quality",
            "printer-resolution"
        ]
    );
    add(
        &mut unoffered,
        DelimiterTag::OperationAttributes,
        "ipp-attribute-fidelity",
        IppValue::Boolean(true),
    );
    assert!(matches!(
        decode(unoffered.to_bytes()).unwrap().operation,
        Operation::Refused(StatusCode::ClientErrorAttributesOrValuesNotSupported)
    ));

    let mut ranges = request(IppOperation::CreateJob);
    add(
        &mut ranges,
        job,
        "page-ranges",
        IppValue::Array(vec![
            IppValue::RangeOfInteger { min: 5, max: 6 },
            IppValue::RangeOfInteger { min: 1, max: 3 },
        ]),
    );
    add(&mut ranges, job, "print-color-mode", keyword("auto"));
    let Operation::CreateJob { requested, .. } = decode(ranges.to_bytes()).unwrap().operation
    else {
        panic!("not a Create-Job");
    };
    assert_eq!(
        requested.page_ranges,
        Some(PageRanges(vec![(1, 3), (5, 6)]))
    );
    assert_eq!(requested.color_mode, None);
}

// A size in media-col, as macOS sends it, is matched to the named medium.
#[test]
fn media_col_sizes_are_matched() {
    let mut create = request(IppOperation::CreateJob);
    let size = IppValue::Collection(
        [
            ("x-dimension".try_into().unwrap(), IppValue::Integer(21590)),
            ("y-dimension".try_into().unwrap(), IppValue::Integer(27940)),
        ]
        .into(),
    );
    let media_col = IppValue::Collection([("media-size".try_into().unwrap(), size)].into());
    add(
        &mut create,
        DelimiterTag::JobAttributes,
        "media-col",
        media_col,
    );
    let Operation::CreateJob { requested, .. } = decode(create.to_bytes()).unwrap().operation
    else {
        panic!("not a Create-Job");
    };
    assert_eq!(requested.media, Some(Media::Letter));
}

#[test]
fn job_ids_come_from_job_uri_too() {
    let mut cancel =
        IppRequestResponse::new(IppVersion::v1_1(), IppOperation::CancelJob, None).unwrap();
    add(
        &mut cancel,
        DelimiterTag::OperationAttributes,
        "job-uri",
        IppValue::new_uri(format!("{PERSONAL}/7")).unwrap(),
    );
    assert!(matches!(
        decode(cancel.to_bytes()).unwrap().operation,
        Operation::CancelJob { job_id: 7 }
    ));

    let mut get = request(IppOperation::GetJobAttributes);
    add(
        &mut get,
        DelimiterTag::OperationAttributes,
        "job-id",
        IppValue::Integer(9),
    );
    let Operation::GetJobAttributes { job_id, requested } =
        decode(get.to_bytes()).unwrap().operation
    else {
        panic!("not a Get-Job-Attributes");
    };
    assert_eq!((job_id, requested), (9, strings(&["all"])));

    // No job named at all.
    let close = request(IppOperation::CloseJob);
    assert_eq!(
        decode(close.to_bytes()).unwrap_err(),
        StatusCode::ClientErrorBadRequest
    );
}

#[test]
fn unknown_operations_and_versions_are_refused() {
    let mut unknown = request(IppOperation::GetJobs);
    unknown.header_mut().operation_or_status = 0x0045;
    assert!(matches!(
        decode(unknown.to_bytes()).unwrap().operation,
        Operation::Unsupported(0x0045)
    ));

    let mut future = request(IppOperation::GetPrinterAttributes);
    future.header_mut().version = IppVersion(0x0300);
    assert_eq!(
        decode(future.to_bytes()).unwrap_err(),
        StatusCode::ServerErrorVersionNotSupported
    );
    // Only the versions recto speaks, as ipp-versions-supported says: 1.0, 1.1 and 2.0.
    for version in [0x0102, 0x0201, 0x0202, 0x0203, 0x0209] {
        let mut odd = request(IppOperation::GetPrinterAttributes);
        odd.header_mut().version = IppVersion(version);
        assert_eq!(
            decode(odd.to_bytes()).unwrap_err(),
            StatusCode::ServerErrorVersionNotSupported,
            "{version:x}"
        );
    }
    for version in [IppVersion::v1_0(), IppVersion::v1_1(), IppVersion::v2_0()] {
        let mut old = request(IppOperation::GetPrinterAttributes);
        old.header_mut().version = version;
        assert_eq!(decode(old.to_bytes()).unwrap().version, version);
    }
}

#[test]
fn malformed_requests_are_bad_requests() {
    let bad = StatusCode::ClientErrorBadRequest;

    let mut zero = request(IppOperation::GetPrinterAttributes);
    zero.header_mut().request_id = 0;
    assert_eq!(decode(zero.to_bytes()).unwrap_err(), bad);

    assert_eq!(
        decode(Bytes::from_static(&[2, 0, 0, 0x0B])).unwrap_err(),
        bad
    );
    let truncated = request(IppOperation::GetPrinterAttributes).to_bytes();
    assert_eq!(
        decode(truncated.slice(..truncated.len() - 3)).unwrap_err(),
        bad
    );

    let mut no_target =
        IppRequestResponse::new(IppVersion::v2_0(), IppOperation::GetJobs, None).unwrap();
    assert_eq!(decode(no_target.to_bytes()).unwrap_err(), bad);
    no_target.attributes_mut().groups_mut()[0]
        .attributes_mut()
        .remove(0);
    assert_eq!(decode(no_target.to_bytes()).unwrap_err(), bad);

    let mut latin = request(IppOperation::GetJobs);
    latin.attributes_mut().groups_mut()[0].attributes_mut()[0] = attribute(
        "attributes-charset",
        IppValue::new_charset("iso-8859-1").unwrap(),
    );
    assert_eq!(
        decode(latin.to_bytes()).unwrap_err(),
        StatusCode::ClientErrorCharsetNotSupported
    );

    let mut send = request(IppOperation::SendDocument);
    add(
        &mut send,
        DelimiterTag::OperationAttributes,
        "job-id",
        IppValue::Integer(3),
    );
    assert_eq!(decode(send.to_bytes()).unwrap_err(), bad);
    add(
        &mut send,
        DelimiterTag::OperationAttributes,
        "last-document",
        IppValue::Boolean(true),
    );
    let Operation::SendDocument {
        job_id,
        last,
        document,
        ..
    } = decode(with_document(&send, b"%PDF-")).unwrap().operation
    else {
        panic!("not a Send-Document");
    };
    assert_eq!((job_id, last, &document[..]), (3, true, &b"%PDF-"[..]));

    let mut compressed = request(IppOperation::PrintJob);
    add(
        &mut compressed,
        DelimiterTag::OperationAttributes,
        "compression",
        keyword("gzip"),
    );
    let refused = decode(compressed.to_bytes()).unwrap();
    assert!(matches!(
        refused.operation,
        Operation::Refused(StatusCode::ClientErrorCompressionNotSupported)
    ));
    assert_eq!(names_of(&refused), ["compression"]);

    // A printer group, then a job group. Built by hand: `to_bytes` reorders groups.
    let get = request(IppOperation::GetJobs).to_bytes();
    let mut misordered = BytesMut::from(&get[..get.len() - 1]);
    misordered.extend_from_slice(&[0x04, 0x02, 0x03]);
    assert_eq!(decode(misordered.freeze()).unwrap_err(), bad);
}

#[test]
fn get_jobs_reads_which_jobs_and_limit() {
    let Operation::GetJobs {
        which,
        limit,
        job_ids,
        requested,
    } = decode(request(IppOperation::GetJobs).to_bytes())
        .unwrap()
        .operation
    else {
        panic!("not a Get-Jobs");
    };
    assert_eq!((which, limit), (WhichJobs::NotCompleted, None));
    assert!(job_ids.is_empty());
    assert_eq!(requested, strings(&["job-id", "job-uri"]));

    let mut completed = request(IppOperation::GetJobs);
    let operation = DelimiterTag::OperationAttributes;
    add(
        &mut completed,
        operation,
        "which-jobs",
        keyword("completed"),
    );
    add(&mut completed, operation, "limit", IppValue::Integer(5));
    add(
        &mut completed,
        operation,
        "requested-attributes",
        IppValue::Array(vec![keyword("job-name"), keyword("job-state")]),
    );
    add(&mut completed, operation, "job-ids", IppValue::Integer(7));
    let Operation::GetJobs {
        which,
        limit,
        job_ids,
        requested,
    } = decode(completed.to_bytes()).unwrap().operation
    else {
        panic!("not a Get-Jobs");
    };
    assert_eq!((which, limit), (WhichJobs::Completed, Some(5)));
    assert_eq!(job_ids, [7]);
    assert_eq!(requested, strings(&["job-name", "job-state"]));

    let mut mine = request(IppOperation::CancelMyJobs);
    add(
        &mut mine,
        operation,
        "job-ids",
        IppValue::Array(vec![IppValue::Integer(4), IppValue::Integer(5)]),
    );
    assert!(matches!(
        decode(mine.to_bytes()).unwrap().operation,
        Operation::CancelMyJobs { job_ids } if job_ids == [4, 5]
    ));
}

#[test]
fn jobs_report_their_state() {
    let printer = printer();
    let all = strings(&["all"]);
    let held = printer.job_attributes(
        &job(JobViewState::Completed),
        &plain(),
        "Held in recto",
        &all,
        now(),
    );
    assert_eq!(find(&held, "job-state"), Some(&IppValue::Enum(9)));
    assert_eq!(
        find(&held, "job-state-reasons"),
        Some(&keyword("job-completed-successfully"))
    );
    assert_eq!(
        find(&held, "job-state-message"),
        Some(&IppValue::new_text_without_language("Held in recto").unwrap())
    );
    assert_eq!(find(&held, "job-impressions"), Some(&IppValue::Integer(2)));
    assert_eq!(
        find(&held, "job-originating-user-name"),
        Some(&IppValue::new_name_without_language("Laptop").unwrap())
    );

    let pending = printer.job_attributes(
        &job(JobViewState::Pending),
        &plain(),
        "Waiting",
        &all,
        now(),
    );
    assert_eq!(find(&pending, "job-state"), Some(&IppValue::Enum(3)));
    assert_eq!(
        find(&pending, "job-state-reasons"),
        Some(&keyword("job-incoming"))
    );
    assert_eq!(
        find(&pending, "time-at-completed"),
        Some(&IppValue::NoValue)
    );

    // Printing directly was refused: still completed for the device, whose part is done, but
    // not successfully.
    let refused = printer.job_attributes(
        &job(JobViewState::CompletedWithErrors),
        &plain(),
        "Not printed",
        &all,
        now(),
    );
    assert_eq!(find(&refused, "job-state"), Some(&IppValue::Enum(9)));
    assert_eq!(
        find(&refused, "job-state-reasons"),
        Some(&keyword("job-completed-with-errors"))
    );
    assert_eq!(
        find(&refused, "number-of-documents"),
        Some(&IppValue::Integer(1))
    );

    let created =
        printer.created_job_attributes(&job(JobViewState::Completed), &plain(), "Held", now());
    let names: Vec<&str> = created.iter().map(|a| a.name().as_str()).collect();
    assert_eq!(
        names,
        [
            "job-id",
            "job-uri",
            "job-state",
            "job-state-reasons",
            "job-state-message"
        ]
    );
}

#[test]
fn responses_carry_raw_statuses_and_one_group_per_job() {
    let mut get = request(IppOperation::GetJobs);
    get.header_mut().request_id = 77;
    let incoming = decode(get.to_bytes()).unwrap();
    let printer = printer();
    let all = strings(&["job-id"]);
    let groups = vec![
        (
            DelimiterTag::JobAttributes,
            printer.job_attributes(&job(JobViewState::Completed), &plain(), "", &all, now()),
        ),
        (
            DelimiterTag::JobAttributes,
            printer.job_attributes(&job(JobViewState::Pending), &plain(), "", &all, now()),
        ),
    ];
    assert_eq!(incoming.language, "en");
    let answer = parse(&response(&incoming, "en", 0, groups));
    assert_eq!(answer.header().request_id, 77);
    assert_eq!(answer.header().version, IppVersion::v2_0());
    assert_eq!(
        answer
            .attributes()
            .groups_of(DelimiterTag::JobAttributes)
            .count(),
        2
    );

    // server-error-too-many-jobs is not in the ipp crate's StatusCode (R-P5-3).
    let answer = parse(&response(&incoming, "en", TOO_MANY_JOBS, Vec::new()));
    assert_eq!(answer.header().operation_or_status, 0x050B);
    let operation = answer
        .attributes()
        .first_of(DelimiterTag::OperationAttributes)
        .unwrap();
    assert!(operation.get("attributes-charset").is_some());

    // The response is labelled with the language its messages are in.
    let answer = parse(&response(&incoming, "fr", 0, Vec::new()));
    let language = answer
        .attributes()
        .first_of(DelimiterTag::OperationAttributes)
        .and_then(|operation| operation.get("attributes-natural-language"))
        .and_then(|attribute| attribute.value().as_natural_language().cloned());
    assert_eq!(language.as_deref(), Some("fr"));

    let refused = parse(&refusal(&header(&[3, 0, 0, 2, 0, 0, 0, 5]), 0x0503));
    assert_eq!(refused.header().operation_or_status, 0x0503);
    assert_eq!(refused.header().request_id, 5);
    assert_eq!(refused.header().version, IppVersion::v2_0());
}

#[test]
fn media_names_round_trip() {
    for media in Media::ALL {
        assert_eq!(media.keyword().parse::<Media>(), Ok(media));
    }
    assert!("na_foo_1x1in".parse::<Media>().is_err());
}

#[test]
fn the_printer_accepts_its_formats() {
    let printer = printer();
    assert!(printer.accepts("application/pdf"));
    assert!(printer.accepts("application/octet-stream"));
    assert!(!printer.accepts("application/postscript"));
    assert_eq!(
        VirtualPrinter::uuid_for("https://print.example.org"),
        VirtualPrinter::uuid_for("https://print.example.org")
    );
    assert_ne!(
        VirtualPrinter::uuid_for("https://print.example.org"),
        VirtualPrinter::uuid_for("https://other.example.org")
    );
}

#[test]
fn unknown_which_jobs_are_refused() {
    let mut get = request(IppOperation::GetJobs);
    add(
        &mut get,
        DelimiterTag::OperationAttributes,
        "which-jobs",
        keyword("fetchable"),
    );
    let incoming = decode(get.to_bytes()).unwrap();
    assert!(matches!(
        incoming.operation,
        Operation::GetJobs {
            which: WhichJobs::Unsupported,
            ..
        }
    ));
    // RFC 8011 section 4.1.7: the refusal names the attribute and its value.
    let answer = response(
        &incoming,
        "en",
        StatusCode::ClientErrorAttributesOrValuesNotSupported as u16,
        Vec::new(),
    );
    let answer = IppParser::new(IppReader::new(std::io::Cursor::new(answer.to_vec())))
        .parse()
        .unwrap();
    assert_eq!(answer.header().operation_or_status, 0x040B);
    let unsupported = answer
        .attributes()
        .groups_of(DelimiterTag::UnsupportedAttributes)
        .next()
        .expect("an unsupported-attributes group");
    assert_eq!(
        unsupported.get("which-jobs").map(|a| a.value().clone()),
        Some(keyword("fetchable"))
    );
}

#[test]
fn unsupported_values_that_cannot_encode_go_back_out_of_band() {
    // An unclosed collection parses as an empty set, which would encode without a length.
    for value in [
        IppValue::Array(Vec::new()),
        IppValue::Array(vec![IppValue::Integer(1), IppValue::Array(Vec::new())]),
    ] {
        let echoed = super::operations::echoable(&attribute("copies", value));
        assert_eq!(echoed.name().as_str(), "copies");
        assert!(matches!(echoed.value(), IppValue::Other { tag: 0x10, .. }));
    }
    let kept = super::operations::echoable(&attribute("copies", IppValue::Integer(0)));
    assert_eq!(kept.value(), &IppValue::Integer(0));
}

#[test]
fn extra_operation_attributes_join_the_operation_group() {
    let incoming = decode(request(IppOperation::GetJobs).to_bytes()).unwrap();
    let message = attribute(
        "status-message",
        IppValue::new_text_without_language("Held").unwrap(),
    );
    let answer = parse(&response(
        &incoming,
        "en",
        0,
        vec![(DelimiterTag::OperationAttributes, vec![message])],
    ));
    let operation = answer
        .attributes()
        .first_of(DelimiterTag::OperationAttributes)
        .unwrap();
    assert!(operation.get("attributes-charset").is_some());
    assert!(operation.get("status-message").is_some());
}

#[test]
fn long_job_names_are_cut_and_a_closed_queue_says_so() {
    let printer = VirtualPrinter {
        accepting_jobs: false,
        ..printer()
    };
    let long = JobView {
        name: "é".repeat(200),
        ..job(JobViewState::Completed)
    };
    let attributes = printer.job_attributes(&long, &plain(), "", &strings(&["job-name"]), now());
    let IppValue::NameWithoutLanguage(name) = find(&attributes, "job-name").unwrap() else {
        panic!("no job-name");
    };
    assert!(name.as_str().len() <= 255);
    assert!(name.as_str().chars().all(|c| c == 'é'));
    let attributes =
        printer.printer_attributes(&plain(), &strings(&["printer-is-accepting-jobs"]), 0, now());
    assert_eq!(
        find(&attributes, "printer-is-accepting-jobs"),
        Some(&IppValue::Boolean(false))
    );
}

/// A Get-Printer-Attributes whose operation group ends with `tail`, written by hand: the `ipp`
/// crate's encoder cannot nest collections past what it was given, and a value list is an
/// attribute and its unnamed additional values.
fn get_printer_attributes_with(tail: &[u8]) -> Bytes {
    let get = request(IppOperation::GetPrinterAttributes).to_bytes();
    let mut body = BytesMut::from(&get[..get.len() - 1]);
    body.extend_from_slice(tail);
    body.extend_from_slice(&[0x03]);
    body.freeze()
}

/// One value: tag, name, value, each length on two bytes.
fn entry(tag: u8, name: &str, value: &[u8]) -> Vec<u8> {
    let mut bytes = vec![tag];
    bytes.extend_from_slice(&u16::try_from(name.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(name.as_bytes());
    bytes.extend_from_slice(&u16::try_from(value.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(value);
    bytes
}

/// `depth` collections, each the only member of the one around it.
fn nested(depth: usize) -> Vec<u8> {
    let mut bytes = entry(0x34, "media-col", b"");
    for _ in 1..depth {
        bytes.extend(entry(0x4A, "", b"m"));
        bytes.extend(entry(0x34, "", b""));
    }
    for _ in 0..depth {
        bytes.extend(entry(0x37, "", b""));
    }
    bytes
}

#[test]
fn deeply_nested_collections_are_refused_on_a_small_stack() {
    // About 4,000 levels fit in the 64 KiB read without credentials. The `ipp` crate clones
    // each collection as it closes, recursively: parsed, this overflows the stack or takes
    // minutes. Refused, it costs one pass over the bytes.
    let body = get_printer_attributes_with(&nested(4000));
    assert!(body.len() <= 64 * 1024, "{} bytes", body.len());
    let started = std::time::Instant::now();
    let decoded = std::thread::Builder::new()
        .stack_size(128 * 1024)
        .spawn(move || decode(body).map(|_| ()))
        .unwrap()
        .join()
        .expect("decoding does not overflow the stack");
    assert_eq!(decoded, Err(StatusCode::ClientErrorBadRequest));
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn the_attribute_section_is_bounded() {
    let bad = Err(StatusCode::ClientErrorBadRequest);
    let decoded = |tail: &[u8]| decode(get_printer_attributes_with(tail)).map(|_| ());

    // Four levels are plenty: recto reads media-col and its media-size.
    assert_eq!(decoded(&nested(4)), Ok(()));
    assert_eq!(decoded(&nested(5)), bad);
    // Collections must close, and close only what was opened.
    let unclosed = nested(2);
    assert_eq!(decoded(&unclosed[..unclosed.len() - 5]), bad);
    let mut extra = nested(2);
    extra.extend(entry(0x37, "", b""));
    assert_eq!(decoded(&extra), bad);

    // 64 values for one attribute, 1,000 attributes in all.
    let values = |count: usize| {
        let mut bytes = entry(0x44, "requested-attributes", b"all");
        for _ in 1..count {
            bytes.extend(entry(0x44, "", b"printer-name"));
        }
        bytes
    };
    assert_eq!(decoded(&values(64)), Ok(()));
    assert_eq!(decoded(&values(65)), bad);
    // The request already carries three: charset, language and printer URI.
    let attributes = |count: usize| {
        (0..count)
            .flat_map(|index| entry(0x21, &format!("x-{index}"), &[0, 0, 0, 1]))
            .collect::<Vec<u8>>()
    };
    assert_eq!(decoded(&attributes(997)), Ok(()));
    assert_eq!(decoded(&attributes(998)), bad);
    // Members of a collection count as attributes too.
    let mut members = entry(0x34, "media-col", b"");
    for index in 0..998 {
        members.extend(entry(0x4A, "", format!("x-{index}").as_bytes()));
        members.extend(entry(0x21, "", &[0, 0, 0, 1]));
    }
    members.extend(entry(0x37, "", b""));
    assert_eq!(decoded(&members), bad);
}

#[test]
fn a_cups_get_printer_attributes_still_parses() {
    // What CUPS's ipp backend (Linux, and macOS, which embeds it) asks before each job.
    const CUPS: [&str; 24] = [
        "compression-supported",
        "copies-supported",
        "cups-version",
        "document-format-supported",
        "job-password-encryption-supported",
        "marker-colors",
        "marker-high-levels",
        "marker-levels",
        "marker-low-levels",
        "marker-message",
        "marker-names",
        "marker-types",
        "media-col-supported",
        "multiple-document-handling-supported",
        "operations-supported",
        "print-color-mode-supported",
        "print-scaling-supported",
        "printer-alert",
        "printer-alert-description",
        "printer-is-accepting-jobs",
        "printer-mandatory-job-attributes",
        "printer-state",
        "printer-state-message",
        "printer-state-reasons",
    ];
    let mut get = request(IppOperation::GetPrinterAttributes);
    add(
        &mut get,
        DelimiterTag::OperationAttributes,
        "requesting-user-name",
        IppValue::new_name_without_language("alice").unwrap(),
    );
    add(
        &mut get,
        DelimiterTag::OperationAttributes,
        "requested-attributes",
        IppValue::Array(CUPS.iter().map(|name| keyword(name)).collect()),
    );
    let Operation::GetPrinterAttributes { requested } = decode(get.to_bytes()).unwrap().operation
    else {
        panic!("not a Get-Printer-Attributes");
    };
    assert_eq!(requested, strings(&CUPS));

    // And what it asks when a queue is added: the media database too.
    let mut add_queue = request(IppOperation::GetPrinterAttributes);
    add(
        &mut add_queue,
        DelimiterTag::OperationAttributes,
        "requested-attributes",
        IppValue::Array(vec![keyword("all"), keyword("media-col-database")]),
    );
    assert!(decode(add_queue.to_bytes()).is_ok());
}
