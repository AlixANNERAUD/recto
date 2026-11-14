//! What the server hands to pages and server functions: the services, as axum state and as a
//! Leptos context, and the signed-in user of the request.

use std::{path::PathBuf, sync::Arc};

use axum::{extract::FromRef, http::request::Parts};
use leptos::{
    config::LeptosOptions,
    prelude::{ServerFnError, use_context},
};
use recto_authentication::User;
use recto_printing::PrintService;
use recto_scanning::{ResultStore, ScannerService};

/// The services behind the interface. Trait objects, so tests swap in mocks.
#[derive(Clone)]
pub struct AppState {
    pub print: Arc<dyn PrintService>,
    pub scan: Arc<dyn ScannerService>,
    /// Finished scans waiting to be downloaded or saved. Its I/O blocks: call it through
    /// `tokio::task::spawn_blocking`. Results belong to [`User::subject`].
    pub results: Arc<ResultStore>,
    /// Where "Save" writes scans; `None` hides the button.
    pub output_dir: Option<PathBuf>,
    /// Largest accepted upload, in bytes.
    pub max_upload_size: usize,
    pub leptos_options: LeptosOptions,
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
        "/auth/login?next={}",
        crate::pages::auth::encode_component(&next)
    ));
    Err(ServerFnError::new(crate::messages::sign_in_required(
        crate::i18n::locale(),
    )))
}
