//! Development mocks: with `RECTO_DEV_MOCKS=1`, CUPS and SANE are replaced by in-memory
//! services and every request is signed in as a development user, an administrator kept in the
//! database like any other, so the interface can be tried without a printer, a scanner or an
//! identity provider.
//!
//! It signs everyone in, so it must never ship: this module only exists in debug builds with
//! the `dev-mocks` feature (`mise run dev` enables it), and the crate refuses to compile with
//! that feature in a release build (see `lib.rs`).

use std::{path::PathBuf, sync::Arc, time::Duration};

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use recto_app::state::{AppState, User};
use recto_domain::users::{self, Identity};
use recto_printing::{
    Capabilities, ColorMode, DriverInfo, Job, JobState, Printer, PrinterState, Sides, StateReason,
    Supply,
    testing::{MockPrintService, MockPrinterAdmin},
};
use recto_scanning::MockScannerService;
use sea_orm::DatabaseConnection;

/// Whether the mocks were asked for. Only the exact value `1` counts, so a stray
/// `RECTO_DEV_MOCKS=0` does not turn them on.
pub fn requested(value: Option<&str>) -> bool {
    value == Some("1")
}

/// Who every request is signed in as: the user of a `dev-mocks` identity, created at the first
/// run and found again afterwards, always an administrator.
pub async fn development_user(database: &DatabaseConnection) -> recto_domain::Result<User> {
    let identity = Identity {
        issuer: "dev-mocks".into(),
        subject: "dev".into(),
        preferred_username: Some("dev".into()),
        name: Some("Development user".into()),
        administrator: Some(true),
        ..Identity::default()
    };
    let row = users::sign_in_with_identity(database, &identity).await?;
    Ok(crate::session::user(&row))
}

/// Middleware giving every request the development user. It runs before the session is read,
/// so a user signed in through the session still takes its place.
pub async fn sign_in(State(user): State<User>, mut request: Request, next: Next) -> Response {
    request.extensions_mut().insert(user);
    next.run(request).await
}

/// Swaps the services for mocks and returns the development user. Scans take a moment, as real
/// ones do, so the waiting states of the scan page can be seen; Save writes under `target/`
/// unless a folder is configured. Printer administration is always the mock, never a real
/// socket, so the Printers page shows its forms; they change nothing.
pub async fn apply(state: &mut AppState) -> recto_domain::Result<User> {
    state.print = Arc::new(
        MockPrintService::new()
            .with_printers(printers())
            .with_jobs(jobs()),
    );
    state.printer_admin = Some(Arc::new(MockPrinterAdmin::new().with_drivers(drivers())));
    state.scan =
        Arc::new(MockScannerService::default().with_scan_delay(Duration::from_millis(800)));
    if state.output_dir.is_none() {
        let dir = PathBuf::from("target/dev-scans");
        if let Err(error) = std::fs::create_dir_all(&dir) {
            tracing::warn!(%error, "the development scan folder cannot be created");
        }
        state.output_dir = Some(dir);
    }
    development_user(&state.database).await
}

/// A few drivers for the Add form's search, such as `laserjet` or `epson`.
fn drivers() -> Vec<DriverInfo> {
    [
        (
            "drv:///hpcups.drv/hp-laserjet_1020.ppd",
            "HP LaserJet 1020 hpcups",
        ),
        (
            "drv:///hpcups.drv/hp-laserjet_p1102.ppd",
            "HP LaserJet Professional P1102 hpcups",
        ),
        (
            "gutenprint.5.3://escp2-xp200/expert",
            "Epson Expression Home XP-200 - CUPS+Gutenprint",
        ),
        (
            "foo2zjs:0/ppd/foo2zjs/HP-LaserJet_1018.ppd",
            "HP LaserJet 1018 Foomatic/foo2zjs-z1",
        ),
    ]
    .into_iter()
    .map(|(name, make_and_model)| DriverInfo {
        name: name.into(),
        make_and_model: make_and_model.into(),
    })
    .collect()
}

fn supply(name: &str, level: Option<u8>, color: &str) -> Supply {
    Supply {
        name: name.into(),
        level,
        color: Some(color.into()),
        kind: "toner".into(),
    }
}

fn printers() -> Vec<Printer> {
    let reasons = |keywords: &[&str]| -> Vec<StateReason> {
        keywords
            .iter()
            .filter_map(|keyword| StateReason::parse(keyword))
            .collect()
    };
    vec![
        Printer {
            name: "office".into(),
            description: Some("Office laser".into()),
            location: Some("Study".into()),
            make_and_model: Some("Brother HL-L2350DW".into()),
            is_default: true,
            state: PrinterState::Idle,
            reasons: vec![],
            accepting_jobs: true,
            supplies: vec![supply("Black toner", Some(64), "#000000")],
            capabilities: Capabilities {
                max_copies: 99,
                sides: vec![
                    Sides::OneSided,
                    Sides::TwoSidedLongEdge,
                    Sides::TwoSidedShortEdge,
                ],
                color_modes: vec![ColorMode::Monochrome],
            },
        },
        Printer {
            name: "photo".into(),
            description: Some("Photo inkjet".into()),
            location: Some("Living room".into()),
            make_and_model: Some("Canon PIXMA".into()),
            is_default: false,
            state: PrinterState::Processing,
            reasons: reasons(&["marker-supply-low-warning"]),
            accepting_jobs: true,
            supplies: vec![
                supply("Cyan", Some(12), "#00AEEF"),
                supply("Magenta", Some(55), "#EC008C"),
                supply("Yellow", Some(80), "#FFF200"),
                supply("Black", None, "#000000"),
            ],
            capabilities: Capabilities {
                max_copies: 10,
                sides: vec![Sides::OneSided],
                color_modes: vec![ColorMode::Color, ColorMode::Monochrome],
            },
        },
        Printer {
            name: "garage".into(),
            description: None,
            location: None,
            make_and_model: None,
            is_default: false,
            state: PrinterState::Stopped,
            reasons: reasons(&["offline-report", "media-empty-error"]),
            accepting_jobs: false,
            supplies: vec![],
            capabilities: Capabilities {
                max_copies: 1,
                sides: vec![Sides::OneSided],
                color_modes: vec![ColorMode::Monochrome],
            },
        },
    ]
}

fn jobs() -> Vec<Job> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0));
    vec![
        Job {
            id: 41,
            printer: "photo".into(),
            name: Some("holiday.jpg".into()),
            owner: Some("dev".into()),
            state: JobState::Processing,
            pages: Some(1),
            created: Some(now - 90),
            counted_pages: None,
            cost: None,
        },
        Job {
            id: 40,
            printer: "office".into(),
            name: None,
            owner: None,
            state: JobState::Pending,
            pages: None,
            created: Some(now - 600),
            counted_pages: None,
            cost: None,
        },
        Job {
            id: 39,
            printer: "office".into(),
            name: Some("tax-return-2026.pdf".into()),
            owner: Some("dev".into()),
            state: JobState::Completed,
            pages: Some(12),
            created: Some(now - 3 * 86_400),
            counted_pages: None,
            cost: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_development_user_is_an_administrator() {
        let database = recto_domain::testing::database().await;
        let first = development_user(&database).await.unwrap();
        // A restart finds the same user.
        let again = development_user(&database).await.unwrap();
        assert_eq!(first, again);
        assert_eq!(first.username, "dev");
        assert_eq!(first.name, "Development user");
        assert!(first.is_administrator);
        assert!(!first.has_password);
    }

    #[test]
    fn only_one_turns_the_mocks_on() {
        assert!(requested(Some("1")));
        for value in [None, Some("0"), Some(""), Some("true")] {
            assert!(!requested(value), "{value:?}");
        }
    }

    #[test]
    fn the_mock_printers_cover_every_case() {
        let printers = printers();
        assert_eq!(
            printers.iter().filter(|printer| printer.is_default).count(),
            1
        );
        assert!(printers.iter().any(|printer| !printer.is_online()));
        assert!(
            printers
                .iter()
                .any(|printer| printer.capabilities.sides.len() > 1)
        );
        assert!(jobs().iter().any(|job| job.owner.is_none()));
    }
}
