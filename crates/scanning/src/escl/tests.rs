use uuid::Uuid;

use super::*;
use crate::{Area, ScanMode, ScannerCapabilities, ScannerInfo, Source};

const UUID: &str = "0f3c1d5e-8a43-5b52-9d0e-3b6c1f2a4e7d";
const JOB: &str = "6f1c9a52-4e0b-4c55-9a3e-2d1f0b7c8e91";

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/escl/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn lide() -> ScannerInfo {
    ScannerInfo {
        id: "genesys:libusb:001:004".into(),
        vendor: "Canon".into(),
        model: "LiDE 300".into(),
        kind: "flatbed scanner".into(),
    }
}

fn a4() -> Area {
    Area {
        left: 0.0,
        top: 0.0,
        right: 215.9,
        bottom: 297.0,
    }
}

fn caps(sources: &[&str]) -> ScannerCapabilities {
    ScannerCapabilities {
        resolutions: vec![75, 150, 300],
        modes: vec![ScanMode::Color, ScanMode::Gray],
        sources: sources.iter().map(|s| Source::new(s)).collect(),
        area: Some(a4()),
    }
}

fn capabilities(caps: &ScannerCapabilities) -> String {
    capabilities_xml(
        &lide(),
        caps,
        UUID.parse().unwrap(),
        Some("https://recto-nas.local:8631/icon.png"),
        "https://recto.example.org/scan",
    )
}

#[test]
fn capabilities_follow_sane() {
    let xml = capabilities(&caps(&["Flatbed", "ADF Duplex"]));
    assert_eq!(xml, fixture("capabilities.xml"));
    assert!(xml.contains(&format!("<pwg:MakeAndModel>{MODEL} ")));
}

#[test]
fn capabilities_without_sources_offer_the_platen() {
    let xml = capabilities(&caps(&[]));
    assert!(xml.contains("<scan:PlatenInputCaps>"));
    assert!(!xml.contains("<scan:Adf>"));
}

#[test]
fn a_simplex_feeder_has_no_duplex() {
    let xml = capabilities(&caps(&["ADF"]));
    assert!(!xml.contains("<scan:Platen>"));
    assert!(xml.contains("<scan:AdfSimplexInputCaps>"));
    assert!(!xml.contains("AdfDuplexInputCaps"));
    assert!(!xml.contains("<scan:AdfOption>Duplex</scan:AdfOption>"));
}

#[test]
fn capabilities_without_geometry_offer_letter_by_a4() {
    let caps = ScannerCapabilities {
        area: None,
        modes: vec![ScanMode::Lineart, ScanMode::Gray],
        ..caps(&["Flatbed"])
    };
    let xml = capabilities(&caps);
    assert!(xml.contains("<scan:MaxWidth>2550</scan:MaxWidth>"));
    assert!(xml.contains("<scan:MaxHeight>3508</scan:MaxHeight>"));
    let modes = xml.find("<scan:ColorMode>BlackAndWhite1").unwrap();
    assert!(modes < xml.find("<scan:ColorMode>Grayscale8").unwrap());
}

#[test]
fn resolutions_too_large_for_a_colour_page_are_not_offered() {
    let caps = ScannerCapabilities {
        resolutions: vec![300, 600, 1200, 2400],
        ..caps(&["Flatbed"])
    };
    let xml = capabilities(&caps);
    assert!(xml.contains("<scan:XResolution>600</scan:XResolution>"));
    assert!(!xml.contains("<scan:XResolution>1200</scan:XResolution>"));
    assert!(xml.contains("<scan:MaxOpticalXResolution>600</scan:MaxOpticalXResolution>"));
}

#[test]
fn device_strings_are_escaped() {
    let info = ScannerInfo {
        vendor: "A&B".into(),
        model: "<Scan\u{1}er>".into(),
        ..lide()
    };
    let xml = capabilities_xml(
        &info,
        &caps(&[]),
        UUID.parse().unwrap(),
        None,
        "https://x/?a=1&b=2",
    );
    assert!(xml.contains("<pwg:MakeAndModel>recto A&amp;B &lt;Scaner&gt;</pwg:MakeAndModel>"));
    assert!(xml.contains("<scan:AdminURI>https://x/?a=1&amp;b=2</scan:AdminURI>"));
    assert!(!xml.contains("IconURI"));
}

#[test]
fn scanner_keys_are_short_and_stable() {
    assert_eq!(scanner_key("genesys:libusb:001:004"), "d83a3503");
    let a = scanner_uuid("https://recto.example.org/", "test:0");
    assert_eq!(a, scanner_uuid("https://recto.example.org/", "test:0"));
    assert_ne!(a, scanner_uuid("https://recto.example.org/", "test:1"));
    assert_ne!(a, scanner_uuid("https://other.example.org/", "test:0"));
    assert_eq!(a.get_version_num(), 5);
}

#[test]
fn own_scanners_are_recognised() {
    let advertised = ["Office scanner".to_owned()];
    assert!(is_own_scanner("recto 0.3", &advertised));
    assert!(is_own_scanner("recto Canon LiDE 300", &[]));
    assert!(is_own_scanner("Office scanner", &advertised));
    assert!(!is_own_scanner("Office scanner 2", &advertised));
    assert!(!is_own_scanner("CanoScan LiDE 300", &advertised));
}

#[test]
fn job_uris_are_built_on_the_callers_root() {
    let id: Uuid = JOB.parse().unwrap();
    let expected = format!("/eSCL/1a2b3c4d/ScanJobs/{JOB}");
    assert_eq!(job_uri("/eSCL/1a2b3c4d/", id), expected);
    assert_eq!(job_uri("/eSCL/1a2b3c4d", id), expected);
}

#[test]
fn status_lists_the_jobs() {
    let root = "/device/abcdefghijklmnopqrstuvwxyz234567/eSCL/1a2b3c4d/";
    let jobs = [JobStatusView {
        id: JOB.parse().unwrap(),
        state: EsclJobState::Processing,
        age_seconds: 3,
        images_completed: 0,
        images_to_transfer: 0,
    }];
    let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<scan:ScannerStatus xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm" xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03">
  <pwg:Version>2.63</pwg:Version>
  <pwg:State>Processing</pwg:State>
  <scan:AdfState>ScannerAdfLoaded</scan:AdfState>
  <scan:Jobs>
    <scan:JobInfo>
      <pwg:JobUri>/device/abcdefghijklmnopqrstuvwxyz234567/eSCL/1a2b3c4d/ScanJobs/6f1c9a52-4e0b-4c55-9a3e-2d1f0b7c8e91</pwg:JobUri>
      <pwg:JobUuid>6f1c9a52-4e0b-4c55-9a3e-2d1f0b7c8e91</pwg:JobUuid>
      <scan:Age>3</scan:Age>
      <pwg:ImagesCompleted>0</pwg:ImagesCompleted>
      <pwg:ImagesToTransfer>0</pwg:ImagesToTransfer>
      <pwg:JobState>Processing</pwg:JobState>
      <pwg:JobStateReasons>
        <pwg:JobStateReason>JobScanning</pwg:JobStateReason>
      </pwg:JobStateReasons>
    </scan:JobInfo>
  </scan:Jobs>
</scan:ScannerStatus>
"#;
    assert_eq!(
        status_xml(EsclState::Processing, Some(AdfState::Loaded), root, &jobs),
        expected
    );
}

#[test]
fn an_idle_flatbed_has_no_feeder_state_and_no_jobs() {
    let xml = status_xml(EsclState::Idle, None, "/eSCL/1a2b3c4d/", &[]);
    assert!(xml.contains("<pwg:State>Idle</pwg:State>"));
    assert!(!xml.contains("AdfState"));
    assert!(!xml.contains("Jobs"));
}

#[test]
fn every_job_state_has_its_reason() {
    for (state, name, reason, adf, adf_name) in [
        (
            EsclJobState::Completed,
            "Completed",
            "JobCompletedSuccessfully",
            AdfState::Empty,
            "ScannerAdfEmpty",
        ),
        (
            EsclJobState::Canceled,
            "Canceled",
            "JobCanceledByUser",
            AdfState::Jam,
            "ScannerAdfJam",
        ),
        (
            EsclJobState::Aborted,
            "Aborted",
            "AbortedBySystem",
            AdfState::DoorOpen,
            "ScannerAdfDoorOpen",
        ),
    ] {
        let job = JobStatusView {
            id: Uuid::nil(),
            state,
            age_seconds: 0,
            images_completed: 2,
            images_to_transfer: 1,
        };
        let xml = status_xml(EsclState::Idle, Some(adf), "/eSCL/k/", &[job]);
        assert!(xml.contains(&format!("<pwg:JobState>{name}</pwg:JobState>")));
        assert!(xml.contains(&format!(
            "<pwg:JobStateReason>{reason}</pwg:JobStateReason>"
        )));
        assert!(xml.contains(&format!("<scan:AdfState>{adf_name}</scan:AdfState>")));
        assert!(xml.contains("<pwg:ImagesCompleted>2</pwg:ImagesCompleted>"));
        assert!(xml.contains("<pwg:ImagesToTransfer>1</pwg:ImagesToTransfer>"));
    }
}

#[test]
fn image_info_describes_the_last_document() {
    let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<scan:ScanImageInfo xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm" xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03">
  <pwg:JobUri>/eSCL/1a2b3c4d/ScanJobs/6f1c9a52-4e0b-4c55-9a3e-2d1f0b7c8e91</pwg:JobUri>
  <pwg:JobUuid>6f1c9a52-4e0b-4c55-9a3e-2d1f0b7c8e91</pwg:JobUuid>
  <scan:ActualWidth>2550</scan:ActualWidth>
  <scan:ActualHeight>3508</scan:ActualHeight>
  <scan:ActualBytesPerLine>7650</scan:ActualBytesPerLine>
</scan:ScanImageInfo>
"#;
    let info = ImageInfo {
        width: 2550,
        height: 3508,
        gray: false,
    };
    assert_eq!(
        scan_image_info_xml("/eSCL/1a2b3c4d/", JOB.parse().unwrap(), &info),
        expected
    );
    let gray = ImageInfo { gray: true, ..info };
    assert_eq!(gray.bytes_per_line(), 2550);
}
