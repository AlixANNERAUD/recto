//! Sign-in with the OpenID Connect provider, and sign-out.
//!
//! `/auth/login` keeps the [`PendingSignIn`] and the page to come back to in the session, then
//! sends the browser to the provider. `/auth/callback` takes both out of the session before
//! anything else, so a sign-in is completed at most once, whether it succeeds or not, then finds
//! or creates the user of the identity in the database. Without a provider configured, both
//! answer 404.

use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use recto_authentication::{OpenId, OpenIdError, PendingSignIn, SignedIn};
use recto_domain::users;
use sea_orm::DatabaseConnection;
use serde::Deserialize;
use tower_sessions::Session;

use crate::{
    session::{self, local_path},
    state::AuthState,
};

const PENDING: &str = "openid.pending";
const NEXT: &str = "openid.next";

pub fn router() -> Router<AuthState> {
    Router::new()
        .route("/auth/login", get(login))
        .route("/auth/callback", get(callback))
        .route("/auth/logout", post(logout))
}

#[derive(Deserialize)]
struct LoginQuery {
    next: Option<String>,
}

async fn login(
    State(auth): State<AuthState>,
    session: Session,
    Query(query): Query<LoginQuery>,
) -> Response {
    let Some(openid) = &auth.openid else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let next = local_path(query.next.as_deref()).to_owned();
    let (url, pending) = match openid.start().await {
        Ok(started) => started,
        Err(error) => {
            tracing::warn!(%error, "starting a sign-in failed");
            return Redirect::to(&failure_location(&error, &next)).into_response();
        }
    };
    // Short-lived until the provider sends the browser back; sign-in makes it 12 hours.
    session::signing_in(&session);
    let stored = async {
        session.insert(PENDING, &pending).await?;
        session.insert(NEXT, &next).await
    };
    if let Err(error) = stored.await {
        tracing::error!(%error, "keeping the sign-in in the session failed");
        return Redirect::to(&error_location("unavailable", &next)).into_response();
    }
    Redirect::to(url.as_str()).into_response()
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn callback(
    State(auth): State<AuthState>,
    session: Session,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let Some(openid) = &auth.openid else {
        return StatusCode::NOT_FOUND.into_response();
    };
    finish(&auth, openid, &session, query).await.into_response()
}

/// Completes the sign-in the provider sent the browser back from.
async fn finish(
    auth: &AuthState,
    openid: &OpenId,
    session: &Session,
    query: CallbackQuery,
) -> Redirect {
    let pending: Option<PendingSignIn> = session.remove(PENDING).await.ok().flatten();
    let next: Option<String> = session.remove(NEXT).await.ok().flatten();
    let next = local_path(next.as_deref()).to_owned();
    let Some(pending) = pending else {
        tracing::warn!("sign-in callback without a sign-in in progress");
        return Redirect::to(&error_location("state", &next));
    };
    let (Some(code), Some(state)) = (query.code, query.state) else {
        tracing::warn!(
            error = query.error.as_deref().unwrap_or("no code"),
            "the provider did not sign the user in"
        );
        return Redirect::to(&error_location("rejected", &next));
    };
    let signed_in = match openid.finish(pending, &code, &state).await {
        Ok(signed_in) => signed_in,
        Err(error) => {
            tracing::warn!(%error, "sign-in refused");
            return Redirect::to(&failure_location(&error, &next));
        }
    };
    complete(&auth.database, session, &signed_in, &next).await
}

/// Signs in the user of an identity the provider vouched for: finds or creates the user, then
/// starts the session. A disabled user gets the No access page and no session.
async fn complete(
    database: &DatabaseConnection,
    session: &Session,
    signed_in: &SignedIn,
    next: &str,
) -> Redirect {
    let user = match users::sign_in_with_identity(database, &signed_in.identity).await {
        Ok(user) => user,
        Err(recto_domain::Error::Disabled) => {
            tracing::warn!(
                subject = signed_in.identity.subject,
                "disabled user refused"
            );
            return Redirect::to("/no-access");
        }
        Err(error) => {
            tracing::error!(%error, "finding or creating the user failed");
            return Redirect::to(&error_location("unavailable", next));
        }
    };
    if let Err(error) = session::sign_in(session, user.id, &signed_in.id_token).await {
        tracing::error!(%error, "starting the session failed");
        return Redirect::to(&error_location("unavailable", next));
    }
    tracing::info!(user = user.username, "signed in");
    Redirect::to(next)
}

/// Ends the session, then the provider's session when it supports that; either way the
/// browser lands on `/auth/signed-out`. The provider's session is ended even without an ID
/// token (on `/no-access` recto kept none): otherwise the next sign-in silently brings back the
/// same refused account.
async fn logout(State(auth): State<AuthState>, session: Session) -> Redirect {
    let id_token = session::sign_out(&session).await;
    let id_token = id_token.filter(|token| !token.is_empty());
    let signed_out = auth.public("auth/signed-out");
    let provider = auth
        .openid
        .as_ref()
        .and_then(|openid| openid.end_session_url(id_token.as_deref(), &signed_out));
    match provider {
        Some(url) => Redirect::to(url.as_str()),
        None => Redirect::to("/auth/signed-out"),
    }
}

/// Where a failed sign-in sends the browser. Only the kind of failure travels, never the
/// error's text: the page translates it.
fn failure_location(error: &OpenIdError, next: &str) -> String {
    match error {
        OpenIdError::Unavailable(_) => error_location("unavailable", next),
        OpenIdError::StateMismatch => error_location("state", next),
        OpenIdError::Rejected(_) => error_location("rejected", next),
        OpenIdError::AccessDenied => "/no-access".to_owned(),
    }
}

fn error_location(reason: &str, next: &str) -> String {
    let next: String = url::form_urlencoded::byte_serialize(next.as_bytes()).collect();
    format!("/auth/error?reason={reason}&next={next}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_map_to_pages() {
        let cases = [
            (
                OpenIdError::Unavailable("down".into()),
                "/auth/error?reason=unavailable&next=%2Fjobs",
            ),
            (
                OpenIdError::StateMismatch,
                "/auth/error?reason=state&next=%2Fjobs",
            ),
            (
                OpenIdError::Rejected("bad token".into()),
                "/auth/error?reason=rejected&next=%2Fjobs",
            ),
            (OpenIdError::AccessDenied, "/no-access"),
        ];
        for (error, location) in cases {
            assert_eq!(failure_location(&error, "/jobs"), location);
        }
    }

    mod completion {
        use std::sync::Arc;

        use axum::http::header;
        use recto_authentication::SignedIn;
        use recto_domain::users::Identity;
        use sea_orm::DatabaseConnection;

        use super::super::*;
        use crate::session_store::DatabaseSessionStore;

        fn signed_in(subject: &str) -> SignedIn {
            SignedIn {
                identity: Identity {
                    issuer: "https://id.test/".into(),
                    subject: subject.into(),
                    preferred_username: Some(subject.into()),
                    ..Identity::default()
                },
                id_token: "token".into(),
            }
        }

        fn location(redirect: Redirect) -> String {
            let response = redirect.into_response();
            response.headers()[header::LOCATION]
                .to_str()
                .unwrap()
                .to_owned()
        }

        /// A session holding a sign-in in progress, as the callback finds it.
        async fn pending(store: &DatabaseSessionStore) -> Session {
            let session = Session::new(None, Arc::new(store.clone()), None);
            session.insert(NEXT, "/jobs").await.unwrap();
            session.save().await.unwrap();
            Session::new(session.id(), Arc::new(store.clone()), None)
        }

        async fn sessions() -> (DatabaseConnection, DatabaseSessionStore) {
            let database = recto_domain::testing::database().await;
            let store = DatabaseSessionStore::new(database.clone());
            (database, store)
        }

        #[tokio::test]
        async fn a_signed_in_identity_starts_a_new_session() {
            let (database, store) = sessions().await;
            let session = pending(&store).await;
            let before = session.id().unwrap();

            let redirect = complete(&database, &session, &signed_in("alice"), "/jobs").await;

            assert_eq!(location(redirect), "/jobs");
            let user = users::find_by_username(&database, "alice")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(session.get::<i64>("user_id").await.unwrap(), Some(user.id));
            assert_ne!(session.id(), Some(before));
        }

        #[tokio::test]
        async fn a_disabled_user_has_no_access() {
            let (database, store) = sessions().await;
            let alice = users::sign_in_with_identity(&database, &signed_in("alice").identity)
                .await
                .unwrap();
            users::set_disabled(&database, alice.id + 1, alice.id, true)
                .await
                .unwrap();
            let session = pending(&store).await;

            let redirect = complete(&database, &session, &signed_in("alice"), "/jobs").await;

            assert_eq!(location(redirect), "/no-access");
            assert_eq!(session.get::<i64>("user_id").await.unwrap(), None);
        }

        #[tokio::test]
        async fn a_database_failure_offers_a_retry() {
            let (_database, store) = sessions().await;
            // Never migrated: finding the user fails for want of the tables.
            let broken = recto_domain::database::connect("sqlite::memory:")
                .await
                .unwrap();
            let session = pending(&store).await;

            let redirect = complete(&broken, &session, &signed_in("alice"), "/jobs").await;

            assert_eq!(
                location(redirect),
                "/auth/error?reason=unavailable&next=%2Fjobs"
            );
            assert_eq!(session.get::<i64>("user_id").await.unwrap(), None);
        }
    }
}
