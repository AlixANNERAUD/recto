//! Web interface of recto, rendered on the server. Pages are complete HTML and actions are plain
//! forms, so print and cancel work without JavaScript; islands add the scan page and live
//! refresh.
//!
//! The crate is compiled twice. With `ssr`, for the server: pages render to HTML. With
//! `hydrate`, for the browser bundle built by the `frontend` crate: only islands run. The two
//! features are never enabled in the same build.
//!
//! Every string goes through [`t!`], [`tf!`] or [`tp!`] ([`i18n`]), with keys in
//! `locales/en.json`.

// Leptos views are deeply nested types; the print form's goes past the default limit.
#![recursion_limit = "256"]

// Every embedded locale must translate every message (strict): a missing key fails the build.
// Each request translates into its own locale (`locale = ...`, see [`i18n`]); the global locale
// of embedded-i18n stays unset, so concurrent requests cannot overwrite each other's language.
embedded_i18n::i18n!(
    "locales",
    default_locale = "en",
    available_locales = ["en"],
    strict = true
);

pub mod components;
pub mod display;
pub mod functions;
pub mod i18n;
pub mod islands;
pub mod layout;
pub mod messages;
pub mod pages;
pub mod refusal;
#[cfg(feature = "ssr")]
pub mod state;
pub mod theme;

use leptos::prelude::*;
use leptos_meta::{MetaTags, provide_meta_context};
use leptos_router::{
    SsrMode,
    components::{Route, Router, Routes},
    path,
};
use pages::{
    account::AccountPage,
    administration::{groups::GroupsPage, users::UsersPage},
    auth::{SignInErrorPage, SignedOutPage},
    jobs::JobsPage,
    no_access::NoAccessPage,
    print::PrintPage,
    printers::PrintersPage,
    scan::ScanPage,
    sign_in::SignInPage,
};

/// The page's Content Security Policy, for the request's nonce (Leptos makes one per request
/// and puts it on the hydration script). Only that inline script runs; the bundle comes from
/// recto itself and compiles its WebAssembly (`'wasm-unsafe-eval'`). Styles allow inline
/// attributes, which the supply meters and the scan selection use; a nonce would disable that.
/// Under `cargo leptos watch`, the live reload script also talks to its own WebSocket port.
#[cfg(feature = "ssr")]
pub fn content_security_policy(nonce: &str, live_reload: bool) -> String {
    let connect = if live_reload { "'self' ws:" } else { "'self'" };
    format!(
        "default-src 'self'; script-src 'self' 'nonce-{nonce}' 'wasm-unsafe-eval'; \
         style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src {connect}; \
         object-src 'none'; base-uri 'none'; frame-ancestors 'none'"
    )
}

/// Sends the page's [`content_security_policy`] with the response.
#[cfg(feature = "ssr")]
fn send_content_security_policy() {
    let (Some(nonce), Some(response)) = (
        leptos::nonce::use_nonce(),
        use_context::<leptos_axum::ResponseOptions>(),
    ) else {
        return;
    };
    let live_reload = std::env::var_os("LEPTOS_WATCH").is_some();
    if let Ok(value) =
        axum::http::HeaderValue::from_str(&content_security_policy(&nonce, live_reload))
    {
        response.insert_header(axum::http::header::CONTENT_SECURITY_POLICY, value);
    }
}

/// The HTML document around every page. Its `lang` attribute is the request's language, which
/// islands read back in the browser.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    #[cfg(feature = "ssr")]
    send_content_security_policy();
    let language = i18n::tag(i18n::locale());
    let theme = theme::current();
    view! {
        <!DOCTYPE html>
        <html lang=language data-theme=theme.attribute()>
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <meta name="color-scheme" content=theme.color_scheme() />
                <link rel="stylesheet" id="leptos" href="/pkg/recto.css" />
                <link rel="icon" href="/favicon.svg" type="image/svg+xml" />
                <AutoReload options=options.clone() />
                <HydrationScripts options islands=true />
                <MetaTags />
            </head>
            <body class="bg-background text-foreground min-h-screen font-sans antialiased">
                <App />
            </body>
        </html>
    }
}

/// The router: every page of the interface.
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    i18n::provide_locale();
    view! {
        <Router>
            <Routes fallback=NotFound>
                <Route path=path!("/") view=PrintPage ssr=SsrMode::Async />
                <Route path=path!("/printers") view=PrintersPage ssr=SsrMode::Async />
                <Route path=path!("/jobs") view=JobsPage ssr=SsrMode::Async />
                <Route path=path!("/scan") view=ScanPage ssr=SsrMode::Async />
                <Route path=path!("/account") view=AccountPage />
                <Route path=path!("/administration/users") view=UsersPage />
                <Route path=path!("/administration/groups") view=GroupsPage />
                <Route path=path!("/sign-in") view=SignInPage />
                <Route path=path!("/no-access") view=NoAccessPage />
                <Route path=path!("/auth/error") view=SignInErrorPage />
                <Route path=path!("/auth/signed-out") view=SignedOutPage />
            </Routes>
        </Router>
    }
}

/// The page of an address that leads nowhere, with status 404. Also what a page answers to a
/// user it has nothing for, such as the Account page to a user without a password or the
/// administration pages to anyone who does not administer recto.
#[component]
pub(crate) fn NotFound() -> impl IntoView {
    #[cfg(feature = "ssr")]
    if let Some(response) = use_context::<leptos_axum::ResponseOptions>() {
        response.set_status(axum::http::StatusCode::NOT_FOUND);
    }
    view! {
        <layout::Layout title=t!("not_found.heading").to_string()>
            <p>
                <a class="text-brand underline underline-offset-4" href="/">
                    {t!("not_found.home")}
                </a>
            </p>
        </layout::Layout>
    }
}
