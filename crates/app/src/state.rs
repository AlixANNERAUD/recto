//! What the server hands to pages and server functions: the services, as axum state and as a
//! Leptos context, and the signed-in user of the request.

use std::{path::PathBuf, sync::Arc};

use axum::{extract::FromRef, http::request::Parts};
use leptos::{
    config::LeptosOptions,
    prelude::{ServerFnError, use_context},
};
use recto_printing::{PrintService, PrinterAdmin, count::PageCounter};
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

/// Currencies with a thousandth as the minor unit (ISO 4217 exponent 3).
const THREE_DECIMALS: [&str; 7] = ["BHD", "IQD", "JOD", "KWD", "LYD", "OMR", "TND"];

/// Currencies without a minor unit (ISO 4217 exponent 0); every other one has two decimals.
const ZERO_DECIMAL: [&str; 16] = [
    "BIF", "CLP", "DJF", "GNF", "ISK", "JPY", "KMF", "KRW", "PYG", "RWF", "UGX", "VND", "VUV",
    "XAF", "XOF", "XPF",
];

impl Currency {
    /// The largest amount [`Currency::parse_amount`] accepts, in major units: far above any
    /// price, and far below where minor units overflow.
    pub const MAX_AMOUNT: i64 = 1_000_000;

    /// The currency of an ISO code, in any case.
    pub fn new(code: &str) -> Self {
        let code = code.to_ascii_uppercase();
        let decimals = if ZERO_DECIMAL.contains(&code.as_str()) {
            0
        } else if THREE_DECIMALS.contains(&code.as_str()) {
            3
        } else {
            2
        };
        Self { code, decimals }
    }

    /// An amount in minor units as `12.34 EUR`.
    pub fn format(&self, minor: i64) -> String {
        format!("{} {}", self.amount(minor), self.code)
    }

    /// An amount in minor units as `12.34`, without the code: what a form field shows, and
    /// what [`Currency::parse_amount`] reads back.
    pub fn amount(&self, minor: i64) -> String {
        let sign = if minor < 0 { "-" } else { "" };
        let minor = minor.unsigned_abs();
        if self.decimals == 0 {
            return format!("{sign}{minor}");
        }
        let unit = 10u64.pow(self.decimals);
        format!(
            "{sign}{}.{:0width$}",
            minor / unit,
            minor % unit,
            width = self.decimals as usize
        )
    }

    /// Reads a price typed in the currency, such as `0.05` or `0,05`, into minor units. Refuses
    /// what is not a plain non-negative amount, more decimals than the currency has, and more
    /// than [`Currency::MAX_AMOUNT`] major units.
    pub fn parse_amount(&self, text: &str) -> Option<i64> {
        let text = text.trim();
        let (whole, fraction) = match text.split_once(['.', ',']) {
            Some((whole, fraction)) => (whole, Some(fraction)),
            None => (text, None),
        };
        let digits =
            |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
        if !digits(whole) {
            return None;
        }
        let unit = 10i64.pow(self.decimals);
        let mut minor = match fraction {
            None => 0,
            Some(fraction) => {
                if !digits(fraction) || fraction.len() > self.decimals as usize {
                    return None;
                }
                // `0.1` in a two-decimal currency is 10 minor units.
                let scale = 10i64.pow(self.decimals - fraction.len() as u32);
                fraction.parse::<i64>().ok()? * scale
            }
        };
        // Leading zeros aside, more digits than the limit has is over it, and would not fit.
        let whole = whole.trim_start_matches('0');
        if whole.len() > 7 {
            return None;
        }
        let whole: i64 = if whole.is_empty() {
            0
        } else {
            whole.parse().ok()?
        };
        minor += whole * unit;
        (minor <= Self::MAX_AMOUNT * unit).then_some(minor)
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
    /// Reads the page count of uploaded PDFs, within bounds.
    pub page_counter: PageCounter,
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
        let dinar = Currency::new("KWD");
        assert_eq!(dinar.decimals, 3);
        assert_eq!(dinar.format(12_345), "12.345 KWD");
        assert_eq!(dinar.format(7), "0.007 KWD");
        assert_eq!(euro.amount(5), "0.05");
        assert_eq!(Currency::new("JPY").amount(12), "12");
    }

    #[test]
    fn typed_amounts_read_into_minor_units() {
        let euro = Currency::new("EUR");
        let yen = Currency::new("JPY");
        let dinar = Currency::new("KWD");
        for (currency, text, expected) in [
            (&euro, "0.05", Some(5)),
            (&euro, "0,05", Some(5)),
            (&euro, " 1.5 ", Some(150)),
            (&euro, "2", Some(200)),
            (&euro, "0", Some(0)),
            (&euro, "007.10", Some(710)),
            (&euro, "1000000", Some(100_000_000)),
            (&euro, "1000000.00", Some(100_000_000)),
            (&yen, "12", Some(12)),
            (&dinar, "0.125", Some(125)),
            (&dinar, "0.1", Some(100)),
            (&euro, "1000000.01", None),
            (&euro, "1000001", None),
            (&euro, "99999999999999999999", None),
            (&euro, "-0.05", None),
            (&euro, "+1", None),
            (&euro, "0.005", None),
            (&euro, "", None),
            (&euro, " ", None),
            (&euro, ".5", None),
            (&euro, "5.", None),
            (&euro, "1,000.00", None),
            (&euro, "1.2.3", None),
            (&euro, "1e3", None),
            (&euro, "٣", None),
            (&euro, "5 EUR", None),
            (&yen, "1.5", None),
            (&yen, "1.0", None),
            (&dinar, "0.0001", None),
        ] {
            assert_eq!(
                currency.parse_amount(text),
                expected,
                "{text:?} in {}",
                currency.code
            );
        }
    }
}
