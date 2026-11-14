//! Sessions and the signed-in user.
//!
//! Sessions live in memory ([`Store`]) behind the `recto_session` cookie: `HttpOnly`,
//! `SameSite=Lax`, `Secure` when the public URL is HTTPS. A signed-in session is forgotten after
//! 12 hours without a request; a session that only holds a sign-in in progress, after 10
//! minutes. The user is read from the session once per request and left in the request's
//! extensions, where handlers, pages and server functions find it
//! ([`recto_app::state::current_user`]).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use async_trait::async_trait;
use axum::{
    extract::Request,
    http::{Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use recto_app::{i18n, messages};
use recto_authentication::User;
use tower_sessions::{
    Expiry, Session, SessionManagerLayer, SessionStore,
    cookie::{
        SameSite,
        time::{Duration, OffsetDateTime},
    },
    session::{Id, Record},
    session_store::{self, ExpiredDeletion},
};

/// The signed-in [`User`].
const USER: &str = "user";
/// The ID token of the sign-in, a hint for the provider at sign-out.
const ID_TOKEN: &str = "id_token";

/// How long a signed-in session survives without a request.
const SIGNED_IN: Duration = Duration::hours(12);
/// How long a session survives with only a sign-in in progress: the provider's round trip.
const SIGNING_IN: Duration = Duration::minutes(10);

/// Sessions in memory. Unlike tower-sessions' `MemoryStore`, expired records are deleted
/// ([`ExpiredDeletion`]), so anonymous visitors starting sign-ins cannot grow memory for good.
#[derive(Clone, Default)]
pub struct Store(Arc<Mutex<HashMap<Id, Record>>>);

// Records hold users and ID tokens; logs get the count only.
impl std::fmt::Debug for Store {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Store")
            .field("records", &self.len())
            .finish()
    }
}

impl Store {
    fn records(&self) -> MutexGuard<'_, HashMap<Id, Record>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many records are kept, expired ones included.
    pub fn len(&self) -> usize {
        self.records().len()
    }

    pub fn is_empty(&self) -> bool {
        self.records().is_empty()
    }
}

#[async_trait]
impl SessionStore for Store {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        let mut records = self.records();
        while records.contains_key(&record.id) {
            record.id = Id::default();
        }
        records.insert(record.id, record.clone());
        Ok(())
    }

    /// Updates a record that still exists, and never brings one back. Every request saves its
    /// session, so a request that loaded it before a sign-out (or before sign-in retired its id)
    /// would otherwise write the old user back when it ends. New and cycled ids go through
    /// [`create`](SessionStore::create).
    async fn save(&self, record: &Record) -> session_store::Result<()> {
        if let Some(slot) = self.records().get_mut(&record.id) {
            *slot = record.clone();
        }
        Ok(())
    }

    async fn load(&self, id: &Id) -> session_store::Result<Option<Record>> {
        let now = OffsetDateTime::now_utc();
        Ok(self
            .records()
            .get(id)
            .filter(|record| record.expiry_date > now)
            .cloned())
    }

    async fn delete(&self, id: &Id) -> session_store::Result<()> {
        self.records().remove(id);
        Ok(())
    }
}

#[async_trait]
impl ExpiredDeletion for Store {
    async fn delete_expired(&self) -> session_store::Result<()> {
        let now = OffsetDateTime::now_utc();
        self.records().retain(|_, record| record.expiry_date > now);
        Ok(())
    }
}

/// The session layer. Sessions are saved on every request (`always_save`), so the inactivity
/// expiry moves with each request even when nothing in the session changes; a request without
/// a session cookie and with nothing to store still creates nothing.
pub fn layer(store: Store, secure: bool) -> SessionManagerLayer<Store> {
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

/// Starts a signed-in session. The session gets a new id, so an id planted in the browser
/// before sign-in (session fixation) is worthless after it.
pub async fn sign_in(
    session: &Session,
    user: &User,
    id_token: &str,
) -> Result<(), tower_sessions::session::Error> {
    session.cycle_id().await?;
    session.set_expiry(Some(Expiry::OnInactivity(SIGNED_IN)));
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
/// be read counts as signed out. A session without a user holds at most a sign-in in progress,
/// so its life stays short even though every request saves it.
pub async fn load_user(session: Session, mut request: Request, next: Next) -> Response {
    match session.get::<User>(USER).await {
        Ok(Some(user)) => {
            request.extensions_mut().insert(user);
        }
        Ok(None) => signing_in(&session),
        Err(error) => {
            tracing::warn!(%error, "reading the session failed");
            signing_in(&session);
        }
    }
    next.run(request).await
}

/// Paths that render for anyone: the pages explaining a failed or ended sign-in.
fn is_public(path: &str) -> bool {
    path == "/no-access" || path.starts_with("/auth/")
}

/// Middleware letting only signed-in users through, except to the public paths.
pub async fn require_user(request: Request, next: Next) -> Response {
    if request.extensions().get::<User>().is_some() || is_public(request.uri().path()) {
        return next.run(request).await;
    }
    unauthenticated(&request)
}

/// The answer to an anonymous request for something that needs a user. A browser navigating
/// to a page is sent to sign in and brought back; anything else (forms, downloads, server
/// functions, images) gets 401, which a script can handle and an `<img>` simply fails on.
pub fn unauthenticated(request: &Request) -> Response {
    if is_page_request(request) {
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

    fn record(expiry_date: OffsetDateTime) -> Record {
        Record {
            id: Id::default(),
            data: Default::default(),
            expiry_date,
        }
    }

    #[tokio::test]
    async fn expired_records_are_deleted() {
        let store = Store::default();
        let now = OffsetDateTime::now_utc();
        let mut expired = record(now - Duration::seconds(1));
        let mut active = record(now + Duration::minutes(5));
        store.create(&mut expired).await.unwrap();
        store.create(&mut active).await.unwrap();
        // Expired records are never served, even before they are deleted.
        assert!(store.load(&expired.id).await.unwrap().is_none());
        assert_eq!(store.len(), 2);

        store.delete_expired().await.unwrap();
        assert_eq!(store.len(), 1);
        let kept = store.load(&active.id).await.unwrap().unwrap();
        assert_eq!(kept.id, active.id);
    }

    fn user() -> User {
        User {
            subject: "alice-subject".into(),
            username: "alice".into(),
            name: None,
        }
    }

    fn session(store: &Store, id: Option<Id>) -> Session {
        Session::new(id, Arc::new(store.clone()), None)
    }

    #[tokio::test]
    async fn a_stale_request_cannot_undo_a_sign_out() {
        let store = Store::default();
        let signed_in = session(&store, None);
        sign_in(&signed_in, &user(), "token").await.unwrap();
        signed_in.save().await.unwrap();
        let id = signed_in.id().unwrap();

        // A slow request loads the session, a sign-out ends it, then the slow request ends.
        let slow = session(&store, Some(id));
        assert_eq!(slow.get::<User>(USER).await.unwrap(), Some(user()));
        assert_eq!(
            sign_out(&session(&store, Some(id))).await.as_deref(),
            Some("token")
        );
        slow.save().await.unwrap();

        assert!(store.load(&id).await.unwrap().is_none());
        assert!(store.is_empty());
    }

    #[tokio::test]
    async fn a_stale_request_cannot_revive_an_id_retired_at_sign_in() {
        let store = Store::default();
        let pending = session(&store, None);
        pending.insert("openid.next", "/jobs").await.unwrap();
        pending.save().await.unwrap();
        let old = pending.id().unwrap();

        let slow = session(&store, Some(old));
        assert!(slow.get::<String>("openid.next").await.unwrap().is_some());
        let callback = session(&store, Some(old));
        sign_in(&callback, &user(), "token").await.unwrap();
        callback.save().await.unwrap();
        let new = callback.id().unwrap();
        assert_ne!(new, old);
        slow.save().await.unwrap();

        assert!(store.load(&old).await.unwrap().is_none());
        assert!(store.load(&new).await.unwrap().is_some());
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn debug_output_hides_records() {
        let store = Store::default();
        store.records().insert(
            Id::default(),
            record(OffsetDateTime::now_utc() + Duration::minutes(5)),
        );
        assert_eq!(format!("{store:?}"), "Store { records: 1 }");
    }
}
