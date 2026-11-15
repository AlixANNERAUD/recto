//! `/sign-in`: the password form of local accounts and, when a provider is configured, a link to
//! single sign-on. It is a plain form, so it works without JavaScript. The server renders it for
//! `GET /sign-in` and again for a refused `POST /sign-in`, handing it a [`SignInForm`]; with only
//! single sign-on configured, the server sends visitors straight to the provider instead.

use leptos::prelude::*;

use super::auth::encode_component;
use crate::{
    components::ui::{Alert, ButtonVariant, Field, INPUT, Tone, button_class},
    layout::Bare,
    t,
};

/// How people sign in, from the server's configuration. The server provides it to every page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignInMethods {
    /// Local accounts sign in with a password.
    pub password: bool,
    /// An OpenID Connect provider is configured.
    pub single_sign_on: bool,
    /// The minimum length of a new password.
    pub password_minimum_length: usize,
}

impl SignInMethods {
    /// The methods the server provided, if any.
    pub fn current() -> Option<Self> {
        use_context::<Self>()
    }
}

/// What the sign-in page shows: where to go once signed in and, after a refused attempt, the
/// login typed and why it was refused. The password typed never comes back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignInForm {
    /// A path on this site, already checked by the server.
    pub next: String,
    pub login: String,
    pub refusal: Option<SignInRefusal>,
}

/// Why a password sign-in was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignInRefusal {
    /// The login or the password is wrong, or the account cannot sign in with a password. One
    /// message for all, so the page does not tell which logins exist.
    Invalid,
    /// Too many attempts from the same address.
    TooMany,
    /// The database failed.
    Unavailable,
}

impl SignInRefusal {
    fn message(self) -> &'static str {
        match self {
            SignInRefusal::Invalid => t!("auth.password.invalid"),
            SignInRefusal::TooMany => t!("auth.password.too_many"),
            SignInRefusal::Unavailable => t!("auth.password.unavailable"),
        }
    }
}

#[component]
pub fn SignInPage() -> impl IntoView {
    let methods = SignInMethods::current().unwrap_or(SignInMethods {
        password: true,
        single_sign_on: false,
        password_minimum_length: 0,
    });
    let form = use_context::<SignInForm>().unwrap_or_else(|| SignInForm {
        next: "/".to_owned(),
        ..SignInForm::default()
    });
    let alert = form
        .refusal
        .map(|refusal| view! { <Alert tone=Tone::Danger>{refusal.message()}</Alert> });
    let single_sign_on = methods.single_sign_on.then(|| {
        let href = format!("/auth/login?next={}", encode_component(&form.next));
        // Next to the password form it is the second choice; alone it is the only one.
        let variant = if methods.password {
            ButtonVariant::Secondary
        } else {
            ButtonVariant::Primary
        };
        view! {
            <a href=href class=format!("{} w-full", button_class(variant))>
                {t!("auth.single_sign_on")}
            </a>
        }
    });
    let separator = (methods.password && methods.single_sign_on).then(|| {
        view! {
            <p class="text-muted-foreground flex items-center gap-3 text-sm">
                <span aria-hidden="true" class="h-px flex-1 bg-current opacity-30"></span>
                {t!("auth.or")}
                <span aria-hidden="true" class="h-px flex-1 bg-current opacity-30"></span>
            </p>
        }
    });
    let password = methods.password.then(|| {
        // After a refusal the login is usually right: the cursor waits in the password.
        let login_focus = form.login.is_empty();
        view! {
            <form method="post" action="/sign-in" class="grid gap-4">
                <input type="hidden" name="next" value=form.next.clone() />
                <Field label=t!("auth.password.login") input_id="login">
                    <input
                        id="login"
                        name="login"
                        type="text"
                        value=form.login.clone()
                        required
                        autofocus=login_focus
                        autocomplete="username"
                        autocapitalize="none"
                        spellcheck="false"
                        class=INPUT
                    />
                </Field>
                <Field label=t!("auth.password.password") input_id="password">
                    <input
                        id="password"
                        name="password"
                        type="password"
                        required
                        autofocus=!login_focus
                        autocomplete="current-password"
                        class=INPUT
                    />
                </Field>
                <button type="submit" class=format!("{} w-full", button_class(ButtonVariant::Primary))>
                    {t!("auth.sign_in")}
                </button>
            </form>
        }
    });
    view! {
        <Bare title=t!("auth.sign_in")>
            {alert}
            {password}
            {separator}
            {single_sign_on}
        </Bare>
    }
}
