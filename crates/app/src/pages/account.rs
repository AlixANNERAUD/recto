//! `/account`: the signed-in user's account. A user with a password changes it here, and with
//! OS integration on everyone keeps the device passwords their computers and phones print and
//! scan with. Users with neither get the not-found page: there is nothing for them here.
//!
//! The password form posts to `/account/password`; the server answers a change with
//! `/account?changed=1`, and a refused one by rendering this page again with a
//! [`PasswordChange`]. The device forms post to `/account/devices` and
//! `/account/devices/<id>/revoke`: a new password is answered with this page and its
//! [`NewDevicePassword`], never a redirect, so the secret stays out of addresses and history; a
//! revoked one with `/account?done=revoked`; a refused one with this page and its
//! [`DevicesRefusal`].

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use super::{failure_alert, sign_in::SignInMethods};
use crate::{
    NotFound,
    components::ui::{
        Alert, ButtonVariant, Card, Field, HINT, INPUT, Tone, button_class, small_button_class,
    },
    display::{iso_time, relative_time, unix_now},
    functions::{DeviceRow, my_device_passwords},
    layout::Layout,
    t, tf,
};

/// The longest device name the form takes, as the server checks it.
const DEVICE_NAME_MAX: usize = 64;

/// A refused password change, which the page shows above the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasswordChange {
    pub refusal: PasswordRefusal,
}

/// Why a password change was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordRefusal {
    WrongCurrent,
    Mismatch,
    TooShort {
        minimum: usize,
    },
    TooLong {
        maximum: usize,
    },
    /// Too many password attempts from this address, sign-in included.
    TooMany,
    /// The database failed.
    Unavailable,
}

impl PasswordRefusal {
    fn message(self) -> String {
        match self {
            PasswordRefusal::WrongCurrent => t!("account.errors.wrong_current").to_owned(),
            PasswordRefusal::Mismatch => t!("account.errors.mismatch").to_owned(),
            PasswordRefusal::TooShort { minimum } => {
                tf!("account.errors.too_short", minimum = minimum)
            }
            PasswordRefusal::TooLong { maximum } => {
                tf!("account.errors.too_long", maximum = maximum)
            }
            PasswordRefusal::TooMany => t!("account.errors.too_many").to_owned(),
            PasswordRefusal::Unavailable => t!("account.errors.unavailable").to_owned(),
        }
    }
}

/// A device password just created: shown once, above everything else.
#[derive(Clone, PartialEq, Eq)]
pub struct NewDevicePassword {
    pub name: String,
    pub secret: String,
    /// What a password prompt may ask for besides the password; recto ignores it.
    pub username: String,
}

/// A refused device form: why, and the name typed, which the form shows again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicesRefusal {
    pub problem: DevicesProblem,
    pub name: String,
}

/// Why a device form was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicesProblem {
    NameInvalid {
        maximum: usize,
    },
    /// The name has a line break, a tab or another control character.
    NameControl,
    TooMany {
        maximum: usize,
    },
    /// The database failed.
    Unavailable,
}

impl DevicesProblem {
    fn message(self) -> String {
        match self {
            DevicesProblem::NameInvalid { maximum } => {
                tf!("account.devices.errors.name_invalid", maximum = maximum)
            }
            DevicesProblem::NameControl => t!("account.devices.errors.name_control").to_owned(),
            DevicesProblem::TooMany { maximum } => {
                tf!("account.devices.errors.too_many", maximum = maximum)
            }
            DevicesProblem::Unavailable => t!("account.devices.errors.unavailable").to_owned(),
        }
    }
}

/// What the page holds for a user: the password form, the Devices section, or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sections {
    password: bool,
    devices: bool,
}

impl Sections {
    fn any(self) -> bool {
        self.password || self.devices
    }
}

#[cfg(feature = "ssr")]
fn sections(user: &crate::state::User) -> Sections {
    let password = SignInMethods::current().is_some_and(|methods| methods.password);
    let devices = crate::state::app_state().is_some_and(|state| state.os_integration);
    Sections {
        password: password && user.has_password,
        devices,
    }
}

/// Whether the page has anything for the user, so the header links to it.
#[cfg(feature = "ssr")]
pub fn has_account_page(user: &crate::state::User) -> bool {
    sections(user).any()
}

/// The signed-in user's name and username, and what the page holds for them.
#[cfg(feature = "ssr")]
fn account() -> Option<(String, String, Sections)> {
    let user = crate::state::current_user()?;
    let sections = sections(&user);
    Some((user.name, user.username, sections))
}

/// Pages render on the server only.
#[cfg(not(feature = "ssr"))]
fn account() -> Option<(String, String, Sections)> {
    None
}

#[component]
pub fn AccountPage() -> impl IntoView {
    let Some((name, username, sections)) = account().filter(|(_, _, sections)| sections.any())
    else {
        return view! { <NotFound /> }.into_any();
    };
    let (changed, revoked) = {
        let query = use_query_map();
        let query = query.read_untracked();
        (
            query.get("changed").is_some_and(|value| value == "1"),
            query.get("done").is_some_and(|value| value == "revoked"),
        )
    };
    let signed_in_as = tf!(
        "account.signed_in_as",
        name = name,
        username = username.clone()
    );
    let devices_refusal = use_context::<DevicesRefusal>();
    let devices_problem = devices_refusal
        .as_ref()
        .map(|refusal| refusal.problem.message());
    let notice = if let Some(change) = use_context::<PasswordChange>() {
        Some(view! { <Alert tone=Tone::Danger>{change.refusal.message()}</Alert> }.into_any())
    } else if let Some(message) = devices_problem {
        Some(view! { <Alert tone=Tone::Danger>{message}</Alert> }.into_any())
    } else if changed {
        Some(view! { <Alert tone=Tone::Success>{t!("account.changed")}</Alert> }.into_any())
    } else if revoked {
        Some(view! { <Alert tone=Tone::Success>{t!("account.devices.revoked")}</Alert> }.into_any())
    } else {
        None
    };
    let created = use_context::<NewDevicePassword>()
        .filter(|_| sections.devices)
        .map(|created| view! { <CreatedPassword created /> });
    let password = sections
        .password
        .then(|| view! { <PasswordForm username=username.clone() /> });
    let devices = sections.devices.then(|| {
        let draft = devices_refusal
            .map(|refusal| refusal.name)
            .unwrap_or_default();
        view! { <Devices draft /> }
    });
    view! {
        <Layout title=t!("account.title")>
            {notice}
            {created}
            {password}
            {devices}
            <p class="text-muted-foreground text-sm">
                {signed_in_as}
            </p>
        </Layout>
    }
    .into_any()
}

#[component]
fn PasswordForm(username: String) -> impl IntoView {
    let minimum = SignInMethods::current().map_or(0, |methods| methods.password_minimum_length);
    view! {
        <Card title=t!("account.password.title") class="max-w-xl">
            <form method="post" action="/account/password" class="grid gap-4">
                // For password managers, which file the new password under this username.
                <input
                    type="text"
                    name="username"
                    value=username
                    autocomplete="username"
                    readonly
                    hidden
                />
                <Field label=t!("account.password.current") input_id="current">
                    <input
                        id="current"
                        name="current"
                        type="password"
                        required
                        autocomplete="current-password"
                        class=INPUT
                    />
                </Field>
                <Field
                    label=t!("account.password.new")
                    input_id="new"
                    hint=tf!("account.password.hint", minimum = minimum)
                >
                    <input
                        id="new"
                        name="new"
                        type="password"
                        required
                        minlength=minimum
                        autocomplete="new-password"
                        aria-describedby="new-hint"
                        class=INPUT
                    />
                </Field>
                <Field label=t!("account.password.confirm") input_id="confirm">
                    <input
                        id="confirm"
                        name="confirm"
                        type="password"
                        required
                        minlength=minimum
                        autocomplete="new-password"
                        class=INPUT
                    />
                </Field>
                <p class=HINT>{t!("account.password.others_signed_out")}</p>
                <div>
                    <button type="submit" class=button_class(ButtonVariant::Primary)>
                        {t!("account.password.submit")}
                    </button>
                </div>
            </form>
        </Card>
    }
}

/// The password just created, its name and the username to go with it.
#[component]
fn CreatedPassword(created: NewDevicePassword) -> impl IntoView {
    view! {
        <Card title=tf!("account.devices.created.title", name = created.name) class="max-w-xl">
            <div class="grid gap-4">
                <Alert tone=Tone::Warning>{t!("account.devices.created.once")}</Alert>
                <dl class="grid gap-3 text-sm">
                    <div class="grid gap-1">
                        <dt class="font-semibold">{t!("account.devices.created.password")}</dt>
                        <dd>
                            <code class="bg-muted rounded-md px-2 py-1 font-mono text-base break-all select-all">
                                {created.secret}
                            </code>
                        </dd>
                    </div>
                    <div class="grid gap-1">
                        <dt class="font-semibold">{t!("account.devices.created.username")}</dt>
                        <dd>
                            <code class="font-mono">{created.username}</code>
                        </dd>
                    </div>
                </dl>
                <p class=HINT>{t!("account.devices.created.username_hint")}</p>
            </div>
        </Card>
    }
}

/// The user's device passwords and the form creating one; `draft` is the name a refused form
/// typed.
#[component]
fn Devices(draft: String) -> impl IntoView {
    let passwords = OnceResource::new(my_device_passwords());
    view! {
        <Card title=t!("account.devices.title") class="max-w-xl">
            <div class="grid gap-4">
                <p class=HINT>{t!("account.devices.intro")}</p>
                <Suspense>
                    {move || Suspend::new(async move {
                        match passwords.await {
                            Ok(rows) => view! { <DeviceList rows now=unix_now() /> }.into_any(),
                            Err(error) => failure_alert(&error).into_any(),
                        }
                    })}
                </Suspense>
                <form method="post" action="/account/devices" class="grid gap-4">
                    <Field
                        label=t!("account.devices.name")
                        input_id="device-name"
                        hint=t!("account.devices.name_hint")
                    >
                        <input
                            id="device-name"
                            name="name"
                            type="text"
                            required
                            maxlength=DEVICE_NAME_MAX
                            autocomplete="off"
                            aria-describedby="device-name-hint"
                            value=draft
                            class=INPUT
                        />
                    </Field>
                    <div>
                        <button type="submit" class=button_class(ButtonVariant::Primary)>
                            {t!("account.devices.create")}
                        </button>
                    </div>
                </form>
            </div>
        </Card>
    }
}

#[component]
fn DeviceList(rows: Vec<DeviceRow>, now: i64) -> impl IntoView {
    if rows.is_empty() {
        return view! { <p class="text-sm">{t!("account.devices.none")}</p> }.into_any();
    }
    view! {
        <ul class="divide-y rounded-md border">
            {rows
                .into_iter()
                .map(|row| {
                    let last_used = match row.last_used_at {
                        Some(unix) => {
                            tf!("account.devices.last_used", time = relative_time(unix, now))
                        }
                        None => t!("account.devices.never_used").to_owned(),
                    };
                    let revoke_label = tf!("account.devices.revoke_label", name = row.name.clone());
                    view! {
                        <li class="flex flex-wrap items-center justify-between gap-3 px-3 py-2">
                            <div class="grid min-w-0 gap-0.5">
                                <span class="font-semibold break-words">{row.name}</span>
                                <span class="text-muted-foreground text-xs">
                                    <time datetime=iso_time(row.created_at)>
                                        {tf!(
                                            "account.devices.created_at",
                                            time = relative_time(row.created_at, now)
                                        )}
                                    </time>
                                    " · "
                                    {last_used}
                                </span>
                            </div>
                            <form method="post" action=format!("/account/devices/{}/revoke", row.id)>
                                <button
                                    type="submit"
                                    aria-label=revoke_label
                                    class=small_button_class(ButtonVariant::Secondary)
                                >
                                    {t!("account.devices.revoke")}
                                </button>
                            </form>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
    .into_any()
}
