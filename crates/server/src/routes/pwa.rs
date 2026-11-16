//! Routes that belong to the browser rather than to a page: the theme switch and the brand's
//! icons and logo. They are open to anyone, so the sign-in page can offer the switch and show
//! the brand too; the cross-site check still applies.

use std::sync::Arc;

use axum::{
    Form, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use recto_app::{
    branding::Branding,
    i18n, messages,
    theme::{self, Theme},
};
use serde::Deserialize;

use crate::session::local_path;

/// A year, in seconds: the choice is remembered until the person changes it.
const THEME_MAX_AGE: u32 = 31_536_000;

/// The brand files change only with a restart, which an hour of caching lets browsers see soon
/// enough.
const BRAND_CACHE: &str = "public, max-age=3600";

/// What an SVG logo opened on its own may do: draw itself, nothing else. Pages show it through
/// `<img>`, which never runs scripts; this covers a visit to its address.
const SVG_POLICY: &str = "default-src 'none'; style-src 'unsafe-inline'; sandbox; \
                          frame-ancestors 'none'";

#[derive(Clone)]
struct PwaState {
    /// Whether cookies get the `Secure` attribute, as the session cookie does.
    secure_cookies: bool,
    branding: Arc<Branding>,
}

/// `POST /theme` and `GET /brand/*`. `secure_cookies` is whether recto is served over HTTPS.
pub fn router(branding: Arc<Branding>, secure_cookies: bool) -> Router {
    let icon = |pick: fn(&Branding) -> &Bytes| {
        get(move |State(state): State<PwaState>| async move {
            brand_file(pick(&state.branding).clone(), "image/png")
        })
    };
    Router::new()
        .route("/theme", post(choose_theme))
        .route(
            "/brand/icon-192.png",
            icon(|branding| &branding.icons.any_192),
        )
        .route(
            "/brand/icon-512.png",
            icon(|branding| &branding.icons.any_512),
        )
        .route(
            "/brand/icon-maskable-512.png",
            icon(|branding| &branding.icons.maskable_512),
        )
        .route(
            "/brand/apple-touch-icon.png",
            icon(|branding| &branding.icons.apple_180),
        )
        .route("/brand/logo", get(logo))
        .with_state(PwaState {
            secure_cookies,
            branding,
        })
}

fn brand_file(bytes: Bytes, content_type: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, BRAND_CACHE),
        ],
        bytes,
    )
        .into_response()
}

/// The operator's logo, or 404 without one.
async fn logo(State(state): State<PwaState>) -> Response {
    let Some(logo) = &state.branding.logo else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut response = brand_file(logo.bytes.clone(), logo.content_type);
    // An SVG is a document that can run scripts when opened directly. Set here, the server's
    // default policy leaves it alone.
    if logo.content_type == "image/svg+xml" {
        response.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(SVG_POLICY),
        );
    }
    response
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
