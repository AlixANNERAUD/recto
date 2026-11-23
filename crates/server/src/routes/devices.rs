//! The device forms of the Account page: `POST /account/devices` creates a device password,
//! `POST /account/devices/{id}/revoke` revokes one, `POST /account/devices/profile` downloads
//! the Apple profile of a new one, and `POST /account/direct-printer` sets where documents from
//! devices go. All are plain forms, behind the session and the cross-site check like every
//! other form, and answer the not-found page when OS integration is off.
//!
//! A new password is answered with the Account page showing it once, with
//! `Cache-Control: no-store`, never with a redirect: the secret must not enter an address, the
//! history or a cache. Logs name the device password's id and name, never its secret.
//!
//! The profile is answered in two steps: the form posts the secret, and the answer sends the
//! browser to `GET /account/devices/profile/{ticket}`, which serves the file. Whether Safari
//! on iOS installs a profile answered to a POST is not documented, while profiles served by a
//! link are the usual way and known to work. The ticket is random, never the secret, works only
//! for the user who asked, and only for 5 minutes.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Extension, Form, Router,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, Uri, header, request::Parts},
    middleware,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use leptos::prelude::provide_context;
use recto_app::{
    pages::{
        account::{DevicesProblem, DevicesRefusal, NewDevicePassword, ScanAddress},
        sign_in::SignInMethods,
    },
    state::{AppState, User},
};
use recto_domain::{
    Error, Problem,
    access::{DeviceKind, Usable},
    device_passwords, users,
};
use recto_printing::printer::VirtualPrinter;
use recto_scanning::escl;
use serde::Deserialize;
use uuid::Uuid;

mod profile;

use profile::AirPrintProfile;

use super::{not_found, render};
use crate::{device_auth, session, state::AuthState};

#[derive(Clone)]
struct DevicesState {
    app: AppState,
    methods: SignInMethods,
    /// Where devices reach the printer: `ipps://host:port`.
    printer_address: String,
    /// Where devices reach the scanners: `https://host:port`, the port always written.
    web_address: String,
    /// The printer as a profile names it: the public host, its port, and whether it is TLS.
    host: Arc<str>,
    port: u16,
    tls: bool,
    /// The printer's `printer-uuid`, as the IPP routes answer it.
    printer_uuid: Uuid,
    tickets: ProfileTickets,
}

/// `POST /account/devices`, `POST /account/devices/{id}/revoke`, the profile download and
/// `POST /account/direct-printer`.
pub fn router(app: AppState, auth: &AuthState) -> Router {
    let (printer_address, _) = super::ipp::printer_address(&auth.public_url);
    let (host, port, tls) = device_auth::public_host(&auth.public_url);
    let state = DevicesState {
        app,
        methods: auth.sign_in_methods(),
        printer_address,
        web_address: format!("{}://{host}:{port}", auth.public_url.scheme()),
        host: Arc::from(host),
        port,
        tls,
        printer_uuid: VirtualPrinter::uuid_for(auth.public_url.as_str()),
        tickets: ProfileTickets::default(),
    };
    Router::new()
        .route("/account/devices", post(create))
        .route("/account/devices/{id}/revoke", post(revoke))
        .route("/account/devices/profile", post(ask_profile))
        .route("/account/devices/profile/{ticket}", get(download_profile))
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
        return not_found(&state.app, state.methods, parts).await;
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
    let scan_addresses = scan_addresses(&state, &user, &created.secret).await;
    let shown = NewDevicePassword {
        name,
        print_address: format!("{}/ipp/print", state.printer_address),
        personal_address: format!(
            "{}{}",
            state.printer_address,
            device_auth::personal_printer_path(&created.secret)
        ),
        scan_addresses,
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

/// The personal eSCL address of each scanner `user` may use, as listed now. None when the list
/// or the access rules cannot be read: the password is still shown, with its printer addresses.
async fn scan_addresses(state: &DevicesState, user: &User, secret: &str) -> Vec<ScanAddress> {
    let scanners = match state.app.scan.scanners(false).await {
        Ok(scanners) => scanners,
        Err(error) => {
            tracing::info!(%error, "no scan addresses: the scanners could not be listed");
            return Vec::new();
        }
    };
    let usable = Usable::read(
        &state.app.database,
        DeviceKind::Scanner,
        user.id,
        user.is_administrator,
    )
    .await;
    let usable = match usable {
        Ok(usable) => usable,
        Err(error) => {
            tracing::error!(%error, "reading the scanner access rules failed");
            return Vec::new();
        }
    };
    scanners
        .into_iter()
        .filter(|scanner| usable.allows(&scanner.id))
        .map(|scanner| ScanAddress {
            scanner: scanner.label(),
            address: format!(
                "{}{}/",
                state.web_address,
                device_auth::personal_scanner_path(secret, &escl::scanner_key(&scanner.id))
            ),
        })
        .collect()
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
        return not_found(&state.app, state.methods, parts).await;
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
        Ok(false) => not_found(&state.app, state.methods, parts).await,
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
        return not_found(&state.app, state.methods, parts).await;
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

/// Profiles asked for and not yet expired, by ticket. One per device password: asking again
/// replaces the ticket, so a user has at most as many as device passwords.
#[derive(Clone, Default)]
struct ProfileTickets {
    tickets: Arc<Mutex<HashMap<String, Ticket>>>,
}

struct Ticket {
    user_id: i64,
    device_password_id: i64,
    secret: String,
    expires_at: Instant,
}

impl ProfileTickets {
    /// Long enough for a slow phone to start the download, short enough that a ticket left in
    /// the history is dead.
    const LIFETIME: Duration = Duration::from_secs(5 * 60);

    fn issue(&self, user_id: i64, device_password_id: i64, secret: String) -> String {
        let ticket = Uuid::new_v4().simple().to_string();
        let now = Instant::now();
        let mut tickets = self
            .tickets
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        tickets.retain(|_, old| {
            old.expires_at > now
                && !(old.user_id == user_id && old.device_password_id == device_password_id)
        });
        tickets.insert(
            ticket.clone(),
            Ticket {
                user_id,
                device_password_id,
                secret,
                expires_at: now + Self::LIFETIME,
            },
        );
        ticket
    }

    /// The secret of `user_id`'s ticket, while it lasts.
    fn secret(&self, user_id: i64, ticket: &str) -> Option<String> {
        let tickets = self
            .tickets
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        tickets
            .get(ticket)
            .filter(|found| found.user_id == user_id && found.expires_at > Instant::now())
            .map(|found| found.secret.clone())
    }

    fn forget(&self, ticket: &str) {
        let mut tickets = self
            .tickets
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        tickets.remove(ticket);
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ProfileFields {
    secret: String,
}

/// Checks that the secret posted back is one of the user's live device passwords, and sends
/// the browser to the profile's download. Anything else gets the not-found page, so the form
/// does not tell whether a secret exists.
async fn ask_profile(
    State(state): State<DevicesState>,
    Extension(user): Extension<User>,
    parts: Parts,
    Form(fields): Form<ProfileFields>,
) -> Response {
    if !state.app.os_integration {
        return not_found(&state.app, state.methods, parts).await;
    }
    match device_passwords::owns(&state.app.database, user.id, &fields.secret).await {
        Ok(Some(id)) => {
            let ticket = state.tickets.issue(user.id, id, fields.secret);
            Redirect::to(&format!("/account/devices/profile/{ticket}")).into_response()
        }
        Ok(None) => not_found(&state.app, state.methods, parts).await,
        Err(error) => {
            tracing::error!(%error, "checking a device password failed");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

/// Serves the profile of a ticket: an AirPrint printer at the personal address of its device
/// password, with `Cache-Control: no-store` since it holds the secret. A ticket of another
/// user, an expired one, or one whose password was revoked since gets the not-found page.
async fn download_profile(
    State(state): State<DevicesState>,
    Extension(user): Extension<User>,
    Path(ticket): Path<String>,
    parts: Parts,
) -> Response {
    // The ticket's address is not the page's: the not-found page must not echo it.
    let mut parts = parts;
    parts.uri = Uri::from_static("/account/devices/profile");
    if !state.app.os_integration {
        return not_found(&state.app, state.methods, parts).await;
    }
    let Some(secret) = state.tickets.secret(user.id, &ticket) else {
        return not_found(&state.app, state.methods, parts).await;
    };
    let database = &state.app.database;
    let device = match device_passwords::owns(database, user.id, &secret).await {
        Ok(Some(id)) => match device_passwords::list(database, user.id).await {
            Ok(passwords) => passwords.into_iter().find(|password| password.id == id),
            Err(error) => {
                tracing::error!(%error, "checking a device password failed");
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
        },
        Ok(None) => None,
        Err(error) => {
            tracing::error!(%error, "checking a device password failed");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };
    let Some(device) = device else {
        state.tickets.forget(&ticket);
        return not_found(&state.app, state.methods, parts).await;
    };
    let locale = recto_app::i18n::from_headers(&parts.headers);
    let brand = &state.app.branding.name;
    let display_name = recto_app::messages::profile_name(brand, &device.name, locale);
    let description = recto_app::messages::profile_description(brand, &device.name, locale);
    let resource_path = device_auth::personal_printer_path(&secret);
    let profile = AirPrintProfile {
        brand,
        display_name: &display_name,
        description: &description,
        printer_uuid: state.printer_uuid,
        device_password_id: device.id,
        host: &state.host,
        port: state.port,
        tls: state.tls,
        resource_path: &resource_path,
    };
    tracing::info!(
        user = user.username,
        device_password_id = device.id,
        device_name = device.name,
        "device profile downloaded"
    );
    let disposition = format!("attachment; filename=\"{}\"", profile.file_name());
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/x-apple-aspen-config"),
            ),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&disposition)
                    .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        profile.to_xml(),
    )
        .into_response()
}
