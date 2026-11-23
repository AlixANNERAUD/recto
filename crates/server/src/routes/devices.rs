//! The device forms of the Account page: `POST /account/devices` creates a device password,
//! `POST /account/devices/{id}/revoke` revokes one, and `POST /account/direct-printer` sets
//! where documents from devices go. All are plain forms, behind the session and the cross-site
//! check like every other form, and answer the not-found page when OS integration is off.
//!
//! A new password is answered with the Account page showing it once, with
//! `Cache-Control: no-store`, never with a redirect: the secret must not enter an address, the
//! history or a cache. Logs name the device password's id and name, never its secret.

use axum::{
    Extension, Form, Router,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, Uri, header, request::Parts},
    middleware,
    response::{IntoResponse, Redirect, Response},
    routing::post,
};
use leptos::prelude::provide_context;
use recto_app::{
    pages::{
        account::{DevicesProblem, DevicesRefusal, NewDevicePassword},
        sign_in::SignInMethods,
    },
    state::{AppState, User},
};
use recto_domain::{
    Error, Problem,
    access::{DeviceKind, Usable},
    device_passwords, users,
};
use serde::Deserialize;

use super::render;
use crate::{session, state::AuthState};

#[derive(Clone)]
struct DevicesState {
    app: AppState,
    methods: SignInMethods,
    /// Where devices reach the printer: `ipps://host:port`.
    printer_address: String,
}

/// `POST /account/devices` and `POST /account/devices/{id}/revoke`.
pub fn router(app: AppState, auth: &AuthState) -> Router {
    let state = DevicesState {
        app,
        methods: auth.sign_in_methods(),
        printer_address: super::ipp::printer_address(&auth.public_url).0,
    };
    Router::new()
        .route("/account/devices", post(create))
        .route("/account/devices/{id}/revoke", post(revoke))
        .route("/account/direct-printer", post(direct_printer))
        .route_layer(middleware::from_fn(session::require_user))
        .with_state(state)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NewDeviceFields {
    name: String,
}

/// Creates a device password and shows its secret, once.
async fn create(
    State(state): State<DevicesState>,
    Extension(user): Extension<User>,
    mut parts: Parts,
    Form(fields): Form<NewDeviceFields>,
) -> Response {
    if !state.app.os_integration {
        return not_found(&state, parts).await;
    }
    parts.uri = Uri::from_static("/account");
    let created = device_passwords::create(
        &state.app.database,
        user.id,
        &fields.name,
        chrono::Utc::now(),
    )
    .await;
    let created = match created {
        Ok(created) => created,
        Err(error) => {
            let (status, problem) = refused(error);
            let refusal = DevicesRefusal {
                problem,
                name: fields.name,
            };
            return render(&state.app, state.methods, parts, status, move || {
                provide_context(refusal.clone());
            })
            .await;
        }
    };
    let name = fields.name.trim().to_owned();
    tracing::info!(
        user = user.username,
        device_password_id = created.id,
        device_name = name,
        "device password created"
    );
    let shown = NewDevicePassword {
        name,
        print_address: format!("{}/ipp/print", state.printer_address),
        personal_address: format!(
            "{}/device/{}/ipp/print",
            state.printer_address, created.secret
        ),
        secret: created.secret,
        username: user.username,
    };
    let mut response = render(
        &state.app,
        state.methods,
        parts,
        StatusCode::OK,
        move || {
            provide_context(shown.clone());
        },
    )
    .await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// The status and the problem a refused creation is shown with.
fn refused(error: Error) -> (StatusCode, DevicesProblem) {
    match error {
        Error::Invalid(Problem::DeviceNameInvalid { maximum }) => (
            StatusCode::BAD_REQUEST,
            DevicesProblem::NameInvalid { maximum },
        ),
        Error::Invalid(Problem::DeviceNameControl) => {
            (StatusCode::BAD_REQUEST, DevicesProblem::NameControl)
        }
        Error::Invalid(Problem::TooManyDevicePasswords { maximum }) => {
            (StatusCode::BAD_REQUEST, DevicesProblem::TooMany { maximum })
        }
        error => {
            tracing::error!(%error, "creating a device password failed");
            (StatusCode::SERVICE_UNAVAILABLE, DevicesProblem::Unavailable)
        }
    }
}

/// Revokes one of the user's device passwords: the device's next request is refused. Another
/// user's password, or one already revoked, gets the not-found page.
async fn revoke(
    State(state): State<DevicesState>,
    Extension(user): Extension<User>,
    Path(id): Path<i64>,
    mut parts: Parts,
) -> Response {
    if !state.app.os_integration {
        return not_found(&state, parts).await;
    }
    match device_passwords::revoke(&state.app.database, user.id, id).await {
        Ok(true) => {
            tracing::info!(
                user = user.username,
                device_password_id = id,
                "device password revoked"
            );
            Redirect::to("/account?done=revoked").into_response()
        }
        Ok(false) => not_found(&state, parts).await,
        Err(error) => {
            tracing::error!(%error, "revoking a device password failed");
            parts.uri = Uri::from_static("/account");
            let refusal = DevicesRefusal {
                problem: DevicesProblem::Unavailable,
                name: String::new(),
            };
            render(
                &state.app,
                state.methods,
                parts,
                StatusCode::SERVICE_UNAVAILABLE,
                move || provide_context(refusal.clone()),
            )
            .await
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DirectPrinterFields {
    printer: String,
}

/// Sets the printer documents from the user's devices print to at once; empty holds them.
/// Answers `303 /account?done=direct`, or `/account?error=direct_printer` for a printer CUPS
/// does not have or the user may not use (the same answer, so the form does not reveal which
/// printers exist), or `/account?error=direct_unavailable`.
async fn direct_printer(
    State(state): State<DevicesState>,
    Extension(user): Extension<User>,
    parts: Parts,
    Form(fields): Form<DirectPrinterFields>,
) -> Response {
    if !state.app.os_integration {
        return not_found(&state, parts).await;
    }
    let app = &state.app;
    let wanted = fields.printer.trim();
    let printer = if wanted.is_empty() {
        None
    } else {
        let usable = match Usable::read(
            &app.database,
            DeviceKind::Printer,
            user.id,
            user.is_administrator,
        )
        .await
        {
            Ok(usable) => usable,
            Err(error) => {
                tracing::error!(%error, "reading the printer access rules failed");
                return Redirect::to("/account?error=direct_unavailable").into_response();
            }
        };
        if !usable.allows(wanted) {
            return Redirect::to("/account?error=direct_printer").into_response();
        }
        match app.print.printer(wanted).await {
            // Kept under the name CUPS answered with, whose rule is checked again: CUPS
            // ignores the case of printer names.
            Ok(printer) if usable.allows(&printer.name) => Some(printer.name),
            Ok(_) | Err(recto_printing::PrintError::PrinterNotFound(_)) => {
                return Redirect::to("/account?error=direct_printer").into_response();
            }
            Err(error) => {
                tracing::warn!(%error, "checking the direct printer failed");
                return Redirect::to("/account?error=direct_unavailable").into_response();
            }
        }
    };
    match users::set_direct_printer(&app.database, user.id, printer.as_deref()).await {
        Ok(()) => {
            tracing::info!(
                user = user.username,
                printer = printer.as_deref().unwrap_or("none"),
                "direct printer set"
            );
            Redirect::to("/account?done=direct").into_response()
        }
        Err(error) => {
            tracing::error!(%error, "setting the direct printer failed");
            Redirect::to("/account?error=direct_unavailable").into_response()
        }
    }
}

/// The not-found page, at the form's own address, which no page has.
async fn not_found(state: &DevicesState, parts: Parts) -> Response {
    render(
        &state.app,
        state.methods,
        parts,
        StatusCode::NOT_FOUND,
        || {},
    )
    .await
}
