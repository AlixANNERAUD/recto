//! The administration pages and their forms, for administrators only: anyone else gets the
//! not-found page with 404 on every route, so the routes do not reveal that they exist. The
//! pages are rendered here rather than by the interface's router, so their data is loaded only
//! once the user is known to administer recto.
//!
//! The forms are plain posts, so they work without JavaScript. An accepted one redirects to the
//! page it came from with a notice; a refused one renders that page again with the problem,
//! keeping what was typed except a password.

use axum::{
    Extension, Form, Router,
    body::Bytes,
    extract::{Path, Request, State},
    http::{StatusCode, Uri, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use leptos::prelude::provide_context;
use recto_app::{
    pages::{
        administration::{
            devices::{
                DeviceAccess, DeviceKind as PageKind, DeviceRow, DevicesPageData, DevicesProblem,
                DriverChoice, GroupChoice, NewPrinterDraft, Notice as DeviceNotice,
                PrinterControls, PrinterTools,
            },
            groups::{GroupMember, GroupRow, GroupsPageData},
            users::{NewUserDraft, Notice, UserRow, UsersPageData, UsersProblem, UsersRefusal},
        },
        sign_in::SignInMethods,
    },
    state::{AppState, User},
};
use recto_domain::{
    Error, Problem,
    access::{self, Access, DeviceKind},
    groups,
    passwords::PasswordPolicy,
    users::{self, NewLocalUser, UserSummary},
};
use recto_printing::{AdminError, Driver, NewPrinter, PrinterAdmin, PrinterState};
use serde::Deserialize;

use super::render;
use crate::{session, state::AuthState};

/// Where the Users page is, and where every form returns to.
const USERS: &str = "/administration/users";
const PRINTERS: &str = "/administration/printers";
const SCANNERS: &str = "/administration/scanners";

#[derive(Clone)]
struct AdministrationState {
    app: AppState,
    methods: SignInMethods,
    policy: PasswordPolicy,
}

/// `GET /administration/users`, `/groups`, `/printers` and `/scanners`, `/administration`
/// leading to the first, and the forms of the Users, Printers and Scanners pages.
pub fn router(app: AppState, auth: &AuthState) -> Router {
    let state = AdministrationState {
        app,
        methods: auth.sign_in_methods(),
        policy: auth.password_policy,
    };
    Router::new()
        .route("/administration", get(|| async { Redirect::to(USERS) }))
        .route(USERS, get(users_page).post(create))
        .route(
            "/administration/users/{id}/administrator",
            post(set_administrator),
        )
        .route("/administration/users/{id}/disabled", post(set_disabled))
        .route("/administration/users/{id}/password", post(set_password))
        .route("/administration/groups", get(groups_page))
        .route(PRINTERS, get(printers_page))
        .route(SCANNERS, get(scanners_page))
        .merge(printer_forms())
        .merge(device_forms(DeviceKind::Printer))
        .merge(device_forms(DeviceKind::Scanner))
        // The last layer runs first: anonymous visitors are sent to sign in before the check.
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_administrator,
        ))
        .route_layer(middleware::from_fn(session::require_user))
        .with_state(state)
}

/// `POST <page>/access` and `<page>/forget` for one kind of device.
fn device_forms(kind: DeviceKind) -> Router<AdministrationState> {
    let page = page_kind(kind).path();
    Router::new()
        .route(
            &format!("{page}/access"),
            post(move |state, actor, parts, body| set_access(state, actor, parts, kind, body)),
        )
        .route(
            &format!("{page}/forget"),
            post(move |state, actor, parts, body| forget(state, actor, parts, kind, body)),
        )
}

/// Middleware letting only administrators through; anyone else gets the not-found page, before
/// the form is even read.
async fn require_administrator(
    State(state): State<AdministrationState>,
    request: Request,
    next: Next,
) -> Response {
    if request
        .extensions()
        .get::<User>()
        .is_some_and(|user| user.is_administrator)
    {
        return next.run(request).await;
    }
    not_found(&state, request.into_parts().0).await
}

/// The not-found page: without their data, the administration pages render it.
async fn not_found(state: &AdministrationState, parts: Parts) -> Response {
    render(
        &state.app,
        state.methods,
        parts,
        StatusCode::NOT_FOUND,
        || {},
    )
    .await
}

/// `GET /administration/users`.
async fn users_page(State(state): State<AdministrationState>, parts: Parts) -> Response {
    users_view(&state, parts, StatusCode::OK, None, NewUserDraft::default()).await
}

/// The Users page, with the refusal of a form if any.
async fn users_view(
    state: &AdministrationState,
    mut parts: Parts,
    status: StatusCode,
    refusal: Option<UsersRefusal>,
    draft: NewUserDraft,
) -> Response {
    let users = match users::list(&state.app.database).await {
        Ok(users) => Some(users.into_iter().map(user_row).collect()),
        Err(error) => {
            tracing::error!(%error, "listing the users failed");
            None
        }
    };
    let status = if users.is_none() && status == StatusCode::OK {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        status
    };
    if refusal.is_some() {
        // A refused form renders the page it came from, without a notice from the address.
        parts.uri = Uri::from_static(USERS);
    }
    let data = UsersPageData {
        users,
        refusal,
        draft,
    };
    render(&state.app, state.methods, parts, status, move || {
        provide_context(data.clone());
    })
    .await
}

fn user_row(summary: UserSummary) -> UserRow {
    let user = summary.user;
    UserRow {
        id: user.id,
        username: user.username,
        name: user.display_name,
        email: user.email,
        groups: summary.groups,
        has_password: summary.has_password,
        single_sign_on: summary.has_identity,
        is_administrator: user.is_administrator,
        administrator_from_claim: user.administrator_from_claim,
        is_disabled: user.is_disabled,
        last_sign_in: user.last_sign_in_at.map(|time| time.timestamp()),
    }
}

/// `GET /administration/groups`.
async fn groups_page(State(state): State<AdministrationState>, parts: Parts) -> Response {
    let groups = match groups::list(&state.app.database).await {
        Ok(groups) => Some(
            groups
                .into_iter()
                .map(|(group, members)| GroupRow {
                    name: group.name,
                    members: members
                        .into_iter()
                        .map(|member| GroupMember {
                            username: member.username,
                            name: member.display_name,
                            is_disabled: member.is_disabled,
                        })
                        .collect(),
                })
                .collect(),
        ),
        Err(error) => {
            tracing::error!(%error, "listing the groups failed");
            None
        }
    };
    let status = if groups.is_some() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let data = GroupsPageData { groups };
    render(&state.app, state.methods, parts, status, move || {
        provide_context(data.clone());
    })
    .await
}

/// Back to the Users page, saying what was done to whom.
fn done(notice: Notice, user_id: i64) -> Response {
    Redirect::to(&format!("{USERS}?done={}&user={user_id}", notice.code())).into_response()
}

/// The status and the problem a refused change is shown with.
fn refused(error: Error) -> (StatusCode, UsersProblem) {
    let problem = match error {
        Error::Invalid(Problem::UsernameInvalid) => UsersProblem::UsernameInvalid,
        Error::Invalid(Problem::UsernameTaken) => UsersProblem::UsernameTaken,
        Error::Invalid(Problem::EmailTaken) => UsersProblem::EmailTaken,
        Error::Invalid(Problem::PasswordTooShort { minimum }) => {
            UsersProblem::PasswordTooShort { minimum }
        }
        Error::Invalid(Problem::PasswordTooLong { maximum }) => {
            UsersProblem::PasswordTooLong { maximum }
        }
        Error::Invalid(Problem::OwnAccount) => UsersProblem::OwnAccount,
        Error::Invalid(Problem::AdministratorFromClaim) => UsersProblem::AdministratorFromClaim,
        Error::NotFound => return (StatusCode::NOT_FOUND, UsersProblem::UserNotFound),
        error @ (Error::Database(_) | Error::Disabled | Error::InvalidCredentials) => {
            tracing::error!(%error, "changing a user failed");
            return (StatusCode::SERVICE_UNAVAILABLE, UsersProblem::Unavailable);
        }
    };
    (StatusCode::BAD_REQUEST, problem)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NewUserFields {
    username: String,
    display_name: String,
    email: String,
    password: String,
    /// A checkbox: present when ticked.
    administrator: Option<String>,
}

/// `POST /administration/users`: creates a local user. Without local accounts there is no such
/// form, so the route answers as if it did not exist.
async fn create(
    State(state): State<AdministrationState>,
    parts: Parts,
    Form(fields): Form<NewUserFields>,
) -> Response {
    if !state.methods.password {
        return not_found(&state, parts).await;
    }
    let draft = NewUserDraft {
        username: fields.username.clone(),
        display_name: fields.display_name.clone(),
        email: fields.email.clone(),
        administrator: fields.administrator.is_some(),
    };
    let email = Some(fields.email).filter(|email| !email.trim().is_empty());
    let user = NewLocalUser {
        username: fields.username,
        display_name: fields.display_name,
        email,
        password: fields.password,
        is_administrator: draft.administrator,
    };
    match users::create_local(&state.app.database, user, &state.policy).await {
        Ok(user) => done(Notice::Created, user.id),
        Err(error) => {
            let (status, problem) = refused(error);
            let refusal = UsersRefusal {
                problem,
                username: None,
            };
            users_view(&state, parts, status, Some(refusal), draft).await
        }
    }
}

#[derive(Deserialize)]
struct ValueField {
    value: bool,
}

/// `POST /administration/users/{id}/administrator` with `value=true|false`.
async fn set_administrator(
    State(state): State<AdministrationState>,
    Extension(actor): Extension<User>,
    Path(user_id): Path<i64>,
    parts: Parts,
    Form(ValueField { value }): Form<ValueField>,
) -> Response {
    let changed = users::set_administrator(&state.app.database, actor.id, user_id, value).await;
    if changed.is_ok() {
        tracing::info!(
            by = actor.username,
            user_id,
            value,
            "administration changed"
        );
    }
    let notice = if value {
        Notice::Granted
    } else {
        Notice::Removed
    };
    finish(&state, parts, user_id, changed, notice).await
}

/// `POST /administration/users/{id}/disabled` with `value=true|false`. Disabling ends every
/// session of the user.
async fn set_disabled(
    State(state): State<AdministrationState>,
    Extension(actor): Extension<User>,
    Path(user_id): Path<i64>,
    parts: Parts,
    Form(ValueField { value }): Form<ValueField>,
) -> Response {
    let changed = users::set_disabled(&state.app.database, actor.id, user_id, value).await;
    if changed.is_ok() {
        tracing::info!(by = actor.username, user_id, value, "disabled changed");
    }
    let notice = if value {
        Notice::Disabled
    } else {
        Notice::Enabled
    };
    finish(&state, parts, user_id, changed, notice).await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PasswordField {
    password: String,
}

/// `POST /administration/users/{id}/password`: sets a new password, which ends every session of
/// the user. Administrators change their own on the Account page instead, which keeps their
/// session; here it would sign them out.
async fn set_password(
    State(state): State<AdministrationState>,
    Extension(actor): Extension<User>,
    Path(user_id): Path<i64>,
    parts: Parts,
    Form(PasswordField { password }): Form<PasswordField>,
) -> Response {
    if !state.methods.password {
        return not_found(&state, parts).await;
    }
    let changed = if user_id == actor.id {
        Err(Error::Invalid(Problem::OwnAccount))
    } else {
        users::set_password(&state.app.database, user_id, password, &state.policy).await
    };
    if changed.is_ok() {
        tracing::info!(by = actor.username, user_id, "password set");
    }
    finish(&state, parts, user_id, changed, Notice::PasswordSet).await
}

/// Redirects with `notice` after a change, or renders the Users page with why it was refused.
async fn finish(
    state: &AdministrationState,
    parts: Parts,
    user_id: i64,
    changed: Result<(), Error>,
    notice: Notice,
) -> Response {
    let error = match changed {
        Ok(()) => return done(notice, user_id),
        Err(error) => error,
    };
    let (status, problem) = refused(error);
    let username = match users::find_by_id(&state.app.database, user_id).await {
        Ok(user) => user.map(|user| user.username),
        Err(_) => None,
    };
    let refusal = UsersRefusal { problem, username };
    users_view(state, parts, status, Some(refusal), NewUserDraft::default()).await
}

fn page_kind(kind: DeviceKind) -> PageKind {
    match kind {
        DeviceKind::Printer => PageKind::Printer,
        DeviceKind::Scanner => PageKind::Scanner,
    }
}

/// `GET /administration/printers`.
async fn printers_page(State(state): State<AdministrationState>, parts: Parts) -> Response {
    devices_view(&state, parts, DeviceKind::Printer, StatusCode::OK, None).await
}

/// `GET /administration/scanners`.
async fn scanners_page(State(state): State<AdministrationState>, parts: Parts) -> Response {
    devices_view(&state, parts, DeviceKind::Scanner, StatusCode::OK, None).await
}

/// A Printers or Scanners page: the devices their service lists and the stored rules, so a
/// rule stays manageable while the service is down or after its device is gone.
async fn devices_view(
    state: &AdministrationState,
    parts: Parts,
    kind: DeviceKind,
    status: StatusCode,
    refusal: Option<DevicesProblem>,
) -> Response {
    devices_view_with(
        state,
        parts,
        kind,
        status,
        refusal,
        NewPrinterDraft::default(),
    )
    .await
}

/// A device the service lists: name, what tells it apart, and what its row can do.
type ListedDevice = (String, Option<String>, Option<PrinterControls>);

/// How many drivers the Add printer list shows: CUPS knows thousands, so a search narrows them.
const DRIVER_LIMIT: u32 = 50;

/// [`devices_view`], with what the Add printer form held when it was refused.
async fn devices_view_with(
    state: &AdministrationState,
    mut parts: Parts,
    kind: DeviceKind,
    status: StatusCode,
    refusal: Option<DevicesProblem>,
    draft: NewPrinterDraft,
) -> Response {
    let managed = kind == DeviceKind::Printer && state.app.printer_admin.is_some();
    let listed: Result<Vec<ListedDevice>, String> = match kind {
        DeviceKind::Printer => state
            .app
            .print
            .printers()
            .await
            .map_err(|error| error.to_string())
            .map(|printers| {
                printers
                    .into_iter()
                    .map(|printer| {
                        let controls = managed.then(|| PrinterControls {
                            description: printer.description.clone().unwrap_or_default(),
                            location: printer.location.clone().unwrap_or_default(),
                            is_default: printer.is_default,
                            paused: printer.state == PrinterState::Stopped,
                            accepting_jobs: printer.accepting_jobs,
                        });
                        let detail = [
                            printer.description,
                            printer.location,
                            printer.make_and_model,
                        ]
                        .into_iter()
                        .flatten()
                        .filter(|text| !text.trim().is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ");
                        (
                            printer.name,
                            Some(detail).filter(|detail| !detail.is_empty()),
                            controls,
                        )
                    })
                    .collect()
            }),
        DeviceKind::Scanner => state
            .app
            .scan
            .scanners(false)
            .await
            .map_err(|error| error.to_string())
            .map(|scanners| {
                scanners
                    .into_iter()
                    .map(|scanner| {
                        let detail = format!("{} {}", scanner.vendor, scanner.model);
                        (
                            scanner.id,
                            Some(detail.trim().to_owned()).filter(|d| !d.is_empty()),
                            None,
                        )
                    })
                    .collect()
            }),
    };
    if let Err(error) = &listed {
        tracing::warn!(%error, "listing the devices for the administration page failed");
    }
    let listing_failed = listed.is_err();
    let listed = listed.unwrap_or_default();

    let rules = access::rules(&state.app.database, kind).await;
    let groups = groups::list(&state.app.database).await;
    let (devices, groups) = match (rules, groups) {
        (Ok(mut rules), Ok(groups)) => {
            let to_row = |name: String, detail, controls, rule: Option<Access>, listed| DeviceRow {
                name,
                detail,
                controls,
                access: match rule {
                    None | Some(Access::Everyone) => DeviceAccess::Everyone,
                    Some(Access::Groups(ids)) => DeviceAccess::Groups(ids),
                },
                listed,
            };
            let mut rows: Vec<DeviceRow> = listed
                .into_iter()
                .map(|(name, detail, controls)| {
                    let rule = rules.remove(&name);
                    to_row(name, detail, controls, rule, true)
                })
                .collect();
            let mut missing: Vec<_> = rules.into_iter().collect();
            missing.sort_by(|first, second| first.0.cmp(&second.0));
            rows.extend(
                missing
                    .into_iter()
                    .map(|(name, rule)| to_row(name, None, None, Some(rule), false)),
            );
            let choices = groups
                .into_iter()
                .map(|(group, _)| GroupChoice {
                    id: group.id,
                    name: group.name,
                })
                .collect();
            (Some(rows), choices)
        }
        (rules, groups) => {
            if let Err(error) = rules {
                tracing::error!(%error, "loading the device rules failed");
            }
            if let Err(error) = groups {
                tracing::error!(%error, "listing the groups failed");
            }
            (None, vec![])
        }
    };
    let status = if devices.is_none() && status == StatusCode::OK {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        status
    };
    let tools = match (managed, state.app.printer_admin.as_deref()) {
        (true, Some(admin)) => Some(printer_tools(admin, &parts, draft).await),
        _ => None,
    };
    if refusal.is_some() {
        parts.uri = Uri::from_static(page_kind(kind).path());
    }
    let data = DevicesPageData {
        kind: page_kind(kind),
        devices,
        groups,
        listing_failed,
        tools,
        refusal,
    };
    render(&state.app, state.methods, parts, status, move || {
        provide_context(data.clone());
    })
    .await
}

fn devices_done(kind: DeviceKind, notice: DeviceNotice) -> Response {
    Redirect::to(&format!(
        "{}?done={}",
        page_kind(kind).path(),
        notice.code()
    ))
    .into_response()
}

/// The first value of a field of a posted form.
fn field(body: &[u8], name: &str) -> Option<String> {
    url::form_urlencoded::parse(body)
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

/// What an access form asks for, or why it is refused.
async fn requested_access(
    state: &AdministrationState,
    body: &[u8],
) -> Result<(String, Access), (StatusCode, DevicesProblem)> {
    let refused = |problem| (StatusCode::BAD_REQUEST, problem);
    let device = field(body, "device")
        .filter(|device| !device.trim().is_empty())
        .ok_or(refused(DevicesProblem::NoDevice))?;
    let rule = match field(body, "access").as_deref() {
        Some("everyone") => Access::Everyone,
        Some("groups") => {
            // A value that is not a number names no group either.
            let chosen: Vec<Option<i64>> = url::form_urlencoded::parse(body)
                .filter(|(key, _)| key == "groups")
                .map(|(_, value)| value.parse().ok())
                .collect();
            let known = groups::list(&state.app.database).await.map_err(|error| {
                tracing::error!(%error, "listing the groups failed");
                (StatusCode::SERVICE_UNAVAILABLE, DevicesProblem::Unavailable)
            })?;
            let chosen = chosen
                .into_iter()
                .map(|id| id.filter(|id| known.iter().any(|(group, _)| group.id == *id)))
                .collect::<Option<Vec<i64>>>()
                .ok_or(refused(DevicesProblem::UnknownGroup))?;
            Access::Groups(chosen)
        }
        _ => return Err(refused(DevicesProblem::NoAccess)),
    };
    Ok((device, rule))
}

/// `POST /administration/{printers,scanners}/access` with `device`, `access=everyone|groups`
/// and one `groups` field per ticked group (none: administrators only). The fields repeat, so
/// the body is read directly rather than through `Form`.
async fn set_access(
    State(state): State<AdministrationState>,
    Extension(actor): Extension<User>,
    parts: Parts,
    kind: DeviceKind,
    body: Bytes,
) -> Response {
    let saved = match requested_access(&state, &body).await {
        Ok((device, rule)) => access::set(&state.app.database, kind, &device, rule)
            .await
            .map(|()| device)
            .map_err(|error| {
                tracing::error!(%error, "saving the device access failed");
                (StatusCode::SERVICE_UNAVAILABLE, DevicesProblem::Unavailable)
            }),
        Err(refusal) => Err(refusal),
    };
    match saved {
        Ok(device) => {
            tracing::info!(by = actor.username, device, "device access changed");
            devices_done(kind, DeviceNotice::Access)
        }
        Err((status, problem)) => devices_view(&state, parts, kind, status, Some(problem)).await,
    }
}

/// `POST /administration/{printers,scanners}/forget` with `device`: drops the rule of a device.
async fn forget(
    State(state): State<AdministrationState>,
    Extension(actor): Extension<User>,
    parts: Parts,
    kind: DeviceKind,
    body: Bytes,
) -> Response {
    let Some(device) = field(&body, "device").filter(|device| !device.trim().is_empty()) else {
        return devices_view(
            &state,
            parts,
            kind,
            StatusCode::BAD_REQUEST,
            Some(DevicesProblem::NoDevice),
        )
        .await;
    };
    match access::forget(&state.app.database, kind, &device).await {
        Ok(()) => {
            tracing::info!(by = actor.username, device, "device access forgotten");
            devices_done(kind, DeviceNotice::Forgotten)
        }
        Err(error) => {
            tracing::error!(%error, "forgetting the device access failed");
            devices_view(
                &state,
                parts,
                kind,
                StatusCode::SERVICE_UNAVAILABLE,
                Some(DevicesProblem::Unavailable),
            )
            .await
        }
    }
}

/// The Add printer section: the drivers matching `?drivers=`, if it was asked.
async fn printer_tools(
    admin: &dyn PrinterAdmin,
    parts: &Parts,
    draft: NewPrinterDraft,
) -> PrinterTools {
    let search = parts
        .uri
        .query()
        .and_then(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .find(|(key, _)| key == "drivers")
                .map(|(_, value)| value.trim().to_owned())
        })
        .unwrap_or_default();
    let mut tools = PrinterTools {
        draft,
        ..PrinterTools::default()
    };
    if !search.is_empty() {
        match admin.drivers(&search, DRIVER_LIMIT).await {
            Ok(drivers) => {
                tools.drivers = drivers
                    .into_iter()
                    .map(|driver| DriverChoice {
                        name: driver.name,
                        make_and_model: driver.make_and_model,
                    })
                    .collect();
            }
            Err(error) => {
                tracing::warn!(%error, "printer administration failed");
                tools.search_failed = true;
            }
        }
    }
    tools.search = search;
    tools
}

/// The schemes an address may have: what CUPS has a backend for and a person would type.
const DEVICE_SCHEMES: [&str; 8] = [
    "ipp", "ipps", "http", "https", "socket", "lpd", "dnssd", "usb",
];

/// A queue name CUPS accepts: it ends up in a URI and in file names.
fn valid_printer_name(name: &str) -> bool {
    (1..=127).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// An address of a printer: a known scheme and a host, or for `usb` and `dnssd`, which name
/// the device in the host or the path, either.
fn valid_device_uri(address: &str) -> bool {
    if address.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Ok(uri) = url::Url::parse(address) else {
        return false;
    };
    if !DEVICE_SCHEMES.contains(&uri.scheme()) {
        return false;
    }
    // CUPS would store credentials in printers.conf, readable by anyone who can read it.
    if !uri.username().is_empty() || uri.password().is_some() {
        return false;
    }
    let host = uri.host_str().is_some_and(|host| !host.is_empty());
    match uri.scheme() {
        "usb" | "dnssd" => host || !uri.path().trim_matches('/').is_empty(),
        _ => host,
    }
}

/// What a printer form asks of CUPS.
#[derive(Clone, Copy)]
enum PrinterAction {
    Add,
    Describe,
    Default,
    Pause,
    Resume,
    Accept,
    Reject,
    Remove,
}

impl PrinterAction {
    /// Logs what an administrator did to a printer. One call per message keeps each a literal,
    /// as the log reference lists them.
    fn log(self, by: &str, printer: &str) {
        match self {
            PrinterAction::Add => tracing::info!(by, printer, "printer added"),
            PrinterAction::Describe => tracing::info!(by, printer, "printer described"),
            PrinterAction::Default => tracing::info!(by, printer, "default printer set"),
            PrinterAction::Pause => tracing::info!(by, printer, "printer paused"),
            PrinterAction::Resume => tracing::info!(by, printer, "printer resumed"),
            PrinterAction::Accept => tracing::info!(by, printer, "printer accepting jobs"),
            PrinterAction::Reject => tracing::info!(by, printer, "printer rejecting jobs"),
            PrinterAction::Remove => tracing::info!(by, printer, "printer removed"),
        }
    }
}

/// `POST /administration/printers/{add,describe,default,pause,resume,accept,reject,remove}`.
/// Without the CUPS socket they are not there.
fn printer_forms() -> Router<AdministrationState> {
    let mut router = Router::new();
    for (name, action) in [
        ("add", PrinterAction::Add),
        ("describe", PrinterAction::Describe),
        ("default", PrinterAction::Default),
        ("pause", PrinterAction::Pause),
        ("resume", PrinterAction::Resume),
        ("accept", PrinterAction::Accept),
        ("reject", PrinterAction::Reject),
        ("remove", PrinterAction::Remove),
    ] {
        router = router.route(
            &format!("{PRINTERS}/{name}"),
            post(move |state, actor, parts, body| administer(state, actor, parts, action, body)),
        );
    }
    router
}

/// A refused printer form: the status, the problem and what to put back in the Add form.
struct Refusal {
    status: StatusCode,
    problem: DevicesProblem,
    draft: NewPrinterDraft,
}

impl Refusal {
    fn new(status: StatusCode, problem: DevicesProblem) -> Box<Self> {
        Box::new(Self {
            status,
            problem,
            draft: NewPrinterDraft::default(),
        })
    }

    fn from_cups(error: AdminError) -> Box<Self> {
        tracing::warn!(%error, "printer administration failed");
        match error {
            AdminError::Refused => Self::new(StatusCode::BAD_GATEWAY, DevicesProblem::CupsRefused),
            AdminError::Unreachable(message) => Self::new(
                StatusCode::BAD_GATEWAY,
                DevicesProblem::Unreachable(message),
            ),
            AdminError::Rejected(message) => {
                Self::new(StatusCode::BAD_REQUEST, DevicesProblem::Rejected(message))
            }
            AdminError::Unavailable(_) => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                DevicesProblem::CupsUnavailable,
            ),
        }
    }
}

/// Text of a form, trimmed.
fn text_field(body: &[u8], name: &str) -> String {
    field(body, name).unwrap_or_default().trim().to_owned()
}

fn some_text(text: String) -> Option<String> {
    Some(text).filter(|text| !text.is_empty())
}

/// Asks CUPS for what the form says. Returns what to tell and log, and the printer.
async fn perform(
    state: &AdministrationState,
    admin: &dyn PrinterAdmin,
    action: PrinterAction,
    body: &[u8],
) -> Result<(DeviceNotice, String), Box<Refusal>> {
    if let PrinterAction::Add = action {
        let draft = NewPrinterDraft {
            name: text_field(body, "name"),
            address: text_field(body, "address"),
            driver: Some(text_field(body, "driver"))
                .filter(|driver| !driver.is_empty())
                .unwrap_or_else(|| "everywhere".into()),
            description: text_field(body, "description"),
            location: text_field(body, "location"),
        };
        let refuse = |problem, draft: NewPrinterDraft| {
            Box::new(Refusal {
                status: StatusCode::BAD_REQUEST,
                problem,
                draft,
            })
        };
        if !valid_printer_name(&draft.name) {
            return Err(refuse(DevicesProblem::InvalidName, draft));
        }
        if !valid_device_uri(&draft.address) {
            return Err(refuse(DevicesProblem::InvalidAddress, draft));
        }
        // CUPS-Add-Modify-Printer would silently replace a queue of that name, whatever its
        // case, with its jobs and settings.
        match state.app.print.printers().await {
            Ok(printers) => {
                if printers
                    .iter()
                    .any(|printer| printer.name.eq_ignore_ascii_case(&draft.name))
                {
                    return Err(Box::new(Refusal {
                        status: StatusCode::CONFLICT,
                        problem: DevicesProblem::NameTaken,
                        draft,
                    }));
                }
            }
            Err(error) => {
                tracing::warn!(%error, "the printing service failed");
                return Err(Box::new(Refusal {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    problem: DevicesProblem::CupsUnavailable,
                    draft,
                }));
            }
        }
        let printer = NewPrinter {
            name: draft.name.clone(),
            device_uri: draft.address.clone(),
            driver: if draft.driver == "everywhere" {
                Driver::Everywhere
            } else {
                Driver::Ppd(draft.driver.clone())
            },
            description: some_text(draft.description.clone()),
            location: some_text(draft.location.clone()),
        };
        return match admin.add(printer).await {
            Ok(()) => Ok((DeviceNotice::Added, draft.name)),
            Err(error) => Err(Box::new(Refusal {
                draft,
                ..*Refusal::from_cups(error)
            })),
        };
    }

    let printer = text_field(body, "printer");
    if printer.is_empty() {
        return Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            DevicesProblem::NoDevice,
        ));
    }
    if !valid_printer_name(&printer) {
        return Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            DevicesProblem::InvalidName,
        ));
    }
    let name = printer.as_str();
    let done = match action {
        PrinterAction::Add => unreachable!("handled above"),
        PrinterAction::Describe => admin
            .describe(
                name,
                Some(text_field(body, "description")),
                Some(text_field(body, "location")),
            )
            .await
            .map(|()| DeviceNotice::Described),
        PrinterAction::Default => admin
            .set_default(name)
            .await
            .map(|()| DeviceNotice::Default),
        PrinterAction::Pause => admin.pause(name).await.map(|()| DeviceNotice::Paused),
        PrinterAction::Resume => admin.resume(name).await.map(|()| DeviceNotice::Resumed),
        PrinterAction::Accept => admin.accept(name).await.map(|()| DeviceNotice::Accepting),
        PrinterAction::Reject => admin.reject(name).await.map(|()| DeviceNotice::Rejecting),
        PrinterAction::Remove => {
            if field(body, "confirm").as_deref() != Some("yes") {
                return Err(Refusal::new(
                    StatusCode::BAD_REQUEST,
                    DevicesProblem::NeedsConfirmation,
                ));
            }
            let removed = admin.delete(name).await;
            if removed.is_ok() {
                // An explicit removal is the administrator deleting the settings too. A rule
                // that stays shows as a missing printer, which can be forgotten by hand.
                if let Err(error) =
                    access::forget(&state.app.database, DeviceKind::Printer, name).await
                {
                    tracing::error!(%error, "forgetting the device access failed");
                }
            }
            removed.map(|()| DeviceNotice::Removed)
        }
    };
    done.map(|notice| (notice, printer))
        .map_err(Refusal::from_cups)
}

async fn administer(
    State(state): State<AdministrationState>,
    Extension(actor): Extension<User>,
    parts: Parts,
    action: PrinterAction,
    body: Bytes,
) -> Response {
    let Some(admin) = state.app.printer_admin.clone() else {
        return not_found(&state, parts).await;
    };
    match perform(&state, &*admin, action, &body).await {
        Ok((notice, printer)) => {
            action.log(&actor.username, &printer);
            devices_done(DeviceKind::Printer, notice)
        }
        Err(refusal) => {
            let refusal = *refusal;
            devices_view_with(
                &state,
                parts,
                DeviceKind::Printer,
                refusal.status,
                Some(refusal.problem),
                refusal.draft,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_plain_and_bounded() {
        for name in ["office", "Lab_2.floor-1", &"a".repeat(127)] {
            assert!(valid_printer_name(name), "{name}");
        }
        for name in ["", "my printer", "a/b", "a#b", "é", &"a".repeat(128)] {
            assert!(!valid_printer_name(name), "{name}");
        }
    }

    #[test]
    fn addresses_need_a_known_scheme_and_a_place() {
        for address in [
            "ipp://192.168.1.20/ipp/print",
            "ipps://printer.local:631/ipp/print",
            "http://10.0.0.5:631/printers/x",
            "socket://192.168.1.21:9100",
            "lpd://192.168.1.22/queue",
            "usb://HP/LaserJet%201020?serial=123",
            "dnssd://Office._ipp._tcp.local/?uuid=1",
        ] {
            assert!(valid_device_uri(address), "{address}");
        }
        for address in [
            "",
            "192.168.1.20",
            "file:///etc/passwd",
            "cups-pdf:/",
            "ipp:///ipp/print",
            "ipp://host/a b",
            "ipp://host/\nx",
            "usb:",
            "ipp://user:pass@host/ipp/print",
            "ipp://user@host/ipp/print",
            "ipp://:pass@host/ipp/print",
        ] {
            assert!(!valid_device_uri(address), "{address:?}");
        }
    }
}
