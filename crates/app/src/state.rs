//! What the server hands to pages and server functions: the services, as axum state and as a
//! Leptos context, and the signed-in user of the request.

use std::{path::PathBuf, sync::Arc};

use axum::{extract::FromRef, http::request::Parts};
use leptos::{
    config::LeptosOptions,
    prelude::{ServerFnError, use_context},
};
use recto_domain::held::files::HeldFiles;
use recto_printing::{PrintService, PrinterAdmin, count::PageCounter, raster::RasterConverter};
use recto_scanning::{ResultStore, ScannerService};
use sea_orm::DatabaseConnection;

use crate::branding::Branding;
pub use crate::currency::Currency;

/// Whose jobs people see in the job list and may cancel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum JobVisibility {
    /// Everyone sees every job (the household trust model).
    #[default]
    All,
    /// People see the jobs they submitted through recto; administrators see every job.
    Own,
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
    /// Turns the rasters computers and phones send into PDFs, within bounds.
    pub raster: RasterConverter,
    /// Whether computers and phones print and scan through recto with device passwords
    /// (`RECTO_OS_INTEGRATION`); without it, the Account page has no Devices section.
    pub os_integration: bool,
    /// The documents of held jobs (`RECTO_DATA_DIR/held`). Its I/O blocks: call it through
    /// [`HeldFiles::blocking`].
    pub held: Arc<HeldFiles>,
    /// How long a held job waits for its owner (`RECTO_HELD_JOB_HOURS`).
    pub held_lifetime: chrono::TimeDelta,
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
