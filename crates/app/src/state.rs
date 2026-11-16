//! What the server hands to pages and server functions: the services, as axum state and as a
//! Leptos context, and the signed-in user of the request.

use std::{path::PathBuf, sync::Arc};

use axum::{extract::FromRef, http::request::Parts};
use leptos::{
    config::LeptosOptions,
    prelude::{ServerFnError, use_context},
};
use recto_printing::{PrintService, PrinterAdmin};
use recto_scanning::{ResultStore, ScannerService};
use sea_orm::DatabaseConnection;

use crate::branding::Branding;

/// Whose jobs people see in the job list and may cancel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum JobVisibility {
    /// Everyone sees every job (the household trust model).
    #[default]
    All,
    /// People see the jobs they submitted through recto; administrators see every job.
    Own,
}

/// The currency prices and costs are shown in. Amounts are stored in minor units (cents); this
/// only decides how they read, and never converts anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Currency {
    /// The ISO 4217 code, in capitals.
    pub code: String,
    /// Digits after the decimal point: the currency's minor unit exponent.
    pub decimals: u32,
}

/// Currencies without a minor unit (ISO 4217 exponent 0); every other one has two decimals.
const ZERO_DECIMAL: [&str; 16] = [
    "BIF", "CLP", "DJF", "GNF", "ISK", "JPY", "KMF", "KRW", "PYG", "RWF", "UGX", "VND", "VUV",
    "XAF", "XOF", "XPF",
];

impl Currency {
    /// The currency of an ISO code, in any case.
    pub fn new(code: &str) -> Self {
        let code = code.to_ascii_uppercase();
        let decimals = if ZERO_DECIMAL.contains(&code.as_str()) {
            0
        } else {
            2
        };
        Self { code, decimals }
    }

    /// An amount in minor units as `12.34 EUR`.
    pub fn format(&self, minor: i64) -> String {
        let sign = if minor < 0 { "-" } else { "" };
        let minor = minor.unsigned_abs();
        if self.decimals == 0 {
            return format!("{sign}{minor} {}", self.code);
        }
        let unit = 10u64.pow(self.decimals);
        format!(
            "{sign}{}.{:0width$} {}",
            minor / unit,
            minor % unit,
            self.code,
            width = self.decimals as usize
        )
    }
}

/// The services behind the interface. Trait objects, so tests swap in mocks.
#[derive(Clone)]
pub struct AppState {
    pub print: Arc<dyn PrintService>,
    /// Adds and changes the printers of CUPS; `None` turns printer administration off.
    pub printer_admin: Option<Arc<dyn PrinterAdmin>>,
    pub scan: Arc<dyn ScannerService>,
    /// Finished scans waiting to be downloaded or saved. Its I/O blocks: call it through
    /// `tokio::task::spawn_blocking`. Results belong to the [`User::id`], as text.
    pub results: Arc<ResultStore>,
    /// Where "Save" writes scans; `None` hides the button.
    pub output_dir: Option<PathBuf>,
    /// Largest accepted upload, in bytes.
    pub max_upload_size: usize,
    pub leptos_options: LeptosOptions,
    /// The accounts database, which the server opens and migrates (crate `recto-domain`).
    pub database: DatabaseConnection,
    /// The name, colors, logo and icons the operator chose.
    pub branding: Arc<Branding>,
    /// Whose jobs a user who is not an administrator sees and may cancel
    /// (`RECTO_JOB_VISIBILITY`).
    pub job_visibility: JobVisibility,
    /// How costs read (`RECTO_CURRENCY`).
    pub currency: Currency,
    /// The zone calendar periods of quotas follow (`RECTO_TIMEZONE`).
    pub timezone: chrono_tz::Tz,
}

/// The signed-in user of a request, read from the accounts database on every request, so that
/// disabling an account or changing its administration applies at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    /// Never changes; the CUPS `requesting-user-name` of the user's jobs.
    pub username: String,
    /// The name shown in the interface; the username when the user has none.
    pub name: String,
    pub is_administrator: bool,
    /// Whether the user has a password, so can sign in with a local account.
    pub has_password: bool,
}

impl FromRef<AppState> for LeptosOptions {
    fn from_ref(state: &AppState) -> Self {
        state.leptos_options.clone()
    }
}

/// The [`AppState`] of the current request, outside a request `None`.
pub fn app_state() -> Option<AppState> {
    use_context::<AppState>()
}

/// The user signed in on the current request. The server reads it from the session before any
/// page or server function runs and leaves it in the request's extensions, so this needs no
/// session access of its own.
pub fn current_user() -> Option<User> {
    use_context::<Parts>()?.extensions.get::<User>().cloned()
}

/// The signed-in user, for every page and server function that reads [`AppState`] or acts for
/// someone: call it first. Without a user, the response becomes a redirect to sign-in and the
/// error says sign-in is required. The server already turns anonymous visitors away before any
/// page renders; this keeps a page safe should a route ever be left open by mistake.
pub fn require_user() -> Result<User, ServerFnError> {
    if let Some(user) = current_user() {
        return Ok(user);
    }
    let next = use_context::<Parts>()
        .and_then(|parts| {
            parts
                .uri
                .path_and_query()
                .map(|path| path.as_str().to_owned())
        })
        .unwrap_or_else(|| "/".to_owned());
    leptos_axum::redirect(&format!(
        "/sign-in?next={}",
        crate::pages::auth::encode_component(&next)
    ));
    Err(ServerFnError::new(crate::messages::sign_in_required(
        crate::i18n::locale(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_read_in_the_currency_minor_unit() {
        let euro = Currency::new("eur");
        assert_eq!(euro.format(1234), "12.34 EUR");
        assert_eq!(euro.format(5), "0.05 EUR");
        assert_eq!(euro.format(-250), "-2.50 EUR");
        assert_eq!(Currency::new("JPY").format(1234), "1234 JPY");
    }
}
