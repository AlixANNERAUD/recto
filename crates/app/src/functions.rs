//! Server functions: the data of the pages and the scan actions of the scan island. Each is
//! served at `/api/<endpoint>` behind the server's sign-in gate and cross-site check, and starts
//! with [`require_user`](crate::state::require_user) as well.
//!
//! Errors carry a translated message made on the server ([`messages::scan_error`],
//! `print.unavailable_banner`), never the error's internals; the details go to the log.
//!
//! [`messages::scan_error`]: crate::messages::scan_error

use leptos::{prelude::*, server_fn::codec::Json};
use recto_printing::{Job, Printer};
use recto_scanning::{OutputFormat, ScanSettings, ScannerCapabilities, ScannerInfo, Selection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A finished preview or scan, stored for the user until downloaded, saved or expired.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanOutcome {
    pub id: Uuid,
    pub file_name: String,
    pub pages: usize,
    /// Whether an output folder is configured, so Save can work.
    pub can_save: bool,
}

#[server(endpoint = "list_printers")]
pub async fn list_printers() -> Result<Vec<Printer>, ServerFnError> {
    let context = server::Context::new()?;
    let printers = context.state.print.printers().await;
    printers.map_err(|error| context.print_failure(&error))
}

#[server(endpoint = "list_jobs")]
pub async fn list_jobs() -> Result<Vec<Job>, ServerFnError> {
    let context = server::Context::new()?;
    let jobs = context.state.print.jobs().await;
    jobs.map_err(|error| context.print_failure(&error))
}

#[server(endpoint = "list_scanners")]
pub async fn list_scanners(refresh: bool) -> Result<Vec<ScannerInfo>, ServerFnError> {
    let context = server::Context::new()?;
    let scanners = context.state.scan.scanners(refresh).await;
    scanners.map_err(|error| context.scan_failure(&error))
}

// Named so its request type does not clash with the `ScannerCapabilities` model.
#[server(name = ScannerCapabilitiesRequest, endpoint = "scanner_capabilities")]
pub async fn scanner_capabilities(id: String) -> Result<ScannerCapabilities, ServerFnError> {
    let context = server::Context::new()?;
    let capabilities = context.state.scan.capabilities(&id).await;
    capabilities.map_err(|error| context.scan_failure(&error))
}

/// A quick low-resolution scan of the whole glass, stored as PNG for the preview image.
#[server(endpoint = "preview")]
pub async fn preview(id: String) -> Result<ScanOutcome, ServerFnError> {
    let context = server::Context::new()?;
    let scanned = async {
        let capabilities = context.state.scan.capabilities(&id).await?;
        let settings = recto_scanning::preview_settings(&capabilities);
        let pages = context.state.scan.scan(&id, settings).await?;
        context.store(pages, OutputFormat::Png).await
    };
    scanned.await.map_err(|error| context.scan_failure(&error))
}

/// Scans with the chosen settings. `selection` is the part of the preview to keep, as fractions;
/// without it, or on a scanner without geometry options, the whole area is scanned.
#[server(endpoint = "scan", input = Json)]
pub async fn scan(
    id: String,
    settings: ScanSettings,
    selection: Option<Selection>,
    format: OutputFormat,
) -> Result<ScanOutcome, ServerFnError> {
    let context = server::Context::new()?;
    let scanned = async {
        let capabilities = context.state.scan.capabilities(&id).await?;
        let settings = server::checked(settings, selection, &capabilities)?;
        let pages = context.state.scan.scan(&id, settings).await?;
        context.store(pages, format).await
    };
    scanned.await.map_err(|error| context.scan_failure(&error))
}

#[cfg(feature = "ssr")]
mod server {
    use std::sync::Arc;

    use embedded_i18n::Locale;
    use leptos::prelude::ServerFnError;
    use recto_authentication::User;
    use recto_printing::PrintError;
    use recto_scanning::{
        OutputFormat, ScanError, ScanSettings, ScannedPage, ScannerCapabilities, Selection,
    };

    use super::ScanOutcome;
    use crate::{
        i18n, messages,
        state::{AppState, require_user},
    };

    /// What every server function needs, read from the request before its first `await`.
    pub struct Context {
        pub user: User,
        pub state: AppState,
        pub locale: Locale,
    }

    impl Context {
        pub fn new() -> Result<Self, ServerFnError> {
            let user = require_user()?;
            let locale = i18n::locale();
            let state = crate::state::app_state()
                .ok_or_else(|| ServerFnError::new(messages::sign_in_required(locale)))?;
            Ok(Self {
                user,
                state,
                locale,
            })
        }

        /// CUPS's own reason is kept when it cannot be reached, so whoever runs recto can tell
        /// a wrong address from a stopped service; other failures get a plain message.
        pub fn print_failure(&self, error: &PrintError) -> ServerFnError {
            tracing::warn!(%error, "the printing service failed");
            let locale = self.locale;
            let message = match error {
                PrintError::Unavailable(reason) => format!(
                    "{}",
                    embedded_i18n::t!("print.unavailable_banner", reason = reason, locale = locale)
                ),
                _ => embedded_i18n::t!("print.errors.refused", locale = locale).to_owned(),
            };
            ServerFnError::new(message)
        }

        pub fn scan_failure(&self, error: &ScanError) -> ServerFnError {
            tracing::warn!(%error, user = self.user.username, "scanning failed");
            ServerFnError::new(messages::scan_error(error, self.locale))
        }

        /// Encodes the pages and keeps the file for the user. Both block, so they run off the
        /// async workers.
        pub async fn store(
            &self,
            pages: Vec<ScannedPage>,
            format: OutputFormat,
        ) -> Result<ScanOutcome, ScanError> {
            let results = Arc::clone(&self.state.results);
            let owner = self.user.subject.clone();
            let stored = tokio::task::spawn_blocking(move || {
                let bytes = recto_scanning::encode(&pages, format)?;
                results.put(&owner, format, pages.len(), &bytes)
            })
            .await
            .map_err(|_| ScanError::Encoding("the encoding task stopped".into()))??;
            Ok(ScanOutcome {
                id: stored.id,
                file_name: stored.file_name,
                pages: stored.pages,
                can_save: self.state.output_dir.is_some(),
            })
        }
    }

    /// The settings to scan with: refused when the scanner does not offer them, and with the
    /// area cut from the selection. The scanner's full area is always set explicitly, so a
    /// window left over from a previous selection is never reused.
    pub fn checked(
        settings: ScanSettings,
        selection: Option<Selection>,
        capabilities: &ScannerCapabilities,
    ) -> Result<ScanSettings, ScanError> {
        let unsupported = |what: &str| Err(ScanError::Unsupported(what.to_owned()));
        if !capabilities.resolutions.is_empty()
            && !capabilities.resolutions.contains(&settings.resolution)
        {
            return unsupported(&format!("{} dpi", settings.resolution));
        }
        if !capabilities.modes.is_empty() && !capabilities.modes.contains(&settings.mode) {
            return unsupported(settings.mode.sane_name());
        }
        if let Some(source) = &settings.source
            && !capabilities
                .sources
                .iter()
                .any(|known| known.name == *source)
        {
            return unsupported(source);
        }
        let area = match (selection, capabilities.area) {
            (Some(selection), Some(full)) => selection.to_area(&full).or(Some(full)),
            (_, full) => full,
        };
        Ok(ScanSettings { area, ..settings })
    }

    #[cfg(test)]
    mod tests {
        use recto_scanning::{Area, ScanMode, Source};

        use super::*;

        fn capabilities() -> ScannerCapabilities {
            ScannerCapabilities {
                resolutions: vec![75, 300],
                modes: vec![ScanMode::Color],
                sources: vec![Source::new("Flatbed")],
                area: Some(Area {
                    left: 0.0,
                    top: 0.0,
                    right: 200.0,
                    bottom: 300.0,
                }),
            }
        }

        fn settings() -> ScanSettings {
            ScanSettings {
                resolution: 300,
                mode: ScanMode::Color,
                source: Some("Flatbed".into()),
                // Whatever the browser sends, the area comes from the selection.
                area: Some(Area {
                    left: 1.0,
                    top: 1.0,
                    right: 2.0,
                    bottom: 2.0,
                }),
            }
        }

        #[test]
        fn the_selection_decides_the_area() {
            let full = checked(settings(), None, &capabilities()).unwrap();
            assert_eq!(full.area, capabilities().area);
            let half = Selection {
                x: 0.0,
                y: 0.5,
                width: 1.0,
                height: 0.5,
            };
            let area = checked(settings(), Some(half), &capabilities())
                .unwrap()
                .area
                .unwrap();
            assert_eq!((area.top, area.bottom), (150.0, 300.0));
            let without_geometry = ScannerCapabilities {
                area: None,
                ..capabilities()
            };
            let scanned = checked(settings(), Some(half), &without_geometry).unwrap();
            assert_eq!(scanned.area, None);
        }

        #[test]
        fn settings_the_scanner_lacks_are_refused() {
            let refused = |settings| checked(settings, None, &capabilities()).unwrap_err();
            assert_eq!(
                refused(ScanSettings {
                    resolution: 600,
                    ..settings()
                }),
                ScanError::Unsupported("600 dpi".into())
            );
            assert_eq!(
                refused(ScanSettings {
                    mode: ScanMode::Gray,
                    ..settings()
                }),
                ScanError::Unsupported("Gray".into())
            );
            assert_eq!(
                refused(ScanSettings {
                    source: Some("ADF".into()),
                    ..settings()
                }),
                ScanError::Unsupported("ADF".into())
            );
        }
    }
}
