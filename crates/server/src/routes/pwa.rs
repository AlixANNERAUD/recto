//! Routes that belong to the browser rather than to a page: the theme switch. They are open to
//! anyone, so the sign-in page can offer the switch too; the cross-site check still applies.

use axum::{
    Form, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::post,
};
use recto_app::{
    i18n, messages,
    theme::{self, Theme},
};
use serde::Deserialize;

use crate::session::local_path;

/// A year, in seconds: the choice is remembered until the person changes it.
const THEME_MAX_AGE: u32 = 31_536_000;

#[derive(Clone)]
struct PwaState {
    /// Whether cookies get the `Secure` attribute, as the session cookie does.
    secure_cookies: bool,
}

/// `POST /theme`. `secure_cookies` is whether recto is served over HTTPS.
pub fn router(secure_cookies: bool) -> Router {
    Router::new()
        .route("/theme", post(choose_theme))
        .with_state(PwaState { secure_cookies })
}

#[derive(Deserialize)]
struct ThemeForm {
    theme: String,
    next: Option<String>,
}

/// Stores the chosen theme in a cookie, then goes back to the page. The server renders the
/// theme from the cookie, so the form needs no JavaScript. System removes the cookie.
async fn choose_theme(
    State(state): State<PwaState>,
    headers: HeaderMap,
    Form(form): Form<ThemeForm>,
) -> Response {
    let Some(chosen) = Theme::parse(&form.theme) else {
        let message = messages::unknown_theme(i18n::from_headers(&headers));
        return (StatusCode::BAD_REQUEST, message).into_response();
    };
    // Readable by the server only: the page never needs it in the browser.
    let flags = if state.secure_cookies {
        "; Path=/; SameSite=Lax; HttpOnly; Secure"
    } else {
        "; Path=/; SameSite=Lax; HttpOnly"
    };
    let cookie = match chosen {
        Theme::System => format!("{}=; Max-Age=0{flags}", theme::COOKIE),
        _ => format!(
            "{}={}; Max-Age={THEME_MAX_AGE}{flags}",
            theme::COOKIE,
            chosen.as_str()
        ),
    };
    let mut response = Redirect::to(local_path(form.next.as_deref())).into_response();
    if let Ok(value) = cookie.parse() {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}
