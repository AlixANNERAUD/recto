//! Sessions and the signed-in user.
//!
//! Sessions live in the accounts database ([`DatabaseSessionStore`]) behind the `recto_session`
//! cookie: `HttpOnly`, `SameSite=Lax`, `Secure` when the public URL is HTTPS. A signed-in
//! session is forgotten after 12 hours without a request; a session that only holds a sign-in
//! in progress, after 10 minutes.
//!
//! A session holds the user's id and its sign-in time, not the user: the user is read from the
//! database on every request ([`load_user`]) and left in the request's extensions, where
//! handlers, pages and server functions find it ([`recto_app::state::current_user`]). Disabling
//! a user or changing their password thus ends their sessions at their next request.

use axum::{
    extract::{Request, State},
    http::{Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use chrono::{DateTime, FixedOffset, Utc};
use recto_app::{i18n, messages};
use recto_authentication::User;
use recto_domain::users;
use recto_entity::users as user_rows;
use sea_orm::DatabaseConnection;
use tower_sessions::{
    Expiry, Session, SessionManagerLayer,
    cookie::{SameSite, time::Duration},
};

use crate::session_store::DatabaseSessionStore;

/// The id of the signed-in user.
const USER_ID: &str = "user_id";
/// When the user signed in, in microseconds since the Unix epoch. Microseconds, because a
/// session is valid only when it was signed in at or after the user's `sessions_valid_after`,
/// which ends sessions to the microsecond (see [`recto_domain::users::set_password`]).
const SIGNED_IN_AT: &str = "signed_in_at";
/// The ID token of the sign-in, a hint for the provider at sign-out; empty without one.
const ID_TOKEN: &str = "id_token";

/// How long a signed-in session survives without a request.
const SIGNED_IN: Duration = Duration::hours(12);
/// How long a session survives with only a sign-in in progress: the provider's round trip.
const SIGNING_IN: Duration = Duration::minutes(10);

/// The session layer. Sessions are saved on every request (`always_save`), so the inactivity
/// expiry moves with each request even when nothing in the session changes; a request without
/// a session cookie and with nothing to store still creates nothing.
pub fn layer(
    store: DatabaseSessionStore,
    secure: bool,
) -> SessionManagerLayer<DatabaseSessionStore> {
    SessionManagerLayer::new(store)
        .with_name("recto_session")
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_secure(secure)
        .with_expiry(Expiry::OnInactivity(SIGNED_IN))
        .with_always_save(true)
}

/// Gives the session the short life of a sign-in in progress.
pub fn signing_in(session: &Session) {
    session.set_expiry(Some(Expiry::OnInactivity(SIGNING_IN)));
}

/// Starts a signed-in session for the user `user_id`. The session gets a new id, so an id
/// planted in the browser before sign-in (session fixation) is worthless after it.
pub async fn sign_in(
    session: &Session,
    user_id: i64,
    id_token: &str,
) -> Result<(), tower_sessions::session::Error> {
    start(session, user_id, id_token, Utc::now().timestamp_micros()).await
}

/// Signs the session in again after the user's sessions were ended at `cutoff` (their
/// `sessions_valid_after`), as a password change does: the other sessions stay ended, this one
/// carries on with a new id. Its sign-in time is never before the cutoff, so it survives even on
/// a clock that has not moved since. The ID token is kept for the provider's sign-out.
pub async fn sign_in_again(
    session: &Session,
    user_id: i64,
    cutoff: Option<DateTime<FixedOffset>>,
) -> Result<(), tower_sessions::session::Error> {
    let id_token = session.get::<String>(ID_TOKEN).await?.unwrap_or_default();
    let now = Utc::now().timestamp_micros();
    let signed_in_at = cutoff.map_or(now, |cutoff| now.max(cutoff.timestamp_micros()));
    start(session, user_id, &id_token, signed_in_at).await
}

async fn start(
    session: &Session,
    user_id: i64,
    id_token: &str,
    signed_in_at: i64,
) -> Result<(), tower_sessions::session::Error> {
    session.cycle_id().await?;
    session.set_expiry(Some(Expiry::OnInactivity(SIGNED_IN)));
    session.insert(USER_ID, user_id).await?;
    session.insert(SIGNED_IN_AT, signed_in_at).await?;
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

/// The [`User`] of a users row.
pub fn user(row: &user_rows::Model) -> User {
    User {
        id: row.id,
        username: row.username.clone(),
        name: row.display_name.clone(),
        is_administrator: row.is_administrator,
        is_local: row.password_hash.is_some(),
    }
}

/// Whether a session signed in at `signed_in_at` (microseconds since the Unix epoch) still
/// counts for the user: the user exists, is not disabled, and the session was signed in at or
/// after the user's `sessions_valid_after`.
fn still_valid(row: &user_rows::Model, signed_in_at: i64) -> bool {
    !row.is_disabled
        && row
            .sessions_valid_after
            .is_none_or(|cutoff| signed_in_at >= cutoff.timestamp_micros())
}

/// Middleware putting the session's [`User`] in the request's extensions, read from the
/// database. A session whose user is gone, disabled, or signed in before their sessions were
/// ended is deleted, and the request is anonymous. A session without a user holds at most a
/// sign-in in progress, so its life stays short even though every request saves it.
pub async fn load_user(
    State(database): State<DatabaseConnection>,
    session: Session,
    mut request: Request,
    next: Next,
) -> Response {
    let signed_in = async {
        let user_id = session.get::<i64>(USER_ID).await?;
        let signed_in_at = session.get::<i64>(SIGNED_IN_AT).await?;
        Ok::<_, tower_sessions::session::Error>(user_id.zip(signed_in_at))
    };
    match signed_in.await {
        Ok(Some((user_id, signed_in_at))) => match users::find_by_id(&database, user_id).await {
            Ok(Some(row)) if still_valid(&row, signed_in_at) => {
                request.extensions_mut().insert(user(&row));
            }
            Ok(_) => {
                tracing::info!(
                    user_id,
                    "session ended: the user is gone, disabled or signed out"
                );
                if let Err(error) = session.flush().await {
                    tracing::warn!(%error, "ending the session failed");
                }
            }
            // The session stays: it is valid again once the database answers.
            Err(error) => tracing::error!(%error, "reading the signed-in user failed"),
        },
        Ok(None) => signing_in(&session),
        Err(error) => {
            tracing::warn!(%error, "reading the session failed");
            signing_in(&session);
        }
    }
    next.run(request).await
}

/// Paths that render for anyone: the sign-in page and form, and the pages explaining a failed
/// or ended sign-in.
fn is_public(path: &str) -> bool {
    path == "/sign-in" || path == "/no-access" || path.starts_with("/auth/")
}

/// Middleware letting only signed-in users through, except to the public paths.
pub async fn require_user(request: Request, next: Next) -> Response {
    if request.extensions().get::<User>().is_some() || is_public(request.uri().path()) {
        return next.run(request).await;
    }
    unauthenticated(&request)
}

/// The answer to an anonymous request for something that needs a user. A browser navigating
/// to a page is sent to the sign-in page and brought back; anything else (forms, downloads, server
/// functions, images) gets 401, which a script can handle and an `<img>` simply fails on.
/// Server functions get it as a server function error carrying [`messages::SIGNED_OUT`], since
/// the islands only see the decoded error, not the status.
pub fn unauthenticated(request: &Request) -> Response {
    if request.uri().path().starts_with("/api/") {
        use leptos::server_fn::error::FromServerFnError;
        let error = leptos::prelude::ServerFnError::new(messages::SIGNED_OUT);
        return (StatusCode::UNAUTHORIZED, error.ser()).into_response();
    }
    if is_page_request(request) {
        let target = request
            .uri()
            .path_and_query()
            .map_or("/", |path| path.as_str());
        let next: String = url::form_urlencoded::byte_serialize(target.as_bytes()).collect();
        Redirect::to(&format!("/sign-in?next={next}")).into_response()
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
    use super::*;

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

    fn row(sessions_valid_after: Option<chrono::DateTime<Utc>>) -> user_rows::Model {
        user_rows::Model {
            id: 1,
            username: "alice".into(),
            display_name: "Alice".into(),
            email: None,
            password_hash: None,
            is_administrator: false,
            administrator_from_claim: false,
            is_disabled: false,
            sessions_valid_after: sessions_valid_after.map(|cutoff| cutoff.fixed_offset()),
            created_at: Utc::now().fixed_offset(),
            last_sign_in_at: None,
        }
    }

    #[test]
    fn sessions_signed_in_before_the_cutoff_are_refused() {
        let cutoff = Utc::now();
        let micros = cutoff.timestamp_micros();
        assert!(still_valid(&row(None), micros));
        assert!(still_valid(&row(Some(cutoff)), micros));
        assert!(still_valid(&row(Some(cutoff)), micros + 1));
        assert!(!still_valid(&row(Some(cutoff)), micros - 1));
        let disabled = user_rows::Model {
            is_disabled: true,
            ..row(None)
        };
        assert!(!still_valid(&disabled, micros));
    }

    #[tokio::test]
    async fn signing_in_again_is_never_before_the_cutoff() {
        let store = DatabaseSessionStore::new(recto_domain::testing::database().await);
        let session = Session::new(None, std::sync::Arc::new(store), None);
        sign_in(&session, 1, "token").await.unwrap();
        // A cutoff ahead of the clock, as a clock too coarse to have moved since would give.
        let cutoff = Utc::now() + chrono::TimeDelta::seconds(1);

        sign_in_again(&session, 1, Some(cutoff.fixed_offset()))
            .await
            .unwrap();

        let signed_in_at = session.get::<i64>(SIGNED_IN_AT).await.unwrap().unwrap();
        assert_eq!(signed_in_at, cutoff.timestamp_micros());
        assert!(still_valid(&row(Some(cutoff)), signed_in_at));
        // The ID token stays, for the provider's sign-out.
        assert_eq!(
            session.get::<String>(ID_TOKEN).await.unwrap().as_deref(),
            Some("token")
        );
    }
}
