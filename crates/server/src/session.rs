//! Sessions and the signed-in user.
//!
//! Sessions live in memory behind the `recto_session` cookie: `HttpOnly`, `SameSite=Lax`,
//! `Secure` when the public URL is HTTPS, and forgotten after 12 hours without a request. The
//! user is read from the session once per request and left in the request's extensions, where
//! handlers, pages and server functions find it ([`recto_app::state::current_user`]).

use axum::{
    extract::Request,
    http::{Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use recto_app::{i18n, messages};
use recto_authentication::User;
use tower_sessions::{
    Expiry, MemoryStore, Session, SessionManagerLayer,
    cookie::{SameSite, time},
};

/// The signed-in [`User`].
const USER: &str = "user";
/// The ID token of the sign-in, a hint for the provider at sign-out.
const ID_TOKEN: &str = "id_token";

pub fn layer(secure: bool) -> SessionManagerLayer<MemoryStore> {
    SessionManagerLayer::new(MemoryStore::default())
        .with_name("recto_session")
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_secure(secure)
        .with_expiry(Expiry::OnInactivity(time::Duration::hours(12)))
}

/// Starts a signed-in session. The session gets a new id, so an id planted in the browser
/// before sign-in (session fixation) is worthless after it.
pub async fn sign_in(
    session: &Session,
    user: &User,
    id_token: &str,
) -> Result<(), tower_sessions::session::Error> {
    session.cycle_id().await?;
    session.insert(USER, user).await?;
    session.insert(ID_TOKEN, id_token).await
}

/// Ends the session and returns its ID token, if any.
pub async fn sign_out(session: &Session) -> Option<String> {
    let id_token = session.get::<String>(ID_TOKEN).await.ok().flatten();
    if let Err(error) = session.flush().await {
        tracing::warn!(%error, "ending the session failed");
    }
    id_token
}

/// Middleware putting the session's [`User`] in the request's extensions. A session that cannot
/// be read counts as signed out.
pub async fn load_user(session: Session, mut request: Request, next: Next) -> Response {
    match session.get::<User>(USER).await {
        Ok(Some(user)) => {
            request.extensions_mut().insert(user);
        }
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "reading the session failed"),
    }
    next.run(request).await
}

/// Paths that render for anyone: the pages explaining a failed or ended sign-in.
fn is_public(path: &str) -> bool {
    path == "/no-access" || path.starts_with("/auth/")
}

/// Middleware letting only signed-in users through. A browser navigating to a page is sent to
/// sign in and brought back; anything else (forms, downloads, server functions, images) gets
/// 401, which a script can handle and an `<img>` simply fails on.
pub async fn require_user(request: Request, next: Next) -> Response {
    if request.extensions().get::<User>().is_some() || is_public(request.uri().path()) {
        return next.run(request).await;
    }
    if is_page_request(&request) {
        let target = request
            .uri()
            .path_and_query()
            .map_or("/", |path| path.as_str());
        let next: String = url::form_urlencoded::byte_serialize(target.as_bytes()).collect();
        Redirect::to(&format!("/auth/login?next={next}")).into_response()
    } else {
        let locale = i18n::from_headers(request.headers());
        (StatusCode::UNAUTHORIZED, messages::sign_in_required(locale)).into_response()
    }
}

/// Browsers send `Accept: text/html` when navigating, and not for images or scripts.
fn is_page_request(request: &Request) -> bool {
    matches!(*request.method(), Method::GET | Method::HEAD)
        && !request.uri().path().starts_with("/api/")
        && request
            .headers()
            .get_all(header::ACCEPT)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|value| value.contains("text/html"))
}

/// `next` when it is a path on this site, else `/`. `//host` and `/\host` are read by browsers
/// as another site, so they are refused like full URLs.
pub fn local_path(next: Option<&str>) -> &str {
    next.filter(|path| {
        path.starts_with('/')
            && !path.starts_with("//")
            && !path.starts_with("/\\")
            && !path.chars().any(char::is_control)
    })
    .unwrap_or("/")
}

#[cfg(test)]
mod tests {
    use super::local_path;

    #[test]
    fn only_local_paths_are_kept() {
        assert_eq!(local_path(Some("/jobs?submitted=3")), "/jobs?submitted=3");
        for foreign in [
            "https://evil.example",
            "//evil.example",
            "/\\evil.example",
            "jobs",
            "/\njobs",
            "",
        ] {
            assert_eq!(local_path(Some(foreign)), "/", "{foreign:?}");
        }
        assert_eq!(local_path(None), "/");
    }
}
