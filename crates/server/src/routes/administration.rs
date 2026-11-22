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
                PRINTER_TEXT_MAX, Price, PriceRow, PrinterControls, PrinterTools,
            },
            groups::{GroupMember, GroupRow, GroupsPageData},
            quotas::{
                Choice, QuotasNotice, QuotasPageData, QuotasProblem, RuleDraft, RuleRow,
                SubjectChoice,
            },
            users::{NewUserDraft, Notice, UserRow, UsersPageData, UsersProblem, UsersRefusal},
        },
        sign_in::SignInMethods,
    },
    quotas::Allowance,
    state::{AppState, Currency, User},
};
use recto_domain::{
    Error, Problem,
    access::{self, Access, DeviceKind},
    groups,
    passwords::PasswordPolicy,
    prices::{self, Prices},
    quotas::{self, Enforcement, Holder, Metric, NewRule, Period, Rule, Subject},
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
const QUOTAS: &str = "/administration/quotas";

#[derive(Clone)]
pub(super) struct AdministrationState {
    pub(super) app: AppState,
    pub(super) methods: SignInMethods,
    policy: PasswordPolicy,
}

/// `GET /administration/users`, `/groups`, `/printers`, `/scanners`, `/quotas` and `/usage`,
/// `/administration` leading to the first, and the forms of the Users, Printers, Scanners and
/// Quotas pages.
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
        .route(
            "/administration/users/{id}/grant",
            post(
                |State(state): State<AdministrationState>,
                 Extension(actor): Extension<User>,
                 Path(id): Path<i64>,
                 parts: Parts,
                 Form(fields): Form<AdjustFields>| {
                    adjust_user(state, actor, id, parts, fields, true)
                },
            ),
        )
        .route(
            "/administration/users/{id}/reset",
            post(
                |State(state): State<AdministrationState>,
                 Extension(actor): Extension<User>,
                 Path(id): Path<i64>,
                 parts: Parts,
                 Form(fields): Form<AdjustFields>| {
                    adjust_user(state, actor, id, parts, fields, false)
                },
            ),
        )
        .route(
            "/administration/quotas/{id}/grant",
            post(
                |State(state): State<AdministrationState>,
                 Extension(actor): Extension<User>,
                 Path(id): Path<i64>,
                 parts: Parts,
                 Form(fields): Form<AdjustFields>| {
                    adjust_pool(state, actor, id, parts, fields, true)
                },
            ),
        )
        .route(
            "/administration/quotas/{id}/reset",
            post(
                |State(state): State<AdministrationState>,
                 Extension(actor): Extension<User>,
                 Path(id): Path<i64>,
                 parts: Parts,
                 Form(fields): Form<AdjustFields>| {
                    adjust_pool(state, actor, id, parts, fields, false)
                },
            ),
        )
        .route("/administration/groups", get(groups_page))
        .route(PRINTERS, get(printers_page))
        .route(SCANNERS, get(scanners_page))
        .merge(printer_forms())
        .route(&format!("{PRINTERS}/prices"), post(set_prices))
        .merge(device_forms(DeviceKind::Printer))
        .merge(device_forms(DeviceKind::Scanner))
        .route(QUOTAS, get(quotas_page).post(create_rule))
        .route("/administration/quotas/{id}", post(update_rule))
        .route(
            "/administration/quotas/{id}/enabled",
            post(set_rule_enabled),
        )
        .route("/administration/quotas/{id}/delete", post(delete_rule))
        .merge(super::usage::router())
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
        Ok(users) => {
            let mut rows: Vec<UserRow> = users.into_iter().map(user_row).collect();
            if let Err(error) = add_usage(state, &mut rows).await {
                // The users still show, without their usage.
                tracing::error!(%error, "reading the quota usage failed");
            }
            Some(rows)
        }
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
        currency: state.app.currency.clone(),
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
        quotas: Vec::new(),
    }
}

/// Fills in what each user used of the enabled personal rules that apply to them on any
/// printer. Pools are the group's, so the Quotas page shows them.
async fn add_usage(state: &AdministrationState, rows: &mut [UserRow]) -> Result<(), Error> {
    let database = &state.app.database;
    let rules = quotas::rules(database).await?;
    if !rules.iter().any(|rule| rule.enabled) {
        return Ok(());
    }
    // A name no printer has picks the rules for every printer; each printer a rule names picks
    // its own.
    let mut printers = vec![String::new()];
    printers.extend(rules.iter().flat_map(|rule| rule.printers.iter().cloned()));
    let now = chrono::Utc::now();
    for row in rows.iter_mut() {
        let groups = access::group_ids_of(database, row.id).await?;
        let mut applicable = std::collections::BTreeMap::new();
        for printer in &printers {
            for rule in quotas::applicable(&rules, row.id, &groups, printer) {
                if !matches!(rule.subject, Subject::Pool(_)) {
                    applicable.insert(rule.id, rule);
                }
            }
        }
        for rule in applicable.values() {
            let standing =
                quotas::standing(database, rule, row.id, now, state.app.timezone).await?;
            row.quotas.push(Allowance::from(&standing));
        }
    }
    Ok(())
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
        // The user forms never meet the problems of quota rules or device passwords.
        Error::Invalid(
            problem @ (Problem::RuleNameEmpty
            | Problem::RuleLimitNegative
            | Problem::RuleLimitTooLarge { .. }
            | Problem::RuleAdjusted
            | Problem::RollingDaysInvalid { .. }
            | Problem::SubjectNotFound
            | Problem::PrinterNameEmpty
            | Problem::GrantNotPositive
            | Problem::GrantTooLarge { .. }
            | Problem::HolderMismatch
            | Problem::DeviceNameInvalid { .. }
            | Problem::TooManyDevicePasswords { .. }),
        ) => {
            tracing::error!(%problem, "changing a user failed");
            return (StatusCode::SERVICE_UNAVAILABLE, UsersProblem::Unavailable);
        }
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
        None,
    )
    .await
}

/// What a refused Prices form held: the printer, then the mono page, color page and sheet
/// amounts as typed.
type TypedPrices = (String, [String; 3]);

/// A printer's prices for its row, or the typed ones of a refused form.
fn price_row(currency: &Currency, prices: Prices, typed: Option<&[String; 3]>) -> PriceRow {
    let price = |minor: i64, index: usize| Price {
        value: match typed {
            Some(typed) => typed[index].clone(),
            None if minor == 0 => String::new(),
            None => currency.amount(minor),
        },
        shown: currency.format(minor),
        free: minor == 0,
    };
    PriceRow {
        mono_page: price(prices.mono_page, 0),
        color_page: price(prices.color_page, 1),
        sheet: price(prices.sheet, 2),
    }
}

/// A device the service lists: name, what tells it apart, and what its row can do.
type ListedDevice = (String, Option<String>, Option<PrinterControls>);

/// How many drivers the Add printer list shows: CUPS knows thousands, so a search narrows them.
const DRIVER_LIMIT: u32 = 50;

/// [`devices_view`], with what the Add printer form or a Prices form held when it was refused.
async fn devices_view_with(
    state: &AdministrationState,
    mut parts: Parts,
    kind: DeviceKind,
    status: StatusCode,
    refusal: Option<DevicesProblem>,
    draft: NewPrinterDraft,
    typed_prices: Option<TypedPrices>,
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
    // Scanners have no prices.
    let priced = match kind {
        DeviceKind::Printer => prices::all(&state.app.database).await,
        DeviceKind::Scanner => Ok(Default::default()),
    };
    let (devices, groups) = match (rules, groups, priced) {
        (Ok(mut rules), Ok(groups), Ok(mut priced)) => {
            // The settings of a device the service no longer lists stay manageable.
            let mut missing: Vec<String> = rules
                .keys()
                .chain(priced.keys())
                .filter(|name| !listed.iter().any(|(listed, _, _)| listed == *name))
                .cloned()
                .collect();
            missing.sort();
            missing.dedup();
            let currency = &state.app.currency;
            let mut to_row = |name: String, detail, controls, listed| {
                let rule = rules.remove(&name);
                let prices = (kind == DeviceKind::Printer).then(|| {
                    let typed = typed_prices
                        .as_ref()
                        .filter(|(printer, _)| *printer == name)
                        .map(|(_, typed)| typed);
                    let prices = priced.remove(&name).unwrap_or_default();
                    price_row(currency, prices, typed)
                });
                DeviceRow {
                    name,
                    detail,
                    controls,
                    access: match rule {
                        None | Some(Access::Everyone) => DeviceAccess::Everyone,
                        Some(Access::Groups(ids)) => DeviceAccess::Groups(ids),
                    },
                    listed,
                    prices,
                }
            };
            let mut rows: Vec<DeviceRow> = listed
                .into_iter()
                .map(|(name, detail, controls)| to_row(name, detail, controls, true))
                .collect();
            rows.extend(
                missing
                    .into_iter()
                    .map(|name| to_row(name, None, None, false)),
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
        (rules, groups, priced) => {
            if let Err(error) = rules {
                tracing::error!(%error, "loading the device rules failed");
            }
            if let Err(error) = groups {
                tracing::error!(%error, "listing the groups failed");
            }
            if let Err(error) = priced {
                tracing::error!(%error, "loading the printer prices failed");
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
        currency: state.app.currency.code.clone(),
        price_example: state.app.currency.amount(5),
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
    let mut forgotten = access::forget(&state.app.database, kind, &device).await;
    if forgotten.is_ok() && kind == DeviceKind::Printer {
        forgotten = prices::set(&state.app.database, &device, Prices::default()).await;
    }
    match forgotten {
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

/// A name for a new printer: plain, since it ends up in URIs, file names and commands.
fn valid_printer_name(name: &str) -> bool {
    (1..=127).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// The name of an existing printer, by CUPS's own rule, so that a queue made with `lpadmin`,
/// such as `Büro` or `HP+`, can be managed too.
fn valid_queue_name(name: &str) -> bool {
    (1..=127).contains(&name.len())
        && !name
            .chars()
            .any(|c| c.is_control() || c == ' ' || matches!(c, '/' | '\\' | '?' | '\'' | '"' | '#'))
}

/// A description or location CUPS keeps: one line, no control characters, a bounded length.
fn valid_printer_text(text: &str) -> bool {
    text.chars().count() <= PRINTER_TEXT_MAX && !text.chars().any(char::is_control)
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
    /// Logs what an administrator did to a printer, with the address of a new one. One call
    /// per message keeps each a literal, as the log reference lists them.
    fn log(self, by: &str, printer: &str, address: Option<&str>) {
        match self {
            PrinterAction::Add => {
                let address = address.unwrap_or_default();
                tracing::info!(by, printer, address, "printer added");
            }
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

    fn from_cups(error: AdminError, by: &str, printer: &str) -> Box<Self> {
        tracing::warn!(%error, by, printer, "printer administration failed");
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

/// Asks CUPS for what the form says, on behalf of `by`, and logs it. Returns what to tell.
async fn perform(
    state: &AdministrationState,
    admin: &dyn PrinterAdmin,
    action: PrinterAction,
    by: &str,
    body: &[u8],
) -> Result<DeviceNotice, Box<Refusal>> {
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
        if !valid_printer_text(&draft.description) || !valid_printer_text(&draft.location) {
            return Err(refuse(DevicesProblem::InvalidText, draft));
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
            Ok(()) => {
                action.log(by, &draft.name, Some(&draft.address));
                Ok(DeviceNotice::Added)
            }
            Err(error) => {
                let refusal = Refusal::from_cups(error, by, &draft.name);
                Err(Box::new(Refusal { draft, ..*refusal }))
            }
        };
    }

    let printer = text_field(body, "printer");
    if printer.is_empty() {
        return Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            DevicesProblem::NoDevice,
        ));
    }
    if !valid_queue_name(&printer) {
        return Err(Refusal::new(
            StatusCode::BAD_REQUEST,
            DevicesProblem::InvalidQueueName,
        ));
    }
    let name = printer.as_str();
    let done = match action {
        PrinterAction::Add => unreachable!("handled above"),
        PrinterAction::Describe => {
            let description = text_field(body, "description");
            let location = text_field(body, "location");
            if !valid_printer_text(&description) || !valid_printer_text(&location) {
                return Err(Refusal::new(
                    StatusCode::BAD_REQUEST,
                    DevicesProblem::InvalidText,
                ));
            }
            admin
                .describe(name, Some(description), Some(location))
                .await
                .map(|()| DeviceNotice::Described)
        }
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
                if let Err(error) = prices::set(&state.app.database, name, Prices::default()).await
                {
                    tracing::error!(%error, "forgetting the printer prices failed");
                }
            }
            removed.map(|()| DeviceNotice::Removed)
        }
    };
    match done {
        Ok(notice) => {
            action.log(by, name, None);
            Ok(notice)
        }
        Err(error) => Err(Refusal::from_cups(error, by, name)),
    }
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
    match perform(&state, &*admin, action, &actor.username, &body).await {
        Ok(notice) => devices_done(DeviceKind::Printer, notice),
        Err(refusal) => {
            let refusal = *refusal;
            devices_view_with(
                &state,
                parts,
                DeviceKind::Printer,
                refusal.status,
                Some(refusal.problem),
                refusal.draft,
                None,
            )
            .await
        }
    }
}

/// `POST /administration/printers/prices` with `printer`, `mono_page`, `color_page` and
/// `sheet`, amounts typed in the currency (empty is 0). Works without printer management: the
/// prices are recto's, not CUPS's.
async fn set_prices(
    State(state): State<AdministrationState>,
    Extension(actor): Extension<User>,
    parts: Parts,
    body: Bytes,
) -> Response {
    let printer = text_field(&body, "printer");
    let typed = ["mono_page", "color_page", "sheet"].map(|name| text_field(&body, name));
    let saved = match requested_prices(&state.app.currency, &printer, &typed) {
        Ok(prices) => prices::set(&state.app.database, &printer, prices)
            .await
            .map(|()| prices)
            .map_err(|error| {
                tracing::error!(%error, "saving the printer prices failed");
                (StatusCode::SERVICE_UNAVAILABLE, DevicesProblem::Unavailable)
            }),
        Err(refusal) => Err(refusal),
    };
    match saved {
        Ok(prices) => {
            tracing::info!(
                by = actor.username,
                printer,
                mono_page = prices.mono_page,
                color_page = prices.color_page,
                sheet = prices.sheet,
                "printer prices set"
            );
            devices_done(DeviceKind::Printer, DeviceNotice::Prices)
        }
        Err((status, problem)) => {
            devices_view_with(
                &state,
                parts,
                DeviceKind::Printer,
                status,
                Some(problem),
                NewPrinterDraft::default(),
                Some((printer, typed)),
            )
            .await
        }
    }
}

/// The prices a Prices form asks for, or why it is refused.
fn requested_prices(
    currency: &Currency,
    printer: &str,
    typed: &[String; 3],
) -> Result<Prices, (StatusCode, DevicesProblem)> {
    if printer.is_empty() {
        return Err((StatusCode::BAD_REQUEST, DevicesProblem::NoDevice));
    }
    let amount = |text: &str| {
        if text.is_empty() {
            Some(0)
        } else {
            currency.parse_amount(text)
        }
    };
    match (amount(&typed[0]), amount(&typed[1]), amount(&typed[2])) {
        (Some(mono_page), Some(color_page), Some(sheet)) => Ok(Prices {
            mono_page,
            color_page,
            sheet,
        }),
        _ => Err((
            StatusCode::BAD_REQUEST,
            DevicesProblem::InvalidPrices {
                currency: currency.code.clone(),
                decimals: currency.decimals,
            },
        )),
    }
}

/// `GET /administration/quotas`.
async fn quotas_page(State(state): State<AdministrationState>, parts: Parts) -> Response {
    quotas_view(&state, parts, StatusCode::OK, None, None).await
}

/// The Quotas page, with the refusal of a form and what it held, if any.
async fn quotas_view(
    state: &AdministrationState,
    mut parts: Parts,
    status: StatusCode,
    refusal: Option<QuotasProblem>,
    draft: Option<(Option<i64>, RuleDraft)>,
) -> Response {
    let database = &state.app.database;
    let currency = state.app.currency.clone();
    let loaded = async {
        let rules = quotas::rules(database).await?;
        let groups = groups::list(database).await?;
        let users = users::list(database).await?;
        let now = chrono::Utc::now();
        let mut rows = Vec::with_capacity(rules.len());
        for rule in rules {
            // A pool's standing does not depend on whom it is read for.
            let pool = match rule.subject {
                Subject::Pool(_) => Some(Allowance::from(
                    &quotas::standing(database, &rule, 0, now, state.app.timezone).await?,
                )),
                _ => None,
            };
            rows.push(rule_row(rule, pool));
        }
        let groups: Vec<Choice> = groups
            .into_iter()
            .map(|(group, _)| Choice {
                id: group.id,
                name: group.name,
            })
            .collect();
        let users: Vec<Choice> = users
            .into_iter()
            .map(|summary| Choice {
                id: summary.user.id,
                name: summary.user.username,
            })
            .collect();
        Ok::<_, Error>((rows, groups, users))
    }
    .await;
    let (rules, groups, users) = match loaded {
        Ok((rules, groups, users)) => (Some(rules), groups, users),
        Err(error) => {
            tracing::error!(%error, "loading the quota rules failed");
            (None, Vec::new(), Vec::new())
        }
    };
    let status = if rules.is_none() && status == StatusCode::OK {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        status
    };
    // The printers to choose from: CUPS's, and those rules already name.
    let mut printers: Vec<String> = match state.app.print.printers().await {
        Ok(printers) => printers.into_iter().map(|printer| printer.name).collect(),
        Err(error) => {
            tracing::warn!(%error, "listing the printers for the quotas page failed");
            Vec::new()
        }
    };
    for rule in rules.iter().flatten() {
        for printer in &rule.printers {
            if !printers
                .iter()
                .any(|known| known.eq_ignore_ascii_case(printer))
            {
                printers.push(printer.clone());
            }
        }
    }
    printers.sort_by_key(|printer| printer.to_lowercase());
    if refusal.is_some() {
        parts.uri = Uri::from_static(QUOTAS);
    }
    let data = QuotasPageData {
        rules,
        groups,
        users,
        printers,
        currency,
        refusal,
        draft,
    };
    render(&state.app, state.methods, parts, status, move || {
        provide_context(data.clone());
    })
    .await
}

fn rule_row(rule: Rule, pool: Option<Allowance>) -> RuleRow {
    RuleRow {
        id: rule.id,
        name: rule.name,
        enabled: rule.enabled,
        subject: match rule.subject {
            Subject::Everyone => SubjectChoice::Everyone,
            Subject::Group(id) => SubjectChoice::Group(id),
            Subject::User(id) => SubjectChoice::User(id),
            Subject::Pool(id) => SubjectChoice::Pool(id),
        },
        printers: rule.printers,
        metric: rule.metric.into(),
        period: rule.period.into(),
        limit: rule.limit,
        hard: rule.enforcement == Enforcement::Hard,
        pool,
    }
}

fn quotas_done(notice: QuotasNotice) -> Response {
    Redirect::to(&format!("{QUOTAS}?done={}", notice.code())).into_response()
}

/// The status and the problem a refused quota form is shown with.
fn quota_refused(error: Error, what: &str) -> (StatusCode, QuotasProblem) {
    let problem = match error {
        Error::Invalid(Problem::RuleNameEmpty) => QuotasProblem::NameEmpty,
        Error::Invalid(Problem::RuleLimitNegative) => QuotasProblem::LimitNegative,
        Error::Invalid(Problem::RuleLimitTooLarge { maximum }) => {
            QuotasProblem::LimitTooLarge { maximum }
        }
        Error::Invalid(Problem::RuleAdjusted) => QuotasProblem::RuleAdjusted,
        Error::Invalid(Problem::RollingDaysInvalid { maximum }) => {
            QuotasProblem::RollingDaysInvalid { maximum }
        }
        Error::Invalid(Problem::SubjectNotFound) => QuotasProblem::SubjectNotFound,
        Error::Invalid(Problem::PrinterNameEmpty | Problem::HolderMismatch) => {
            QuotasProblem::InvalidForm
        }
        Error::Invalid(Problem::GrantNotPositive) => QuotasProblem::AmountInvalid,
        Error::Invalid(Problem::GrantTooLarge { maximum }) => {
            QuotasProblem::AmountTooLarge { maximum }
        }
        Error::NotFound => return (StatusCode::NOT_FOUND, QuotasProblem::RuleNotFound),
        error => {
            tracing::error!(%error, "{what} failed");
            return (StatusCode::SERVICE_UNAVAILABLE, QuotasProblem::Unavailable);
        }
    };
    (StatusCode::BAD_REQUEST, problem)
}

/// What a rule form holds, as typed.
fn rule_draft(body: &[u8]) -> RuleDraft {
    RuleDraft {
        name: field(body, "name").unwrap_or_default(),
        // A checkbox: present when ticked.
        enabled: field(body, "enabled").is_some(),
        subject: text_field(body, "subject"),
        printers: url::form_urlencoded::parse(body)
            .filter(|(key, _)| key == "printers")
            .map(|(_, value)| value.into_owned())
            .collect(),
        metric: text_field(body, "metric"),
        period: text_field(body, "period"),
        rolling_days: text_field(body, "rolling_days"),
        limit: text_field(body, "limit"),
        enforcement: text_field(body, "enforcement"),
    }
}

/// The rule a form asks for, or why it is refused. The domain checks the rest.
fn requested_rule(draft: &RuleDraft, currency: &Currency) -> Result<NewRule, QuotasProblem> {
    let subject = match SubjectChoice::parse(&draft.subject).ok_or(QuotasProblem::InvalidForm)? {
        SubjectChoice::Everyone => Subject::Everyone,
        SubjectChoice::Group(id) => Subject::Group(id),
        SubjectChoice::User(id) => Subject::User(id),
        SubjectChoice::Pool(id) => Subject::Pool(id),
    };
    let metric = Metric::from_keyword(&draft.metric).ok_or(QuotasProblem::InvalidForm)?;
    let rolling_days = match draft.period.as_str() {
        "rolling" => Some(
            draft
                .rolling_days
                .parse::<u16>()
                .ok()
                .filter(|days| (1..=quotas::MAX_ROLLING_DAYS).contains(days))
                .ok_or(QuotasProblem::RollingDaysInvalid {
                    maximum: quotas::MAX_ROLLING_DAYS,
                })?,
        ),
        _ => None,
    };
    let period =
        Period::from_keyword(&draft.period, rolling_days).ok_or(QuotasProblem::InvalidForm)?;
    let enforcement =
        Enforcement::from_keyword(&draft.enforcement).ok_or(QuotasProblem::InvalidForm)?;
    let limit = match metric {
        Metric::Cost => currency.parse_amount(&draft.limit),
        // `-5` reads, so the domain can say it is negative.
        _ => draft.limit.parse::<i64>().ok(),
    };
    let limit = match limit {
        Some(limit) => limit,
        None if draft.limit.starts_with('-') => return Err(QuotasProblem::LimitNegative),
        None => return Err(QuotasProblem::LimitInvalid),
    };
    Ok(NewRule {
        name: draft.name.clone(),
        enabled: draft.enabled,
        subject,
        printers: draft.printers.clone(),
        metric,
        period,
        limit,
        enforcement,
    })
}

/// `POST /administration/quotas`: creates a rule.
async fn create_rule(
    State(state): State<AdministrationState>,
    Extension(actor): Extension<User>,
    parts: Parts,
    body: Bytes,
) -> Response {
    let draft = rule_draft(&body);
    let created = match requested_rule(&draft, &state.app.currency) {
        Ok(rule) => quotas::create(&state.app.database, rule)
            .await
            .map_err(|error| quota_refused(error, "creating a quota rule")),
        Err(problem) => Err((StatusCode::BAD_REQUEST, problem)),
    };
    match created {
        Ok(rule) => {
            tracing::info!(by = actor.username, rule, "quota rule created");
            quotas_done(QuotasNotice::Created)
        }
        Err((status, problem)) => {
            quotas_view(&state, parts, status, Some(problem), Some((None, draft))).await
        }
    }
}

/// `POST /administration/quotas/{id}`: changes every field of a rule.
async fn update_rule(
    State(state): State<AdministrationState>,
    Extension(actor): Extension<User>,
    Path(rule): Path<i64>,
    parts: Parts,
    body: Bytes,
) -> Response {
    let draft = rule_draft(&body);
    let updated = match requested_rule(&draft, &state.app.currency) {
        Ok(new) => quotas::update(&state.app.database, rule, new)
            .await
            .map_err(|error| quota_refused(error, "changing a quota rule")),
        Err(problem) => Err((StatusCode::BAD_REQUEST, problem)),
    };
    match updated {
        Ok(()) => {
            tracing::info!(by = actor.username, rule, "quota rule changed");
            quotas_done(QuotasNotice::Saved)
        }
        Err((status, problem)) => {
            quotas_view(
                &state,
                parts,
                status,
                Some(problem),
                Some((Some(rule), draft)),
            )
            .await
        }
    }
}

/// `POST /administration/quotas/{id}/enabled` with `enabled=true|false`.
async fn set_rule_enabled(
    State(state): State<AdministrationState>,
    Extension(actor): Extension<User>,
    Path(rule): Path<i64>,
    parts: Parts,
    body: Bytes,
) -> Response {
    let Some(enabled) = field(&body, "enabled").and_then(|value| value.parse::<bool>().ok()) else {
        return quotas_view(
            &state,
            parts,
            StatusCode::BAD_REQUEST,
            Some(QuotasProblem::InvalidForm),
            None,
        )
        .await;
    };
    let database = &state.app.database;
    let changed = async {
        let current = quotas::rule(database, rule).await?;
        let new = NewRule {
            name: current.name,
            enabled,
            subject: current.subject,
            printers: current.printers,
            metric: current.metric,
            period: current.period,
            limit: current.limit,
            enforcement: current.enforcement,
        };
        quotas::update(database, rule, new).await
    }
    .await;
    match changed {
        Ok(()) => {
            tracing::info!(
                by = actor.username,
                rule,
                enabled,
                "quota rule enabled changed"
            );
            quotas_done(if enabled {
                QuotasNotice::Enabled
            } else {
                QuotasNotice::Disabled
            })
        }
        Err(error) => {
            let (status, problem) = quota_refused(error, "changing a quota rule");
            quotas_view(&state, parts, status, Some(problem), None).await
        }
    }
}

/// `POST /administration/quotas/{id}/delete` with `confirm=yes`: deletes a rule with its grants
/// and resets.
async fn delete_rule(
    State(state): State<AdministrationState>,
    Extension(actor): Extension<User>,
    Path(rule): Path<i64>,
    parts: Parts,
    body: Bytes,
) -> Response {
    let deleted = if field(&body, "confirm").as_deref() == Some("yes") {
        quotas::delete(&state.app.database, rule)
            .await
            .map_err(|error| quota_refused(error, "deleting a quota rule"))
    } else {
        Err((StatusCode::BAD_REQUEST, QuotasProblem::ConfirmMissing))
    };
    match deleted {
        Ok(()) => {
            tracing::info!(by = actor.username, rule, "quota rule deleted");
            quotas_done(QuotasNotice::Deleted)
        }
        Err((status, problem)) => quotas_view(&state, parts, status, Some(problem), None).await,
    }
}

/// The longest reason an adjustment keeps, in characters.
const MAX_REASON: usize = 200;

/// Grants extra (`amount` typed in the rule's unit) or resets (`None`) a rule for `holder`, and
/// logs it. Returns the amount granted in the rule's unit.
async fn adjust(
    state: &AdministrationState,
    actor: &User,
    rule_id: i64,
    holder: Holder,
    amount: Option<String>,
    reason: &str,
) -> Result<(), (StatusCode, QuotasProblem)> {
    let reason = reason.trim();
    if reason.is_empty() || reason.chars().count() > MAX_REASON {
        return Err((StatusCode::BAD_REQUEST, QuotasProblem::ReasonInvalid));
    }
    let database = &state.app.database;
    let rule = quotas::rule(database, rule_id)
        .await
        .map_err(|error| quota_refused(error, "adjusting a quota"))?;
    let now = chrono::Utc::now();
    let (user, group) = match holder {
        Holder::User(id) => (Some(id), None),
        Holder::Group(id) => (None, Some(id)),
    };
    match amount {
        Some(text) => {
            let amount = match rule.metric {
                Metric::Cost => state.app.currency.parse_amount(&text),
                _ => text.trim().parse::<i64>().ok(),
            }
            .filter(|amount| *amount > 0)
            .ok_or((StatusCode::BAD_REQUEST, QuotasProblem::AmountInvalid))?;
            quotas::grant(
                database,
                rule_id,
                holder,
                amount,
                reason,
                actor.id,
                now,
                state.app.timezone,
            )
            .await
            .map_err(|error| quota_refused(error, "granting extra"))?;
            tracing::info!(
                by = actor.username,
                rule = rule_id,
                user,
                group,
                amount,
                "quota granted"
            );
        }
        None => {
            quotas::reset(database, rule_id, holder, reason, actor.id, now)
                .await
                .map_err(|error| quota_refused(error, "resetting a quota"))?;
            tracing::info!(
                by = actor.username,
                rule = rule_id,
                user,
                group,
                "quota reset"
            );
        }
    }
    Ok(())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AdjustFields {
    rule: String,
    amount: Option<String>,
    reason: String,
}

/// `POST /administration/users/{id}/grant` with `rule`, `amount` and `reason`, and
/// `/administration/users/{id}/reset` with `rule` and `reason`.
async fn adjust_user(
    state: AdministrationState,
    actor: User,
    user_id: i64,
    parts: Parts,
    fields: AdjustFields,
    grant: bool,
) -> Response {
    let adjusted = match fields.rule.trim().parse::<i64>() {
        Ok(rule) => {
            let amount = if grant {
                Some(fields.amount.unwrap_or_default())
            } else {
                None
            };
            adjust(
                &state,
                &actor,
                rule,
                Holder::User(user_id),
                amount,
                &fields.reason,
            )
            .await
        }
        Err(_) => Err((StatusCode::BAD_REQUEST, QuotasProblem::InvalidForm)),
    };
    match adjusted {
        Ok(()) => done(
            if grant {
                Notice::QuotaGranted
            } else {
                Notice::QuotaReset
            },
            user_id,
        ),
        Err((status, problem)) => {
            // The user being gone is what the form meets, more than a mismatch.
            let username = match users::find_by_id(&state.app.database, user_id).await {
                Ok(user) => user.map(|user| user.username),
                Err(_) => None,
            };
            let problem = match (problem, &username) {
                (QuotasProblem::SubjectNotFound, None) => UsersProblem::UserNotFound,
                (problem, _) => UsersProblem::Quota(problem),
            };
            let refusal = UsersRefusal { problem, username };
            users_view(
                &state,
                parts,
                status,
                Some(refusal),
                NewUserDraft::default(),
            )
            .await
        }
    }
}

/// `POST /administration/quotas/{id}/grant` with `amount` and `reason`, and `/reset` with
/// `reason`: for the group of a pool rule.
async fn adjust_pool(
    state: AdministrationState,
    actor: User,
    rule: i64,
    parts: Parts,
    fields: AdjustFields,
    grant: bool,
) -> Response {
    let adjusted = match quotas::rule(&state.app.database, rule).await {
        Ok(found) => match found.subject {
            Subject::Pool(group) => {
                let amount = if grant {
                    Some(fields.amount.unwrap_or_default())
                } else {
                    None
                };
                adjust(
                    &state,
                    &actor,
                    rule,
                    Holder::Group(group),
                    amount,
                    &fields.reason,
                )
                .await
            }
            _ => Err((StatusCode::BAD_REQUEST, QuotasProblem::InvalidForm)),
        },
        Err(error) => Err(quota_refused(error, "adjusting a quota")),
    };
    match adjusted {
        Ok(()) => quotas_done(if grant {
            QuotasNotice::Granted
        } else {
            QuotasNotice::Reset
        }),
        Err((status, problem)) => quotas_view(&state, parts, status, Some(problem), None).await,
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
    fn existing_names_follow_cups() {
        for name in [
            "office",
            "Büro",
            "HP+",
            "lab@home",
            &"a".repeat(127),
            &"é".repeat(63),
        ] {
            assert!(valid_queue_name(name), "{name}");
        }
        for name in [
            "",
            "a b",
            "a/b",
            "a\\b",
            "a?b",
            "a'b",
            "a\"b",
            "a#b",
            "a\tb",
            "a\u{7f}b",
            &"a".repeat(128),
            &"é".repeat(64),
        ] {
            assert!(!valid_queue_name(name), "{name:?}");
        }
    }

    #[test]
    fn texts_are_one_bounded_line() {
        for text in ["", "Office laser, 2nd floor", &"é".repeat(1023)] {
            assert!(valid_printer_text(text), "{text}");
        }
        for text in ["a\nb", "a\rb", "a\u{0}b", &"a".repeat(1024)] {
            assert!(!valid_printer_text(text), "{text:?}");
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
