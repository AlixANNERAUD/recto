//! Server functions: the data of the pages and the scan actions of the scan island. Each is
//! served at `/api/<endpoint>` behind the server's sign-in gate and cross-site check, and starts
//! with [`require_user`](crate::state::require_user) as well.
//!
//! Errors carry a translated message made on the server ([`messages::scan_error`],
//! `print.unavailable_banner`, `common.try_again`), never the error's internals; the details go
//! to the log.
//!
//! [`messages::scan_error`]: crate::messages::scan_error

use leptos::{prelude::*, server_fn::codec::Json};
use recto_printing::{Job, Printer};
#[cfg(feature = "ssr")]
use recto_scanning::ScanRequest;
use recto_scanning::{OutputFormat, ScanSettings, ScannerCapabilities, ScannerInfo, Selection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{currency::Currency, quotas::Allowance};

/// A finished preview or scan, stored for the user until downloaded, saved or expired.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanOutcome {
    pub id: Uuid,
    pub file_name: String,
    pub pages: usize,
    /// Whether an output folder is configured, so Save can work.
    pub can_save: bool,
}

/// Where the signed-in user stands against the quota rules that apply to them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allowances {
    /// One per enabled rule that applies on a printer the user may use, by rule id; empty
    /// without rules.
    pub allowances: Vec<Allowance>,
    pub currency: Currency,
}

#[server(endpoint = "list_printers")]
pub async fn list_printers() -> Result<Vec<Printer>, ServerFnError> {
    let context = server::Context::new()?;
    let printers = context.state.print.printers().await;
    let printers = printers.map_err(|error| context.print_failure(&error))?;
    context.usable_printers(printers).await
}

#[server(endpoint = "list_jobs")]
pub async fn list_jobs() -> Result<Vec<Job>, ServerFnError> {
    let context = server::Context::new()?;
    let jobs = context.state.print.jobs().await;
    let jobs = jobs.map_err(|error| context.print_failure(&error))?;
    context.name_owners_and_filter(jobs).await
}

#[server(endpoint = "my_quotas")]
pub async fn my_quotas() -> Result<Allowances, ServerFnError> {
    let context = server::Context::new()?;
    context.my_quotas().await
}

#[server(endpoint = "list_scanners")]
pub async fn list_scanners(refresh: bool) -> Result<Vec<ScannerInfo>, ServerFnError> {
    let context = server::Context::new()?;
    let scanners = context.state.scan.scanners(refresh).await;
    let scanners = scanners.map_err(|error| context.scan_failure(&error))?;
    context.usable_scanners(scanners).await
}

// Named so its request type does not clash with the `ScannerCapabilities` model.
#[server(name = ScannerCapabilitiesRequest, endpoint = "scanner_capabilities")]
pub async fn scanner_capabilities(id: String) -> Result<ScannerCapabilities, ServerFnError> {
    let context = server::Context::new()?;
    context.listed(&id).await?;
    let capabilities = context.state.scan.capabilities(&id).await;
    capabilities.map_err(|error| context.scan_failure(&error))
}

/// A quick low-resolution scan of the whole glass from `source` (the device's current one when
/// absent), stored as PNG for the preview image. It reads one page only, even from a feeder.
#[server(endpoint = "preview")]
pub async fn preview(id: String, source: Option<String>) -> Result<ScanOutcome, ServerFnError> {
    let context = server::Context::new()?;
    context.listed(&id).await?;
    let scanned = async {
        let capabilities = context.state.scan.capabilities(&id).await?;
        let settings = recto_scanning::preview_settings(&capabilities, source);
        let settings = server::checked(settings, None, &capabilities)?;
        let request = ScanRequest {
            settings,
            format: OutputFormat::Png,
            single_page: true,
        };
        let pages = context.state.scan.scan(&id, request).await?;
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
    context.listed(&id).await?;
    let scanned = async {
        let capabilities = context.state.scan.capabilities(&id).await?;
        let settings = server::checked(settings, selection, &capabilities)?;
        let request = ScanRequest {
            settings,
            format,
            single_page: false,
        };
        let pages = context.state.scan.scan(&id, request).await?;
        context.store(pages, format).await
    };
    let outcome = scanned
        .await
        .map_err(|error| context.scan_failure(&error))?;
    context.record_scan(&id, format, &outcome).await;
    Ok(outcome)
}

/// Whether `user` sees a job and may cancel it. `record` is the job's record, when recto
/// submitted it. Administrators and the `All` setting see everything; otherwise only the
/// submitter does, so jobs recto did not submit (no record) are for administrators only.
///
/// Without a creation time from CUPS, [`recto_domain::jobs::matching`] takes the newest record
/// with the job's id. That is acceptable here: CUPS always sends `time-at-creation` in practice.
#[cfg(feature = "ssr")]
pub fn visible(
    record: Option<&recto_domain::jobs::RecordedJob>,
    user: &crate::state::User,
    visibility: crate::state::JobVisibility,
) -> bool {
    user.is_administrator
        || visibility == crate::state::JobVisibility::All
        || record.is_some_and(|record| record.user_id == user.id)
}

#[cfg(feature = "ssr")]
mod server {
    use std::{collections::BTreeMap, sync::Arc};

    use embedded_i18n::Locale;
    use leptos::prelude::ServerFnError;
    use recto_domain::{
        access::{self, DeviceKind, Usable},
        jobs, quotas,
        scans::{self, NewScan, Source},
    };
    use recto_printing::{Job, PrintError, Printer};
    use recto_scanning::{
        OutputFormat, ScanError, ScanSettings, ScannedPage, ScannerCapabilities, ScannerInfo,
        Selection,
    };

    use super::{Allowances, ScanOutcome, visible};
    use crate::{
        i18n, messages,
        quotas::Allowance,
        state::{AppState, JobVisibility, User, require_user},
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

        /// Names the jobs' owners from the records of the jobs recto submitted and drops the
        /// jobs the user may not see. A database failure is logged and leaves CUPS's own owners,
        /// so the list still shows; but when ownership decides what the user sees, no record
        /// means no jobs: it fails closed.
        pub async fn name_owners_and_filter(
            &self,
            mut jobs: Vec<Job>,
        ) -> Result<Vec<Job>, ServerFnError> {
            let ids: Vec<i32> = jobs.iter().map(|job| job.id).collect();
            let database = &self.state.database;
            let visibility = self.state.job_visibility;
            let records = match jobs::for_cups_ids(database, &ids).await {
                Ok(records) => records,
                Err(error) => {
                    tracing::error!(%error, "reading the job records failed");
                    if self.user.is_administrator || visibility == JobVisibility::All {
                        return Ok(jobs);
                    }
                    return Err(self.try_again());
                }
            };
            jobs.retain_mut(|job| {
                let record = jobs::matching(&records, job.id, job.created);
                if let Some(record) = record {
                    job.owner = Some(record.display_name.clone());
                    // What a job counted and cost is its owner's business, and the
                    // administrators'.
                    if self.user.is_administrator || record.user_id == self.user.id {
                        job.counted_pages = record.pages;
                        job.cost = Some(record.cost).filter(|cost| *cost > 0);
                    }
                }
                visible(record, &self.user, visibility)
            });
            Ok(jobs)
        }

        /// The rules that apply to the user on any printer they may use, each once, with where
        /// the user stands in its current window.
        pub async fn my_quotas(&self) -> Result<Allowances, ServerFnError> {
            let currency = self.state.currency.clone();
            let database = &self.state.database;
            let failed = |error: recto_domain::Error| {
                tracing::error!(%error, "reading the print quotas failed");
                self.try_again()
            };
            let rules = quotas::rules(database).await.map_err(failed)?;
            if !rules.iter().any(|rule| rule.enabled) {
                return Ok(Allowances {
                    allowances: Vec::new(),
                    currency,
                });
            }
            let groups = access::group_ids_of(database, self.user.id)
                .await
                .map_err(failed)?;
            let printers = self.state.print.printers().await;
            let printers = printers.map_err(|error| self.print_failure(&error))?;
            let printers = self.usable_printers(printers).await?;
            let mut applicable = BTreeMap::new();
            for printer in &printers {
                for rule in quotas::applicable(&rules, self.user.id, &groups, &printer.name) {
                    applicable.insert(rule.id, rule);
                }
            }
            let now = chrono::Utc::now();
            let mut allowances = Vec::new();
            for rule in applicable.values() {
                let standing =
                    quotas::standing(database, rule, self.user.id, now, self.state.timezone)
                        .await
                        .map_err(failed)?;
                allowances.push(Allowance::from(&standing));
            }
            Ok(Allowances {
                allowances,
                currency,
            })
        }

        /// What the user is told when the database cannot say what they may see or use: nothing
        /// about the cause helps them, and the details are in the log.
        fn try_again(&self) -> ServerFnError {
            ServerFnError::new(embedded_i18n::t!("common.try_again", locale = self.locale))
        }

        /// Drops the printers the user may not use. The rules not being readable is a failure,
        /// not an open door: a restricted printer must never show because its rule was lost.
        pub async fn usable_printers(
            &self,
            printers: Vec<Printer>,
        ) -> Result<Vec<Printer>, ServerFnError> {
            let usable = self.usable(DeviceKind::Printer).await.map_err(|error| {
                tracing::error!(%error, "reading the printer access rules failed");
                self.try_again()
            })?;
            Ok(printers
                .into_iter()
                .filter(|printer| usable.allows(&printer.name))
                .collect())
        }

        /// Drops the scanners the user may not use; fails closed like
        /// [`usable_printers`](Self::usable_printers).
        pub async fn usable_scanners(
            &self,
            scanners: Vec<ScannerInfo>,
        ) -> Result<Vec<ScannerInfo>, ServerFnError> {
            let usable = self.usable(DeviceKind::Scanner).await.map_err(|error| {
                tracing::error!(%error, "reading the scanner access rules failed");
                self.try_again()
            })?;
            Ok(scanners
                .into_iter()
                .filter(|scanner| usable.allows(&scanner.id))
                .collect())
        }

        async fn usable(&self, kind: DeviceKind) -> recto_domain::Result<Usable> {
            Usable::read(
                &self.state.database,
                kind,
                self.user.id,
                self.user.is_administrator,
            )
            .await
        }

        /// Refuses a scanner the scanner list does not have. Ids come from the browser, and
        /// SANE's `net` backend opens an unknown `net:<host>:<device>` by connecting to that
        /// host: any user could make recto reach arbitrary hosts, and an unreachable one holds
        /// the single SANE thread, so every other scan, until the connection times out. The
        /// cached list is enough: Refresh in the interface updates it.
        /// A scanner the user may not use is as unknown as one that does not exist.
        pub async fn listed(&self, id: &str) -> Result<(), ServerFnError> {
            let scanners = self.state.scan.scanners(false).await;
            let scanners = scanners.map_err(|error| self.scan_failure(&error))?;
            let scanners = self.usable_scanners(scanners).await?;
            if scanners.iter().any(|scanner| scanner.id == id) {
                Ok(())
            } else {
                Err(self.scan_failure(&ScanError::NotFound(id.to_owned())))
            }
        }

        pub fn scan_failure(&self, error: &ScanError) -> ServerFnError {
            tracing::warn!(%error, user = self.user.username, "scanning failed");
            ServerFnError::new(messages::scan_error(
                error,
                &self.state.branding.name,
                self.locale,
            ))
        }

        /// Logs a finished scan for the usage reports. Best effort: the person still gets the
        /// scan when the log cannot be written, since scans have no quotas to enforce.
        pub async fn record_scan(
            &self,
            scanner: &str,
            format: OutputFormat,
            outcome: &ScanOutcome,
        ) {
            let scan = NewScan {
                user_id: Some(self.user.id),
                username: self.user.username.clone(),
                scanner: scanner.to_owned(),
                result_id: Some(outcome.id.to_string()),
                source: Source::Web,
                format: format.extension().to_owned(),
                pages: u32::try_from(outcome.pages).unwrap_or(u32::MAX),
            };
            let now = chrono::Utc::now();
            if let Err(error) = scans::record(&self.state.database, scan, now).await {
                tracing::error!(%error, user = self.user.username, "recording the scan failed");
            }
        }

        /// Assembles the pages into the file and keeps it for the user. Both block, so they run
        /// off the async workers.
        pub async fn store(
            &self,
            pages: Vec<ScannedPage>,
            format: OutputFormat,
        ) -> Result<ScanOutcome, ScanError> {
            let results = Arc::clone(&self.state.results);
            let owner = self.user.id.to_string();
            let stored = tokio::task::spawn_blocking(move || {
                let count = pages.len();
                let bytes = recto_scanning::assemble(pages, format)?;
                results.put(&owner, format, count, &bytes)
            })
            .await
            .map_err(|_| ScanError::Interrupted)??;
            Ok(ScanOutcome {
                id: stored.id,
                file_name: stored.file_name,
                pages: stored.pages,
                can_save: self.state.output_dir.is_some(),
            })
        }
    }

    /// The settings to scan with: refused when the scanner does not offer them or when a page
    /// would exceed the memory budget, and with the area cut from the selection. The scanner's
    /// full area is always set explicitly, so a window left over from a previous selection is
    /// never reused.
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
        let settings = ScanSettings { area, ..settings };
        if let Some(bytes) = settings.page_bytes() {
            recto_scanning::check_page_bytes(bytes)?;
        }
        Ok(settings)
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
        fn pages_over_the_memory_budget_are_refused() {
            let large = ScannerCapabilities {
                resolutions: vec![300, 1200],
                ..capabilities()
            };
            // 200 x 300 mm in colour at 1200 dpi is about 390 MB raw.
            let refused = checked(
                ScanSettings {
                    resolution: 1200,
                    ..settings()
                },
                None,
                &large,
            );
            assert_eq!(refused, Err(ScanError::TooLarge));
            // A smaller area at the same resolution fits.
            let quarter = Selection {
                x: 0.0,
                y: 0.0,
                width: 0.5,
                height: 0.5,
            };
            let fits = checked(
                ScanSettings {
                    resolution: 1200,
                    ..settings()
                },
                Some(quarter),
                &large,
            );
            assert!(fits.is_ok(), "{fits:?}");
            assert!(checked(settings(), None, &large).is_ok());
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
