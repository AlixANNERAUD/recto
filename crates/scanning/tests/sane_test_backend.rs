//! Runs against SANE's virtual `test` backend. Needs libsane and a config dir enabling only that
//! backend, so it is ignored unless asked for:
//!
//! `RECTO_SANE_TESTS=1 SANE_CONFIG_DIR=$PWD/crates/scanning/tests/sane-config \
//!  cargo nextest run -p recto-scanning --run-ignored all`
//!
//! libsane allows one instance per process and reads its config once, so every test shares one
//! `SaneScanners`.

use std::sync::OnceLock;

use recto_scanning::{Area, SaneScanners, ScanError, ScanMode, ScannerService, preview_settings};

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
    let mut settings = preview_settings(&scanners().capabilities("test:0").await.unwrap());
    settings.resolution = 100;
    settings.mode = ScanMode::Color;
    settings.area = Some(Area {
        left: 0.0,
        top: 0.0,
        right: 50.0,
        bottom: 50.0,
    });
    let pages = scanners().scan("test:0", settings).await.unwrap();
    assert_eq!(pages.len(), 1);
    let (w, h) = (pages[0].image.width(), pages[0].image.height());
    assert!(w.abs_diff(197) <= 2 && h.abs_diff(197) <= 2, "{w}x{h}");
    assert_eq!(pages[0].dpi, 100);
}

#[tokio::test]
#[ignore = "needs libsane, see the module docs"]
async fn preview_then_scan() {
    let caps = scanners().capabilities("test:0").await.unwrap();
    let preview = scanners()
        .scan("test:0", preview_settings(&caps))
        .await
        .unwrap();
    assert_eq!(preview.len(), 1);
    let mut settings = preview_settings(&caps);
    settings.resolution = 100;
    settings.area = Some(Area {
        left: 10.0,
        top: 10.0,
        right: 60.0,
        bottom: 40.0,
    });
    let pages = scanners().scan("test:0", settings).await.unwrap();
    let (w, h) = (pages[0].image.width(), pages[0].image.height());
    assert!(w.abs_diff(197) <= 2 && h.abs_diff(118) <= 2, "{w}x{h}");
}
