//! Runs against SANE's virtual `test` backend. Needs libsane and a config dir enabling only that
//! backend, so it is ignored unless asked for:
//!
//! `RECTO_SANE_TESTS=1 SANE_CONFIG_DIR=$PWD/crates/scanning/tests/sane-config \
//!  cargo nextest run -p recto-scanning --run-ignored all`
//!
//! libsane allows one instance per process and reads its config once, so every test shares one
//! `SaneScanners`.

use std::sync::OnceLock;

use recto_scanning::{
    Area, OutputFormat, SaneScanners, ScanError, ScanMode, ScanRequest, ScanSettings,
    ScannerService, preview_settings,
};

fn scanners() -> &'static SaneScanners {
    static SANE: OnceLock<SaneScanners> = OnceLock::new();
    SANE.get_or_init(|| {
        assert!(
            std::env::var_os("RECTO_SANE_TESTS").is_some(),
            "set RECTO_SANE_TESTS=1 and SANE_CONFIG_DIR to run the SANE tests"
        );
        SaneScanners::start().expect("sane starts")
    })
}

fn request(settings: ScanSettings, format: OutputFormat) -> ScanRequest {
    ScanRequest {
        settings,
        format,
        single_page: false,
    }
}

#[tokio::test]
#[ignore = "needs libsane, see the module docs"]
async fn lists_test_device() {
    let list = scanners().scanners(true).await.unwrap();
    assert!(list.iter().any(|s| s.id == "test:0"), "{list:?}");
}

#[tokio::test]
#[ignore = "needs libsane, see the module docs"]
async fn reads_capabilities_of_test_device() {
    let caps = scanners().capabilities("test:0").await.unwrap();
    assert!(!caps.resolutions.is_empty());
    assert!(caps.modes.contains(&ScanMode::Color));
    assert!(caps.modes.contains(&ScanMode::Gray));
    assert!(caps.area.is_some());
}

#[tokio::test]
#[ignore = "needs libsane, see the module docs"]
async fn unknown_device_is_not_found() {
    let err = scanners().capabilities("test:nope").await.unwrap_err();
    assert_eq!(err, ScanError::NotFound("test:nope".into()));
}

#[tokio::test]
#[ignore = "needs libsane, see the module docs"]
async fn scans_area_at_resolution() {
    let mut settings = preview_settings(&scanners().capabilities("test:0").await.unwrap(), None);
    settings.resolution = 100;
    settings.mode = ScanMode::Color;
    settings.area = Some(Area {
        left: 0.0,
        top: 0.0,
        right: 50.0,
        bottom: 50.0,
    });
    let pages = scanners()
        .scan("test:0", request(settings, OutputFormat::Png))
        .await
        .unwrap();
    assert_eq!(pages.len(), 1);
    let (w, h) = (pages[0].width, pages[0].height);
    assert!(w.abs_diff(197) <= 2 && h.abs_diff(197) <= 2, "{w}x{h}");
    assert_eq!(pages[0].dpi, 100);
    // Encoded losslessly as soon as it was read.
    assert!(pages[0].data.starts_with(b"\x89PNG"));
}

#[tokio::test]
#[ignore = "needs libsane, see the module docs"]
async fn preview_then_scan() {
    let caps = scanners().capabilities("test:0").await.unwrap();
    let preview = scanners()
        .scan(
            "test:0",
            request(preview_settings(&caps, None), OutputFormat::Png),
        )
        .await
        .unwrap();
    assert_eq!(preview.len(), 1);
    let mut settings = preview_settings(&caps, None);
    settings.resolution = 100;
    settings.area = Some(Area {
        left: 10.0,
        top: 10.0,
        right: 60.0,
        bottom: 40.0,
    });
    let pages = scanners()
        .scan("test:0", request(settings, OutputFormat::Pdf))
        .await
        .unwrap();
    let (w, h) = (pages[0].width, pages[0].height);
    assert!(w.abs_diff(197) <= 2 && h.abs_diff(118) <= 2, "{w}x{h}");
}

/// The test backend's feeder holds ten pages.
const FEEDER: &str = "Automatic Document Feeder";

fn feeder_settings() -> ScanSettings {
    ScanSettings {
        resolution: 20,
        mode: ScanMode::Gray,
        source: Some(FEEDER.into()),
        area: None,
    }
}

#[tokio::test]
#[ignore = "needs libsane, see the module docs"]
async fn feeder_batches_are_kept_compressed() {
    let pages = scanners()
        .scan("test:0", request(feeder_settings(), OutputFormat::Pdf))
        .await
        .unwrap();
    assert_eq!(pages.len(), 10);
    for page in &pages {
        assert!(page.data.starts_with(&[0xFF, 0xD8]), "a JPEG");
        assert!(page.gray);
    }
    // PNG and JPEG keep the first page only, and nothing of the others.
    let pages = scanners()
        .scan("test:0", request(feeder_settings(), OutputFormat::Png))
        .await
        .unwrap();
    assert_eq!(pages.len(), 1);
}

#[tokio::test]
#[ignore = "needs libsane, see the module docs"]
async fn feeder_batches_keep_every_image_page_when_asked() {
    for format in [OutputFormat::Png, OutputFormat::Jpeg] {
        let pages = scanners()
            .scan_every_page("test:0", request(feeder_settings(), format))
            .await
            .unwrap();
        assert_eq!(pages.len(), 10, "{format:?}");
        assert!(pages.iter().all(|page| page.format == format));
    }
}

#[tokio::test]
#[ignore = "needs libsane, see the module docs"]
async fn a_single_page_request_reads_one_page_from_the_feeder() {
    let caps = scanners().capabilities("test:0").await.unwrap();
    let preview = preview_settings(&caps, Some(FEEDER.into()));
    assert_eq!(preview.source.as_deref(), Some(FEEDER));
    let pages = scanners()
        .scan(
            "test:0",
            ScanRequest {
                single_page: true,
                ..request(preview, OutputFormat::Png)
            },
        )
        .await
        .unwrap();
    assert_eq!(pages.len(), 1);
}
