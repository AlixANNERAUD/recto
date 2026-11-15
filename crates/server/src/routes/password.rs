//! Sign-in with a local account's password, and the change of that password from the Account
//! page. Both are plain forms, so they work without JavaScript. A refused form is answered with
//! its page rendered again around the problem (the login typed stays, a password never comes
//! back); an accepted one with a redirect, so a reload does not post it twice.
//!
//! Password sign-in is rate limited per client address: a burst of 5 attempts, then one every
//! 4 seconds. Behind a reverse proxy the peer address is the proxy's, so every user shares one
//! budget unless `RECTO_TRUST_FORWARDED_HEADERS` is set; setting it without a proxy lets clients
//! pick their own address and escape the limit.

use std::{
    net::IpAddr,
    sync::{Arc, Weak},
    time::Duration,
};

use axum::{
    Extension, Form, Router,
    extract::{Query, State},
    http::{Request, StatusCode, Uri, request::Parts},
    middleware,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use leptos::prelude::provide_context;
use recto_app::{
    pages::{
        account::{PasswordChange, PasswordRefusal},
        sign_in::{SignInForm, SignInMethods, SignInRefusal},
    },
    state::AppState,
};
use recto_authentication::User;
use recto_domain::{
    Error, Problem,
    passwords::{self, PasswordPolicy},
    users,
};
use serde::Deserialize;
use tower_governor::{
    governor::GovernorConfigBuilder,
    key_extractor::{KeyExtractor, PeerIpKeyExtractor, SmartIpKeyExtractor},
};
use tower_sessions::Session;

use super::render;
use crate::{
    session::{self, local_path},
    state::AuthState,
};

#[derive(Clone)]
struct PasswordState {
    app: AppState,
    methods: SignInMethods,
    policy: PasswordPolicy,
    attempts: Attempts,
}

/// `GET` and `POST /sign-in`, and `POST /account/password`. The routes exist with local accounts
/// off too: the page then sends visitors to single sign-on, and the forms answer 404.
pub fn router(app: AppState, auth: &AuthState) -> Router {
    let state = PasswordState {
        app,
        methods: auth.sign_in_methods(),
        policy: auth.password_policy,
        attempts: Attempts::new(auth.trust_forwarded_headers),
    };
    Router::new()
        .route("/sign-in", get(page).post(sign_in))
        .route("/account/password", post(change_password))
        // `/sign-in` is public (see `session::is_public`); the password change needs a user.
        .route_layer(middleware::from_fn(session::require_user))
        .with_state(state)
}

/// Password sign-in attempts per client address.
#[derive(Clone)]
struct Attempts {
    /// Counts an attempt from an address; `false` when it is one too many.
    allow: Arc<dyn Fn(IpAddr) -> bool + Send + Sync>,
    trust_forwarded_headers: bool,
}

impl Attempts {
    /// Unlike nanobom, which puts tower_governor's layer on the route, recto checks the limit
    /// in the handler: a refused attempt then gets the sign-in page with its message and the
    /// login typed, rather than a bare 429.
    fn new(trust_forwarded_headers: bool) -> Self {
        let configuration = GovernorConfigBuilder::default()
            .per_second(4)
            .burst_size(5)
            .finish()
            .expect("valid rate limits");
        let limiter = Arc::clone(configuration.limiter());
        forget_old_clients(Arc::downgrade(&limiter), |limiter| limiter.retain_recent());
        Self {
            allow: Arc::new(move |address| limiter.check_key(&address).is_ok()),
            trust_forwarded_headers,
        }
    }

    /// The address attempts are counted by: the peer's, or with trusted forwarded headers the
    /// one they name (else the peer's).
    fn client(&self, parts: &Parts) -> IpAddr {
        let request = Request::from_parts(parts.clone(), ());
        let address = if self.trust_forwarded_headers {
            SmartIpKeyExtractor.extract(&request)
        } else {
            PeerIpKeyExtractor.extract(&request)
        };
        address.unwrap_or_else(|_| {
            // The server always knows its peers; only a router served without connection
            // information lands here, and then every client shares one budget.
            tracing::warn!("the client address of a sign-in is unknown");
            IpAddr::from([0, 0, 0, 0])
        })
    }
}

/// Drops the rate-limiting state of clients not seen recently, so it does not grow forever.
/// It stops with the router that owns the limiter.
fn forget_old_clients<T: Send + Sync + 'static>(
    limiter: Weak<T>,
    retain_recent: impl Fn(&T) + Send + 'static,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let Some(limiter) = limiter.upgrade() else {
                break;
            };
            retain_recent(&limiter);
        }
    });
}

#[derive(Deserialize)]
struct NextQuery {
    next: Option<String>,
}

/// `GET /sign-in?next=<path>`: the sign-in page. A signed-in user goes on to `next`; with only
/// single sign-on, the visitor goes straight to the provider, as before local accounts existed.
async fn page(
    State(state): State<PasswordState>,
    Query(query): Query<NextQuery>,
    parts: Parts,
) -> Response {
    let next = local_path(query.next.as_deref()).to_owned();
    if parts.extensions.get::<User>().is_some() {
        return Redirect::to(&next).into_response();
    }
    if !state.methods.password {
        let next: String = url::form_urlencoded::byte_serialize(next.as_bytes()).collect();
        return Redirect::to(&format!("/auth/login?next={next}")).into_response();
    }
    let form = SignInForm {
        next,
        ..SignInForm::default()
    };
    render(
        &state.app,
        state.methods,
        parts,
        StatusCode::OK,
        move || {
            provide_context(form.clone());
        },
    )
    .await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SignInFields {
    login: String,
    password: String,
    next: Option<String>,
}

/// `POST /sign-in`: signs in with a username or email address and a password, then goes to
/// `next`. A refusal renders the page again: 401 for wrong credentials, whatever was wrong (so
/// log watchers such as fail2ban can count them), 429 past the rate limit.
async fn sign_in(
    State(state): State<PasswordState>,
    session: Session,
    parts: Parts,
    Form(fields): Form<SignInFields>,
) -> Response {
    if !state.methods.password {
        return StatusCode::NOT_FOUND.into_response();
    }
    let next = local_path(fields.next.as_deref()).to_owned();
    let client = state.attempts.client(&parts);
    let refusal = if (state.attempts.allow)(client) {
        match signed_in(&state, &session, fields.login.clone(), fields.password).await {
            Ok(username) => {
                tracing::info!(user = username, "signed in with a password");
                return Redirect::to(&next).into_response();
            }
            Err(refusal) => refusal,
        }
    } else {
        SignInRefusal::TooMany
    };
    let status = match refusal {
        SignInRefusal::Invalid => StatusCode::UNAUTHORIZED,
        SignInRefusal::TooMany => StatusCode::TOO_MANY_REQUESTS,
        SignInRefusal::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    };
    if refusal != SignInRefusal::Unavailable {
        tracing::warn!(%client, ?refusal, "password sign-in refused");
    }
    let form = SignInForm {
        next,
        login: fields.login,
        refusal: Some(refusal),
    };
    render(&state.app, state.methods, parts, status, move || {
        provide_context(form.clone());
    })
    .await
}

/// Checks the password and starts the session; returns the username.
async fn signed_in(
    state: &PasswordState,
    session: &Session,
    login: String,
    password: String,
) -> Result<String, SignInRefusal> {
    // No password is that long: refused without spending a hash on it.
    if password.chars().count() > passwords::MAXIMUM_LENGTH {
        return Err(SignInRefusal::Invalid);
    }
    let user = match users::sign_in_with_password(&state.app.database, &login, password).await {
        Ok(user) => user,
        Err(Error::InvalidCredentials) => return Err(SignInRefusal::Invalid),
        Err(error) => {
            tracing::error!(%error, "password sign-in failed");
            return Err(SignInRefusal::Unavailable);
        }
    };
    if let Err(error) = session::sign_in(session, user.id, "").await {
        tracing::error!(%error, "starting the session failed");
        return Err(SignInRefusal::Unavailable);
    }
    Ok(user.username)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PasswordFields {
    current: String,
    new: String,
    confirm: String,
}

/// `POST /account/password`: changes the signed-in user's password, then goes back to
/// `/account?changed=1`. A refusal renders the Account page again with 400. Users the Account
/// page is not for get the not-found page.
async fn change_password(
    State(state): State<PasswordState>,
    Extension(user): Extension<User>,
    session: Session,
    mut parts: Parts,
    Form(fields): Form<PasswordFields>,
) -> Response {
    if !(state.methods.password && user.is_local) {
        return render(
            &state.app,
            state.methods,
            parts,
            StatusCode::NOT_FOUND,
            || {},
        )
        .await;
    }
    match change(&state, &user, &session, fields).await {
        Ok(()) => {
            tracing::info!(user = user.username, "password changed");
            Redirect::to("/account?changed=1").into_response()
        }
        Err(refusal) => {
            parts.uri = Uri::from_static("/account");
            render(
                &state.app,
                state.methods,
                parts,
                StatusCode::BAD_REQUEST,
                move || {
                    provide_context(PasswordChange { refusal });
                },
            )
            .await
        }
    }
}

/// Checks the current password and the new one, sets it, which ends every session of the user,
/// then signs this session in again so that it alone carries on.
async fn change(
    state: &PasswordState,
    user: &User,
    session: &Session,
    fields: PasswordFields,
) -> Result<(), PasswordRefusal> {
    let database = &state.app.database;
    let unavailable = |error: Error| {
        tracing::error!(%error, "changing a password failed");
        PasswordRefusal::Unavailable
    };
    let row = users::find_by_id(database, user.id)
        .await
        .map_err(unavailable)?
        .ok_or(PasswordRefusal::Unavailable)?;
    let current_matches = match row.password_hash {
        Some(hash) if fields.current.chars().count() <= passwords::MAXIMUM_LENGTH => {
            passwords::verify(fields.current, hash).await
        }
        _ => false,
    };
    if !current_matches {
        return Err(PasswordRefusal::WrongCurrent);
    }
    if fields.new != fields.confirm {
        return Err(PasswordRefusal::Mismatch);
    }
    match users::set_password(database, user.id, fields.new, &state.policy).await {
        Ok(()) => {}
        Err(Error::Invalid(Problem::PasswordTooShort { minimum })) => {
            return Err(PasswordRefusal::TooShort { minimum });
        }
        Err(Error::Invalid(Problem::PasswordTooLong { maximum })) => {
            return Err(PasswordRefusal::TooLong { maximum });
        }
        Err(error) => return Err(unavailable(error)),
    }
    // The password is changed whatever happens next: a failure here only signs this session
    // out too, which the next request shows.
    let cutoff = match users::find_by_id(database, user.id).await {
        Ok(row) => row.and_then(|row| row.sessions_valid_after),
        Err(error) => {
            tracing::warn!(%error, "reading the new sessions cutoff failed");
            None
        }
    };
    if let Err(error) = session::sign_in_again(session, user.id, cutoff).await {
        tracing::error!(%error, "keeping the session after a password change failed");
    }
    Ok(())
}
