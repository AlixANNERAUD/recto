//! `/account`: the signed-in user's account, where a user with a password changes it. The form
//! posts to `/account/password`; the server answers a change with `/account?changed=1`, and a
//! refused one by rendering this page again with a [`PasswordChange`]. Users without a password,
//! or with local accounts turned off, get the not-found page: there is nothing for them here.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use super::sign_in::SignInMethods;
use crate::{
    NotFound,
    components::ui::{Alert, ButtonVariant, Card, Field, INPUT, Tone, button_class},
    layout::Layout,
    t, tf,
};

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

/// The signed-in user's name and username, when they may change a password here.
#[cfg(feature = "ssr")]
fn account() -> Option<(String, String)> {
    let user = crate::state::current_user()?;
    let methods = SignInMethods::current()?;
    (methods.password && user.has_password).then_some((user.name, user.username))
}

/// Pages render on the server only.
#[cfg(not(feature = "ssr"))]
fn account() -> Option<(String, String)> {
    None
}

#[component]
pub fn AccountPage() -> impl IntoView {
    let Some((name, username)) = account() else {
        return view! { <NotFound /> }.into_any();
    };
    let minimum = SignInMethods::current().map_or(0, |methods| methods.password_minimum_length);
    let changed = use_query_map()
        .read_untracked()
        .get("changed")
        .is_some_and(|value| value == "1");
    let signed_in_as = tf!(
        "account.signed_in_as",
        name = name,
        username = username.clone()
    );
    let notice = if let Some(change) = use_context::<PasswordChange>() {
        Some(view! { <Alert tone=Tone::Danger>{change.refusal.message()}</Alert> }.into_any())
    } else {
        changed
            .then(|| view! { <Alert tone=Tone::Success>{t!("account.changed")}</Alert> }.into_any())
    };
    view! {
        <Layout title=t!("account.title")>
            {notice}
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
                    <p class="text-muted-foreground text-sm">{t!("account.password.others_signed_out")}</p>
                    <div>
                        <button type="submit" class=button_class(ButtonVariant::Primary)>
                            {t!("account.password.submit")}
                        </button>
                    </div>
                </form>
            </Card>
            <p class="text-muted-foreground text-sm">
                {signed_in_as}
            </p>
        </Layout>
    }
    .into_any()
}
