//! `/administration/users`: every user, how they sign in, and the forms that create a local
//! user, grant or remove administration, disable or enable a user, and set a new password.
//! The server answers an accepted form with `/administration/users?done=<notice>&user=<id>`
//! and a refused one by rendering this page again with its [`UsersRefusal`].

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use super::{Section, Sections, page_data};
use crate::{
    NotFound,
    components::ui::{
        Alert, Badge, ButtonVariant, Card, Field, INPUT, LABEL, Tone, button_class,
        small_button_class,
    },
    display::{iso_time, relative_time, unix_now},
    layout::Layout,
    pages::sign_in::SignInMethods,
    t, tf,
};

/// A user as the page lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub name: String,
    pub email: Option<String>,
    /// Names of the user's groups, in alphabetical order.
    pub groups: Vec<String>,
    pub has_password: bool,
    pub single_sign_on: bool,
    pub is_administrator: bool,
    /// Administration was granted by the identity provider's claim, which also takes it back.
    pub administrator_from_claim: bool,
    pub is_disabled: bool,
    /// In seconds since the Unix epoch.
    pub last_sign_in: Option<i64>,
}

/// What the server hands the page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsersPageData {
    /// `None` when they could not be loaded.
    pub users: Option<Vec<UserRow>>,
    pub refusal: Option<UsersRefusal>,
    /// What the New local user form shows: after a refusal, what was typed but the password.
    pub draft: NewUserDraft,
}

/// The New local user form's fields, without the password, which never comes back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewUserDraft {
    pub username: String,
    pub display_name: String,
    pub email: String,
    pub administrator: bool,
}

/// A refused form: why, and the username of the user it was about, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsersRefusal {
    pub problem: UsersProblem,
    pub username: Option<String>,
}

/// Why a form of the page was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsersProblem {
    UsernameInvalid,
    UsernameTaken,
    EmailTaken,
    PasswordTooShort {
        minimum: usize,
    },
    PasswordTooLong {
        maximum: usize,
    },
    OwnAccount,
    UserNotFound,
    /// The database failed.
    Unavailable,
}

impl UsersRefusal {
    fn message(&self) -> String {
        let message = match self.problem {
            UsersProblem::UsernameInvalid => {
                t!("administration.errors.username_invalid").to_owned()
            }
            UsersProblem::UsernameTaken => t!("administration.errors.username_taken").to_owned(),
            UsersProblem::EmailTaken => t!("administration.errors.email_taken").to_owned(),
            UsersProblem::PasswordTooShort { minimum } => {
                tf!("administration.errors.too_short", minimum = minimum)
            }
            UsersProblem::PasswordTooLong { maximum } => {
                tf!("administration.errors.too_long", maximum = maximum)
            }
            UsersProblem::OwnAccount => t!("administration.errors.own_account").to_owned(),
            UsersProblem::UserNotFound => t!("administration.errors.not_found").to_owned(),
            UsersProblem::Unavailable => t!("administration.errors.unavailable").to_owned(),
        };
        match &self.username {
            Some(username) => tf!(
                "administration.errors.about",
                username = username.clone(),
                message = message
            ),
            None => message,
        }
    }
}

/// What an accepted form did, named by the `done` query parameter. Only the code and a user id
/// travel in the address, so a crafted link cannot put arbitrary text on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    Created,
    Granted,
    Removed,
    Disabled,
    Enabled,
    PasswordSet,
}

impl Notice {
    const ALL: [Notice; 6] = [
        Notice::Created,
        Notice::Granted,
        Notice::Removed,
        Notice::Disabled,
        Notice::Enabled,
        Notice::PasswordSet,
    ];

    /// The value of the `done` query parameter.
    pub fn code(self) -> &'static str {
        match self {
            Notice::Created => "created",
            Notice::Granted => "granted",
            Notice::Removed => "removed",
            Notice::Disabled => "disabled",
            Notice::Enabled => "enabled",
            Notice::PasswordSet => "password_set",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|notice| notice.code() == code)
    }

    fn message(self, username: String) -> String {
        match self {
            Notice::Created => tf!("administration.done.created", username = username),
            Notice::Granted => tf!("administration.done.granted", username = username),
            Notice::Removed => tf!("administration.done.removed", username = username),
            Notice::Disabled => tf!("administration.done.disabled", username = username),
            Notice::Enabled => tf!("administration.done.enabled", username = username),
            Notice::PasswordSet => tf!("administration.done.password_set", username = username),
        }
    }
}

/// The success notice the address names, for a user still listed.
fn notice(users: &[UserRow]) -> Option<String> {
    let query = use_query_map();
    let query = query.read_untracked();
    let notice = Notice::from_code(&query.get("done")?)?;
    let id = query.get("user")?.parse::<i64>().ok()?;
    let user = users.iter().find(|user| user.id == id)?;
    Some(notice.message(user.username.clone()))
}

#[component]
pub fn UsersPage() -> impl IntoView {
    let Some((me, data)) = page_data::<UsersPageData>() else {
        return view! { <NotFound /> }.into_any();
    };
    let methods = SignInMethods::current();
    let local_accounts = methods.is_some_and(|methods| methods.password);
    let minimum = methods.map_or(0, |methods| methods.password_minimum_length);
    let alert = if let Some(message) = data.refusal.as_ref().map(UsersRefusal::message) {
        Some(view! { <Alert tone=Tone::Danger>{message}</Alert> }.into_any())
    } else {
        data.users
            .as_deref()
            .and_then(notice)
            .map(|message| view! { <Alert tone=Tone::Success>{message}</Alert> }.into_any())
    };
    let create = local_accounts.then(|| view! { <NewUser draft=data.draft.clone() minimum /> });
    let list = match data.users {
        Some(users) => view! { <UserTable users me local_accounts minimum /> }.into_any(),
        None => view! {
            <Alert tone=Tone::Danger>{t!("administration.users.load_failed")}</Alert>
        }
        .into_any(),
    };
    view! {
        <Layout title=t!("administration.title")>
            <Sections current=Section::Users />
            {alert}
            {create}
            {list}
        </Layout>
    }
    .into_any()
}

/// The New local user form.
#[component]
fn NewUser(draft: NewUserDraft, minimum: usize) -> impl IntoView {
    view! {
        <Card title=t!("administration.new_user.title")>
            <form method="post" action="/administration/users" class="grid gap-4 sm:grid-cols-2">
                <Field
                    label=t!("administration.new_user.username")
                    input_id="new-username"
                    hint=t!("administration.new_user.username_hint")
                >
                    <input
                        id="new-username"
                        name="username"
                        type="text"
                        value=draft.username
                        required
                        maxlength="64"
                        autocomplete="off"
                        autocapitalize="none"
                        spellcheck="false"
                        aria-describedby="new-username-hint"
                        class=INPUT
                    />
                </Field>
                <Field
                    label=t!("administration.new_user.display_name")
                    input_id="new-display-name"
                    hint=t!("administration.new_user.display_name_hint")
                >
                    <input
                        id="new-display-name"
                        name="display_name"
                        type="text"
                        value=draft.display_name
                        autocomplete="off"
                        aria-describedby="new-display-name-hint"
                        class=INPUT
                    />
                </Field>
                <Field label=t!("administration.new_user.email") input_id="new-email">
                    <input
                        id="new-email"
                        name="email"
                        type="email"
                        value=draft.email
                        autocomplete="off"
                        class=INPUT
                    />
                </Field>
                <Field
                    label=t!("administration.new_user.password")
                    input_id="new-password"
                    hint=tf!("administration.new_user.password_hint", minimum = minimum)
                >
                    <input
                        id="new-password"
                        name="password"
                        type="password"
                        required
                        minlength=minimum
                        autocomplete="new-password"
                        aria-describedby="new-password-hint"
                        class=INPUT
                    />
                </Field>
                <label class=format!("{LABEL} flex min-h-10 items-center gap-2 sm:col-span-2")>
                    <input
                        type="checkbox"
                        name="administrator"
                        checked=draft.administrator
                        class="accent-brand size-4"
                    />
                    {t!("administration.new_user.administrator")}
                </label>
                <div class="sm:col-span-2">
                    <button type="submit" class=button_class(ButtonVariant::Primary)>
                        {t!("administration.new_user.submit")}
                    </button>
                </div>
            </form>
        </Card>
    }
}

const HEAD: &str =
    "text-muted-foreground px-3 py-2 text-left text-xs font-semibold whitespace-nowrap";
/// From `md` a table cell; on phones a block of the row's card, under a [`label`].
const CELL: &str = "block md:table-cell md:px-3 md:py-3 md:align-top";

/// The column's name above a value, on phones only: there the column headings are hidden.
fn label(text: &'static str) -> impl IntoView + use<> {
    view! { <span class="text-muted-foreground mb-1 block text-xs font-semibold md:hidden">{text}</span> }
}

/// Every user. From `md` a table, which scrolls sideways inside its card when the window is
/// narrow, with the user column kept in view; on phones each user is a block of its own, so
/// the actions are in reach without scrolling. The ARIA roles keep the table's meaning for
/// screen readers, which some browsers drop once the elements no longer display as a table.
#[component]
fn UserTable(users: Vec<UserRow>, me: i64, local_accounts: bool, minimum: usize) -> impl IntoView {
    let now = unix_now();
    let rows = users
        .into_iter()
        .map(|user| view! { <UserTableRow user me local_accounts minimum now /> })
        .collect_view();
    view! {
        <section class="bg-card overflow-hidden rounded-lg border shadow-[0_1px_2px_rgba(0,0,0,0.08)]">
            // Positioned, so the visually hidden heading of the last column stays inside it too.
            <div class="relative overflow-x-auto">
                <table role="table" class="block w-full text-sm md:table">
                    <caption class="block px-4 pt-4 pb-3 text-left text-base font-semibold md:table-caption md:px-6 md:pt-5">
                        {t!("administration.users.list")}
                    </caption>
                    <thead role="rowgroup" class="hidden border-y md:table-header-group">
                        <tr role="row">
                            <th role="columnheader" scope="col" class=format!("{HEAD} bg-card sticky left-0 pl-4 md:pl-6")>
                                {t!("administration.users.columns.user")}
                            </th>
                            <th role="columnheader" scope="col" class=HEAD>
                                {t!("administration.users.columns.groups")}
                            </th>
                            <th role="columnheader" scope="col" class=HEAD>
                                {t!("administration.users.columns.sign_in")}
                            </th>
                            <th role="columnheader" scope="col" class=HEAD>
                                {t!("administration.users.columns.status")}
                            </th>
                            <th role="columnheader" scope="col" class=HEAD>
                                {t!("administration.users.columns.last_sign_in")}
                            </th>
                            <th role="columnheader" scope="col" class=format!("{HEAD} pr-4 md:pr-6")>
                                <span class="sr-only">{t!("administration.users.columns.actions")}</span>
                            </th>
                        </tr>
                    </thead>
                    <tbody role="rowgroup" class="block divide-y border-t md:table-row-group md:border-t-0">
                        {rows}
                    </tbody>
                </table>
            </div>
        </section>
    }
}

#[component]
fn UserTableRow(
    user: UserRow,
    me: i64,
    local_accounts: bool,
    minimum: usize,
    now: i64,
) -> impl IntoView {
    let groups = if user.groups.is_empty() {
        view! { <span class="text-muted-foreground">{t!("administration.users.no_groups")}</span> }
            .into_any()
    } else {
        view! { <span>{user.groups.join(", ")}</span> }.into_any()
    };
    let methods = {
        let single_sign_on = user.single_sign_on.then(|| {
            view! { <Badge>{t!("administration.users.single_sign_on")}</Badge> }
        });
        let password = user
            .has_password
            .then(|| view! { <Badge>{t!("administration.users.password")}</Badge> });
        let none = (!user.single_sign_on && !user.has_password).then(|| {
            view! { <span class="text-muted-foreground">{t!("administration.users.no_method")}</span> }
        });
        view! { <div class="grid justify-items-start gap-1">{single_sign_on}{password}{none}</div> }
    };
    // Whether the user may sign in, then whether they administer recto: one column, so the
    // table fits the page beside the actions on a laptop.
    let enabled = if user.is_disabled {
        view! { <Badge tone=Tone::Danger>{t!("administration.users.disabled")}</Badge> }.into_any()
    } else {
        view! { <Badge tone=Tone::Success>{t!("administration.users.active")}</Badge> }.into_any()
    };
    let administrator = user.is_administrator.then(|| {
        let from_claim = user.administrator_from_claim.then(|| {
            view! { <span class="text-muted-foreground text-xs">{t!("administration.users.from_claim")}</span> }
        });
        view! {
            <Badge tone=Tone::Brand>{t!("administration.users.administrator")}</Badge>
            {from_claim}
        }
    });
    let status =
        view! { <div class="grid justify-items-start gap-1">{enabled}{administrator}</div> };
    let last_sign_in = match user.last_sign_in {
        Some(unix) => view! {
            <time datetime=iso_time(unix) class="whitespace-nowrap">{relative_time(unix, now)}</time>
        }
        .into_any(),
        None => view! { <span class="text-muted-foreground">{t!("administration.users.never")}</span> }
            .into_any(),
    };
    let actions = if user.id == me {
        view! {
            <span class="text-muted-foreground block md:text-right">{t!("administration.users.you")}</span>
        }
        .into_any()
    } else {
        view! { <UserActions user=user.clone() local_accounts minimum /> }.into_any()
    };
    let email = user.email.clone().map(
        |email| view! { <span class="text-muted-foreground block max-md:break-all">{email}</span> },
    );
    view! {
        <tr role="row" class="grid grid-cols-2 gap-x-3 gap-y-3 px-4 py-4 md:table-row md:p-0">
            <th
                role="rowheader"
                scope="row"
                class=format!(
                    "{CELL} col-span-2 text-left font-normal md:bg-card md:sticky md:left-0 md:pl-6",
                )
            >
                <span class="block font-semibold md:whitespace-nowrap">{user.name.clone()}</span>
                <span class="text-muted-foreground block">{user.username.clone()}</span>
                {email}
            </th>
            <td role="cell" class=CELL>
                {label(t!("administration.users.columns.groups"))}
                {groups}
            </td>
            <td role="cell" class=CELL>
                {label(t!("administration.users.columns.sign_in"))}
                {methods}
            </td>
            <td role="cell" class=CELL>
                {label(t!("administration.users.columns.status"))}
                {status}
            </td>
            <td role="cell" class=CELL>
                {label(t!("administration.users.columns.last_sign_in"))}
                {last_sign_in}
            </td>
            <td role="cell" class=format!("{CELL} col-span-2 md:pr-6")>{actions}</td>
        </tr>
    }
}

/// The forms acting on another user. Administration granted by the identity provider has no
/// toggle: the provider's claim would take it back or grant it again at the next sign-in.
#[component]
fn UserActions(user: UserRow, local_accounts: bool, minimum: usize) -> impl IntoView {
    let id = user.id;
    let username = user.username;
    let administration = (!user.administrator_from_claim).then(|| {
        let (label, aria) = if user.is_administrator {
            (
                t!("administration.users.remove"),
                tf!(
                    "administration.users.remove_label",
                    username = username.clone()
                ),
            )
        } else {
            (
                t!("administration.users.grant"),
                tf!(
                    "administration.users.grant_label",
                    username = username.clone()
                ),
            )
        };
        view! { <Toggle id action="administrator" value=!user.is_administrator label aria /> }
    });
    let (label, aria) = if user.is_disabled {
        (
            t!("administration.users.enable"),
            tf!(
                "administration.users.enable_label",
                username = username.clone()
            ),
        )
    } else {
        (
            t!("administration.users.disable"),
            tf!(
                "administration.users.disable_label",
                username = username.clone()
            ),
        )
    };
    let disabled = view! { <Toggle id action="disabled" value=!user.is_disabled label aria /> };
    // Only local accounts sign in with a password, so only theirs is worth setting.
    let password = (local_accounts && user.has_password).then(|| {
        let input_id = format!("password-{id}");
        let note_id = format!("password-{id}-note");
        view! {
            <details class="group">
                <summary class=format!(
                    "{} list-none [&::-webkit-details-marker]:hidden",
                    small_button_class(ButtonVariant::Secondary),
                )>{t!("administration.users.set_password")}</summary>
                <form
                    method="post"
                    action=format!("/administration/users/{id}/password")
                    class="mt-2 grid w-60 gap-2 text-left"
                >
                    <label for=input_id.clone() class=LABEL>
                        {tf!("administration.users.new_password_for", username = username.clone())}
                    </label>
                    <div class="flex gap-2">
                        <input
                            id=input_id
                            name="password"
                            type="password"
                            required
                            minlength=minimum
                            autocomplete="new-password"
                            aria-describedby=note_id.clone()
                            class=INPUT
                        />
                        <button type="submit" class=button_class(ButtonVariant::Primary)>
                            {t!("administration.users.set_password_submit")}
                        </button>
                    </div>
                    <p id=note_id class="text-muted-foreground text-xs">
                        {tf!("administration.users.set_password_note", username = username.clone())}
                    </p>
                </form>
            </details>
        }
    });
    view! {
        <div class="flex flex-wrap items-start gap-2 md:flex-col md:flex-nowrap md:items-end md:gap-1.5">
            {administration}
            {disabled}
            {password}
        </div>
    }
}

/// A form that sets one yes-or-no setting of a user to `value`.
#[component]
fn Toggle(
    id: i64,
    action: &'static str,
    value: bool,
    label: &'static str,
    aria: String,
) -> impl IntoView {
    view! {
        <form method="post" action=format!("/administration/users/{id}/{action}")>
            <input type="hidden" name="value" value=value.to_string() />
            <button
                type="submit"
                aria-label=aria
                class=small_button_class(ButtonVariant::Secondary)
            >
                {label}
            </button>
        </form>
    }
}
