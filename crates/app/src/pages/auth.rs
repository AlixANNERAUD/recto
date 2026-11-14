//! The pages around sign-in. They render without a session: a failed sign-in, a refused account
//! or a sign-out leaves the visitor without one.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_query_map;

use crate::t;

/// `/auth/error?reason=<unavailable|state|rejected>&next=<path>`: why the sign-in failed, with
/// a link starting it again and coming back to `next`.
#[component]
pub fn SignInErrorPage() -> impl IntoView {
    let query = use_query_map();
    let message = move || match query.read().get("reason").as_deref() {
        Some("unavailable") => t!("auth.errors.unavailable"),
        Some("rejected") => t!("auth.errors.rejected"),
        _ => t!("auth.errors.state"),
    };
    let retry = move || {
        let next = query.read().get("next").unwrap_or_else(|| "/".to_owned());
        format!("/auth/login?next={}", encode_component(&next))
    };
    view! {
        <Title text=t!("auth.errors.title") />
        <main class="mx-auto max-w-xl p-8">
            <h1 class="text-2xl font-semibold">{t!("auth.errors.heading")}</h1>
            <p role="alert">{message}</p>
            <p>
                <a href=retry>{t!("auth.errors.retry")}</a>
            </p>
        </main>
    }
}

/// `/no-access`: the provider signed the user in, but the required claim was missing.
#[component]
pub fn NoAccessPage() -> impl IntoView {
    view! {
        <Title text=t!("auth.no_access.title") />
        <main class="mx-auto max-w-xl p-8">
            <h1 class="text-2xl font-semibold">{t!("auth.no_access.heading")}</h1>
            <p>{t!("auth.no_access.body")}</p>
            <SignOutButton />
        </main>
    }
}

/// `/auth/signed-out`, where the provider sends the browser back after sign-out.
#[component]
pub fn SignedOutPage() -> impl IntoView {
    view! {
        <Title text=t!("auth.signed_out.title") />
        <main class="mx-auto max-w-xl p-8">
            <h1 class="text-2xl font-semibold">{t!("auth.signed_out.heading")}</h1>
            <p>
                <a href="/auth/login">{t!("auth.signed_out.sign_in")}</a>
            </p>
        </main>
    }
}

/// A plain form, so signing out works without JavaScript.
#[component]
pub fn SignOutButton() -> impl IntoView {
    view! {
        <form method="post" action="/auth/logout">
            <button type="submit">{t!("auth.sign_out")}</button>
        </form>
    }
}

/// Percent-encodes everything but unreserved characters, for a query parameter value.
pub fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::encode_component;

    #[test]
    fn encodes_query_values() {
        assert_eq!(encode_component("/jobs?a=1&b"), "%2Fjobs%3Fa%3D1%26b");
        assert_eq!(encode_component("é"), "%C3%A9");
    }
}
